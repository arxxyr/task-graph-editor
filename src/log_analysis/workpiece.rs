//! 工件节拍：北京时间、进程边界、整数微秒和可追溯证据独立于 GUI。

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use super::{AnalysisReport, AnalysisRequest, FileReport, StatisticsOptions};

mod report;
#[cfg(test)]
mod tests;

const ANCHOR: &str = "ROS2ActionNode[normal_nav_pick] - 创建成功";
const WAIT_START: &str = "SequenceNode[wait_pending_box_before_pick_seq] - 开始执行";
const WAIT_END: &str =
    "SlotFlowSchedulerNode[wait_pending_box_before_pick_done] action=box_done success";
const BEIJING_US: i64 = 8 * 3600 * 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub date: String,
    pub include_interventions: bool,
    pub deduct_wait: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            date: String::new(),
            include_interventions: false,
            deduct_wait: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Evidence {
    timestamp_us: i64,
    line: usize,
    text: String,
}
#[derive(Default)]
struct Event {
    evidence: Option<Evidence>,
    thread: String,
    anchor: bool,
    success: Option<(usize, u8)>,
    pause: bool,
    resume: bool,
    error: bool,
    manual: bool,
    retry_anchor: bool,
    wait_start: bool,
    wait_end: bool,
}
impl Event {
    fn evidence(&self) -> &Evidence {
        self.evidence.as_ref().expect("保留的事件必须携带证据")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Interval {
    start: Evidence,
    end: Option<Evidence>,
    observed_end_us: i64,
    reliable: bool,
    note: String,
}
impl Interval {
    fn overlaps(&self, start: i64, end: i64) -> bool {
        self.start.timestamp_us < end && self.observed_end_us > start
            || self.start.timestamp_us >= start && self.start.timestamp_us < end
    }
}
#[derive(Debug, Serialize, Deserialize)]
struct Cycle {
    id: String,
    process: String,
    source: PathBuf,
    start: Evidence,
    end: Option<Evidence>,
    station: Option<u8>,
    success: Vec<Evidence>,
    raw_us: Option<i64>,
    wait_us: i64,
    net_us: Option<i64>,
    stages_us: Vec<i64>,
    stage_wait_us: Vec<i64>,
    pauses: Vec<Interval>,
    waits: Vec<Interval>,
    errors: Vec<Evidence>,
    interventions: Vec<Evidence>,
    structural_reasons: Vec<String>,
    reasons: Vec<String>,
    included: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct SourceAudit {
    source: PathBuf,
    identity: String,
    first_us: Option<i64>,
    last_us: Option<i64>,
    processes: usize,
    ignored_tail: bool,
    ignored_lines: usize,
    warnings: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
struct Dataset {
    schema_version: u32,
    timezone: String,
    options: Options,
    statistics: StatisticsOptions,
    sources: Vec<SourceAudit>,
    duplicates: Vec<String>,
    cycles: Vec<Cycle>,
}

/// 时间戳是 UTC 微秒；新格式的无时区日期显式解释为北京时间。
fn timestamp(line: &str) -> Option<i64> {
    if line.starts_with('[') {
        let (_, rest) = line.split_once("] [")?;
        let epoch = rest.split_once(']')?.0;
        let (seconds, fraction) = epoch.split_once('.').unwrap_or((epoch, ""));
        let seconds = seconds.parse::<i64>().ok()?;
        if !fraction.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let fraction = format!("{fraction:0<6}");
        return seconds
            .checked_mul(1_000_000)?
            .checked_add(fraction.get(..6)?.parse::<i64>().ok()?);
    }
    let mut parts = line.splitn(3, ' ');
    let value = format!("{} {}", parts.next()?, parts.next()?);
    NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f")
        .ok()?
        .and_utc()
        .timestamp_micros()
        .checked_sub(BEIJING_US)
}
fn beijing(ts: i64) -> String {
    chrono::DateTime::from_timestamp_micros(ts + BEIJING_US)
        .map(|date| date.format("%Y-%m-%d %H:%M:%S%.6f").to_string())
        .unwrap_or_default()
}
fn nonzero_code(text: &str) -> bool {
    ["error_code: ", "error_code=", "result code=", "结果码: "]
        .iter()
        .any(|prefix| {
            text.split_once(prefix)
                .and_then(|(_, value)| {
                    value
                        .split(|c: char| c != '-' && !c.is_ascii_digit())
                        .next()?
                        .parse::<i64>()
                        .ok()
                })
                .is_some_and(|value| value != 0)
        })
}
fn classify(line: &str, number: usize, ts: i64) -> Option<Event> {
    // 初始化打印的是配置模板，不能视为真实执行或控制事件。
    if line.contains("LogNode[") && line.contains("初始化完成:") {
        return None;
    }
    let body = line.split_once(" - ").map(|(_, body)| body).unwrap_or(line);
    let severe = line
        .split('[')
        .filter_map(|part| part.split_once(']').map(|(token, _)| token))
        .find_map(|token| match token {
            "ERROR" | "FATAL" | "CRITICAL" => Some(true),
            "TRACE" | "DEBUG" | "INFO" | "WARN" | "WARNING" => Some(false),
            _ => None,
        })
        .unwrap_or(false);
    let success = body.split_once("BehaviorTreeNode ").and_then(|(_, rest)| {
        let (node, result) =
            rest.split_once(": 已发布任务日志 [INFO] BehaviorTree 执行成功 - error_code: ")?;
        let code = result
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse::<u32>()
            .ok()?;
        if code != 0 {
            return None;
        }
        match node {
            "normal_arm_pick_merged" => Some((0, 0)),
            "normal_arm_leak_grab_and_place_station_1_merged" => Some((1, 1)),
            "normal_arm_leak_grab_and_place_station_2_merged" => Some((1, 2)),
            "normal_arm_put_merged" => Some((2, 0)),
            _ => None,
        }
    });
    let control = body.contains("TaskGraphExecutor:")
        || body.contains("[TaskGraph]")
        || body.contains("BehaviorTreeNode")
        || body.starts_with("开始执行任务图:")
        || body.starts_with("重试失败节点:");
    let manual = control
        && [
            "重试节点",
            "重试失败节点:",
            "用户选择重试",
            "跳过节点",
            "用户选择跳过",
            "取消执行",
            "取消任务",
            "开始执行任务图:",
            "切换任务",
            "重新启动任务图",
        ]
        .iter()
        .any(|value| body.contains(value));
    let pause = [
        "用户请求暂停任务",
        "进入失败暂停状态",
        "因阻塞告警已暂停",
        "检测到暂停请求",
    ]
    .iter()
    .any(|value| body.contains(value));
    let resume = manual
        || (control
            && ["恢复任务", "恢复执行"]
                .iter()
                .any(|value| body.contains(value)));
    // 产品气密检测结果不等同执行故障；仅收集级别、执行器失败及非零执行结果码。
    let execution = body.contains("BehaviorTree")
        || body.contains("ROS2ActionNode")
        || body.contains("TaskGraphExecutor")
        || body.contains("arm_move response:")
        || body.contains("gripper done")
        || body.contains("action result");
    let error = severe
        || execution
            && (nonzero_code(body)
                || [
                    "已发布任务日志 [ERROR]",
                    "已发布任务日志 [FATAL]",
                    "已发布任务日志 [CRITICAL]",
                    "执行失败",
                    "结果: FAILURE",
                    "结果：FAILURE",
                    "status=failed",
                    "status=failure",
                    "result=FAILURE",
                    "执行结果: FAILED",
                ]
                .iter()
                .any(|value| body.contains(value)));
    let event = Event {
        thread: line
            .split('[')
            .filter_map(|part| part.split_once(']').map(|p| p.0))
            .find(|part| part.starts_with("0x"))
            .unwrap_or("")
            .to_string(),
        anchor: line.contains(ANCHOR),
        success,
        pause,
        resume,
        error,
        manual,
        retry_anchor: body.contains("重试节点 normal_nav_pick")
            || body.contains("跳过节点 normal_nav_pick"),
        wait_start: line.contains(WAIT_START),
        wait_end: line.contains(WAIT_END),
        ..Default::default()
    };
    if event.anchor
        || event.success.is_some()
        || pause
        || resume
        || error
        || manual
        || event.wait_start
        || event.wait_end
    {
        Some(Event {
            evidence: Some(Evidence {
                timestamp_us: ts,
                line: number,
                text: line.into(),
            }),
            ..event
        })
    } else {
        None
    }
}

/// 相同源身份只选择一个完整采集副本；内容冲突拒绝分析，不能以文件名去重后静默丢数据。
type SourceCopies = Vec<(PathBuf, String, Vec<u8>)>;

fn read_sources(inputs: &[PathBuf]) -> Result<(SourceCopies, Vec<String>), String> {
    let mut sources: Vec<(PathBuf, String, Vec<u8>)> = Vec::new();
    let mut duplicates = Vec::new();
    for path in inputs {
        let mut bytes = fs::read(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let header = String::from_utf8_lossy(&bytes[..complete.min(2048)]);
        let name = header
            .lines()
            .find_map(|line| line.strip_prefix("=== Log file: "));
        let started = header
            .lines()
            .find(|line| line.starts_with("=== SPR Logger Started"));
        let identity = match (name, started) {
            (Some(name), Some(started)) => format!("{name} | {started}"),
            _ => path
                .canonicalize()
                .unwrap_or_else(|_| path.clone())
                .display()
                .to_string(),
        };
        let existing = sources.iter().position(|(_, id, data)| {
            id == &identity
                || (complete > 0
                    && data.get(..complete) == Some(&bytes[..complete])
                    && data.len() == bytes.len())
        });
        if let Some(index) = existing {
            let (old_path, _, old_data) = &mut sources[index];
            let old_complete = old_data
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i + 1);
            let shared = complete.min(old_complete);
            if old_data[..shared] != bytes[..shared] {
                return Err(format!(
                    "同一源日志的采集副本内容冲突，请只选择可靠副本：{} 与 {}",
                    old_path.display(),
                    path.display()
                ));
            }
            if complete > old_complete {
                duplicates.push(format!(
                    "{} → 使用更完整副本 {}",
                    old_path.display(),
                    path.display()
                ));
                *old_path = path.clone();
                std::mem::swap(old_data, &mut bytes);
            } else {
                duplicates.push(format!(
                    "{} → 已使用 {}",
                    path.display(),
                    old_path.display()
                ));
            }
        } else {
            sources.push((path.clone(), identity, bytes));
        }
    }
    Ok((sources, duplicates))
}

fn union_overlap(intervals: &[Interval], start: i64, end: i64) -> i64 {
    let mut parts: Vec<_> = intervals
        .iter()
        .filter(|i| i.reliable)
        .filter_map(|interval| {
            let a = start.max(interval.start.timestamp_us);
            let b = end.min(interval.observed_end_us);
            (b > a).then_some((a, b))
        })
        .collect();
    parts.sort_unstable();
    let mut cursor = start;
    let mut total = 0;
    for (a, b) in parts {
        total += (b - cursor.max(a)).max(0);
        cursor = cursor.max(b);
    }
    total
}

fn intervals(events: &[Event], end_us: i64) -> (Vec<Interval>, Vec<Interval>) {
    let mut pauses = Vec::new();
    let mut waits = Vec::new();
    let mut paused: Option<Evidence> = None;
    let mut waiting: HashMap<String, (Evidence, bool)> = HashMap::new();
    for event in events {
        let evidence = event.evidence();
        if event.pause && paused.is_none() {
            paused = Some(evidence.clone());
        }
        if event.resume {
            if let Some(start) = paused.take() {
                pauses.push(Interval {
                    start,
                    end: Some(evidence.clone()),
                    observed_end_us: evidence.timestamp_us,
                    reliable: true,
                    note: String::new(),
                });
            }
            for (_, (start, _)) in waiting.drain() {
                waits.push(Interval {
                    start,
                    end: Some(evidence.clone()),
                    observed_end_us: evidence.timestamp_us,
                    reliable: false,
                    note: "等待被重试、取消或任务控制打断，未扣除".into(),
                });
            }
        }
        if event.wait_start {
            waiting
                .entry(event.thread.clone())
                .and_modify(|(_, ambiguous)| *ambiguous = true)
                .or_insert((evidence.clone(), false));
        }
        if event.wait_end {
            match waiting.remove(&event.thread) {
                Some((start, ambiguous)) => waits.push(Interval {
                    start,
                    end: Some(evidence.clone()),
                    observed_end_us: evidence.timestamp_us,
                    reliable: !ambiguous,
                    note: match ambiguous {
                        true => "同线程重复等待开始，配对有歧义，未扣除".into(),
                        false => String::new(),
                    },
                }),
                None => waits.push(Interval {
                    start: evidence.clone(),
                    end: None,
                    observed_end_us: evidence.timestamp_us,
                    reliable: false,
                    note: "等待结束缺少同线程开始证据，未扣除".into(),
                }),
            }
        }
    }
    if let Some(start) = paused {
        pauses.push(Interval {
            start,
            end: None,
            observed_end_us: end_us,
            reliable: false,
            note: "暂停未闭合，仅标注本进程已观测范围".into(),
        });
    }
    for (_, (start, _)) in waiting {
        waits.push(Interval {
            start,
            end: None,
            observed_end_us: end_us,
            reliable: false,
            note: "等待未闭合或线程无法配对，未扣除".into(),
        });
    }
    (pauses, waits)
}

fn process_cycles(
    events: &mut [Event],
    end_us: i64,
    process: &str,
    source: &std::path::Path,
    options: &Options,
    statistics: StatisticsOptions,
) -> Vec<Cycle> {
    events.sort_by_key(|event| (event.evidence().timestamp_us, event.evidence().line));
    let (pauses, waits) = intervals(events, end_us);
    let anchors: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(i, event)| event.anchor.then_some(i))
        .collect();
    let mut cycles = Vec::new();
    for (number, &index) in anchors.iter().enumerate() {
        let start = events[index].evidence();
        if !options.date.is_empty() && !beijing(start.timestamp_us).starts_with(&options.date) {
            continue;
        }
        let next = anchors.get(number + 1).copied();
        let end = next.map(|i| events[i].evidence().clone());
        let upper = end.as_ref().map_or(end_us, |e| e.timestamp_us);
        // 前闭后开按时间定义，同一时间戳的记录统一归下一轮。
        let from = events.partition_point(|e| e.evidence().timestamp_us < start.timestamp_us);
        let to = match end {
            Some(_) => events.partition_point(|e| e.evidence().timestamp_us < upper),
            None => events.len(),
        };
        let slice = &events[from..to.max(from)];
        let successes: Vec<_> = slice.iter().filter(|e| e.success.is_some()).collect();
        let mut structural_reasons = Vec::new();
        if end.is_none() {
            structural_reasons.push("周期未闭合".into());
        }
        if upper <= start.timestamp_us {
            structural_reasons.push("周期时间边界无效".into());
        }
        if successes
            .iter()
            .map(|e| e.success.unwrap().0)
            .collect::<Vec<_>>()
            != [0, 1, 2]
        {
            structural_reasons.push("必要成功节点缺失、重复或顺序异常".into());
        }
        let station = successes.iter().find_map(|e| {
            e.success
                .filter(|(stage, _)| *stage == 1)
                .map(|(_, station)| station)
        });
        let errors: Vec<_> = slice
            .iter()
            .filter(|e| e.error)
            .map(|e| e.evidence().clone())
            .collect();
        let mut interventions: Vec<_> = slice
            .iter()
            .filter(|e| e.manual)
            .map(|e| e.evidence().clone())
            .collect();
        // 取件导航重试控制先于创建日志，沿着相邻锚点区间关联恢复轮，不使用任意秒数阈值。
        let previous = match number.checked_sub(1) {
            Some(i) => anchors[i] + 1,
            None => 0,
        };
        interventions.extend(
            events[previous..index]
                .iter()
                .filter(|e| e.retry_anchor)
                .map(|e| e.evidence().clone()),
        );
        let cycle_pauses: Vec<_> = pauses
            .iter()
            .filter(|p| p.overlaps(start.timestamp_us, upper))
            .cloned()
            .collect();
        let cycle_waits: Vec<_> = waits
            .iter()
            .filter(|p| p.overlaps(start.timestamp_us, upper))
            .cloned()
            .collect();
        let raw_us = end
            .as_ref()
            .map(|end| end.timestamp_us - start.timestamp_us);
        let wait_us = match options.deduct_wait {
            true => union_overlap(&cycle_waits, start.timestamp_us, upper),
            false => 0,
        };
        let mut stages_us = Vec::new();
        let mut stage_wait_us = Vec::new();
        if structural_reasons.is_empty() {
            let bounds = [
                start.timestamp_us,
                successes[0].evidence().timestamp_us,
                successes[1].evidence().timestamp_us,
                successes[2].evidence().timestamp_us,
                upper,
            ];
            for pair in bounds.windows(2) {
                let deducted = match options.deduct_wait {
                    true => union_overlap(&cycle_waits, pair[0], pair[1]),
                    false => 0,
                };
                stages_us.push(pair[1] - pair[0] - deducted);
                stage_wait_us.push(deducted);
            }
        }
        let mut reasons = structural_reasons.clone();
        if !statistics.include_paused && !cycle_pauses.is_empty() {
            reasons.push("暂停影响".into());
        }
        if !statistics.include_errors && !errors.is_empty() {
            reasons.push("执行错误".into());
        }
        if !options.include_interventions && !interventions.is_empty() {
            reasons.push("人工干预或任务切换".into());
        }
        cycles.push(Cycle {
            id: format!("{process}-{:04}", number + 1),
            process: process.into(),
            source: source.into(),
            start: start.clone(),
            end,
            station,
            success: successes.iter().map(|e| e.evidence().clone()).collect(),
            raw_us,
            wait_us,
            net_us: raw_us.map(|raw| raw - wait_us),
            stages_us,
            stage_wait_us,
            pauses: cycle_pauses,
            waits: cycle_waits,
            errors,
            interventions,
            included: reasons.is_empty(),
            structural_reasons,
            reasons,
        });
    }
    cycles
}

fn calculate(request: &AnalysisRequest, options: &Options) -> Result<Dataset, String> {
    if !options.date.is_empty()
        && (options.date.len() != 10
            || NaiveDate::parse_from_str(&options.date, "%Y-%m-%d").is_err())
    {
        return Err("日期请填写 YYYY-MM-DD，或留空分析全部日期（按北京时间）".into());
    }
    let (sources, duplicates) = read_sources(&request.inputs)?;
    let mut dataset = Dataset {
        schema_version: 1,
        timezone: "Asia/Shanghai (UTC+08:00)".into(),
        options: options.clone(),
        statistics: request.statistics,
        sources: Vec::new(),
        duplicates,
        cycles: Vec::new(),
    };
    for (file_index, (source, identity, bytes)) in sources.into_iter().enumerate() {
        let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let text = std::str::from_utf8(&bytes[..complete])
            .map_err(|e| format!("{} 不是有效 UTF-8 日志：{e}", source.display()))?;
        let mut audit = SourceAudit {
            source: source.clone(),
            identity,
            first_us: None,
            last_us: None,
            processes: 0,
            ignored_tail: complete < bytes.len(),
            ignored_lines: 0,
            warnings: Vec::new(),
        };
        let mut events = Vec::new();
        let mut process_end = None;
        let mut part = 1;
        let mut other_waits = BTreeMap::new();
        for (number, line) in text.lines().enumerate() {
            if line.starts_with("=== SPR Logger Started") && process_end.is_some() {
                dataset.cycles.extend(process_cycles(
                    &mut events,
                    process_end.take().unwrap(),
                    &format!("P{}.{part}", file_index + 1),
                    &source,
                    options,
                    request.statistics,
                ));
                events.clear();
                part += 1;
            }
            if let Some(ts) = timestamp(line) {
                audit.first_us = Some(audit.first_us.map_or(ts, |old| old.min(ts)));
                audit.last_us = Some(audit.last_us.map_or(ts, |old| old.max(ts)));
                process_end = Some(process_end.map_or(ts, |old: i64| old.max(ts)));
                if let Some(event) = classify(line, number + 1, ts) {
                    events.push(event);
                }
                if line.contains("开始执行")
                    && !line.contains(WAIT_START)
                    && let Some((_, tail)) = line.split_once("SequenceNode[")
                    && let Some((node, _)) = tail.split_once(']')
                    && node.contains("wait")
                    && node.contains("box")
                {
                    other_waits.entry(node.to_string()).or_insert(number + 1);
                }
            } else if !line.starts_with("===") && !line.trim().is_empty() {
                audit.ignored_lines += 1;
            }
        }
        if let Some(end) = process_end {
            dataset.cycles.extend(process_cycles(
                &mut events,
                end,
                &format!("P{}.{part}", file_index + 1),
                &source,
                options,
                request.statistics,
            ));
            audit.processes = part;
        }
        for (node, line) in other_waits {
            audit.warnings.push(format!(
                "第 {line} 行发现其他搬框等待候选 {node}；没有已配置的可靠闭合规则，未自动扣除"
            ));
        }
        dataset.sources.push(audit);
    }
    dataset
        .cycles
        .sort_by_key(|cycle| (cycle.start.timestamp_us, cycle.id.clone()));
    // 发布前验证关键不变量；不允许用修剪负值掩盖时段计算错误。
    for cycle in &dataset.cycles {
        if cycle.structural_reasons.is_empty()
            && (Some(cycle.stages_us.iter().sum::<i64>()) != cycle.net_us
                || cycle.stage_wait_us.iter().sum::<i64>() != cycle.wait_us
                || cycle.stages_us.iter().any(|v| *v < 0))
        {
            return Err(format!("周期 {} 阶段加总校验失败，停止生成统计", cycle.id));
        }
    }
    Ok(dataset)
}

pub(super) fn analyze(
    request: &AnalysisRequest,
    options: &Options,
) -> Result<AnalysisReport, String> {
    let dataset = calculate(request, options)?;
    let output = request.output.join("workpiece");
    fs::create_dir(&output).map_err(|e| format!("创建工件节拍报告目录失败：{e}"))?;
    let (summary, files) = report::write(&dataset, &output)?;
    Ok(AnalysisReport {
        statistics: request.statistics,
        output: request.output.clone(),
        merged: None,
        warnings: dataset
            .sources
            .iter()
            .flat_map(|s| s.warnings.clone())
            .chain(dataset.duplicates.clone())
            .collect(),
        reports: vec![FileReport {
            input: request.inputs[0].clone(),
            summary,
            duration_label: if options.deduct_wait {
                "净周期（扣除搬框等待）"
            } else {
                "原始周期（未扣除搬框等待）"
            }
            .into(),
            rounds: dataset
                .cycles
                .iter()
                .map(|c| super::RoundOverview {
                    id: c.id.clone(),
                    label: c
                        .station
                        .map(|s| format!("工位 {s}"))
                        .unwrap_or_else(|| "工位未知".into()),
                    start_us: c.start.timestamp_us,
                    end_us: c.end.as_ref().map(|e| e.timestamp_us),
                    raw_seconds: c.raw_us.map(|v| v as f64 / 1_000_000.0),
                    deducted_seconds: c.wait_us as f64 / 1_000_000.0,
                    duration_seconds: c.net_us.map(|v| v as f64 / 1_000_000.0),
                    included: c.included,
                    notes: format!(
                        "{} · {}:{}",
                        c.reasons.join("、"),
                        c.source.display(),
                        c.start.line
                    ),
                    detail: None,
                })
                .collect(),
            files,
            error: None,
        }],
    })
}
