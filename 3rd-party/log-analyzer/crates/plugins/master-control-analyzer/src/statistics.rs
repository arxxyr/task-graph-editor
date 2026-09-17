//! 常规轮次统计口径：统一筛选图表和统计 CSV，保留逐轮审计依据。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::models::{CycleType, LogLine, NavigationFlow, Round};

/// 默认排除含暂停、执行错误的轮次；勾选后可扩大观察范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatisticsOptions {
    pub include_paused: bool,
    pub include_errors: bool,
}

impl Default for StatisticsOptions {
    fn default() -> Self {
        Self {
            include_paused: false,
            include_errors: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoundDecision {
    pub round_id: usize,
    pub has_pause: bool,
    pub has_error: bool,
    pub included: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatisticsSelection {
    pub options: StatisticsOptions,
    pub included_count: usize,
    pub rounds: Vec<RoundDecision>,
}

// 只匹配日志头部的严重级别，不把业务文本、模板或错误计数中的单词当成错误事件。
fn error_header(line: &str) -> bool {
    line.split('[')
        .filter_map(|part| part.split_once(']').map(|(token, _)| token))
        .find_map(|token| match token {
            "ERROR" | "FATAL" | "CRITICAL" => Some(true),
            "TRACE" | "DEBUG" | "INFO" | "WARN" | "WARNING" => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

fn failed_status(status: &str) -> bool {
    let status = status.to_ascii_lowercase();
    matches!(status.as_str(), "error" | "failure" | "failed" | "aborted")
        || status.starts_with("failed_")
}

/// 输入日志由解析器按时间排序；二分定位每个轮次范围，不对整份日志反复扫描。
/// 相邻轮次的公共时间戳归后一个轮次，避免将同一错误同时归入两个轮次。
pub fn select_rounds(
    rounds: &[Round],
    flows: &[NavigationFlow],
    lines: &[LogLine],
    options: StatisticsOptions,
) -> StatisticsSelection {
    let mut flags: HashMap<usize, (bool, bool)> = HashMap::new();
    for flow in flows {
        let flags = flags.entry(flow.round_id).or_default();
        flags.1 |= failed_status(&flow.nav_status);
        for operation in &flow.operations {
            flags.0 |= !operation.pause_events.is_empty();
            flags.1 |= failed_status(&operation.status);
        }
    }
    let mut ordered: Vec<_> = rounds.iter().collect();
    ordered.sort_by(|a, b| a.start_ts.total_cmp(&b.start_ts));
    let mut decisions = Vec::with_capacity(rounds.len());
    for (index, round) in ordered.iter().enumerate() {
        let (mut has_pause, mut has_error) = flags.get(&round.id).copied().unwrap_or_default();
        has_pause |= !round.pause_events.is_empty();
        let next_start = ordered.get(index + 1).map(|r| r.start_ts);
        let start = lines.partition_point(|line| line.timestamp < round.start_ts);
        for line in &lines[start..] {
            if round.end_ts.is_some_and(|end| line.timestamp > end)
                || next_start.is_some_and(|next| line.timestamp >= next)
            {
                break;
            }
            has_pause |= crate::round_detector::is_pause_marker(&line.line);
            has_error |= error_header(&line.line);
        }
        let mut reasons = Vec::new();
        if !matches!(round.cycle_type, CycleType::Normal(_)) {
            reasons.push("非普通常规轮次".into());
        }
        if round.end_ts.is_none() {
            reasons.push("轮次未完成".into());
        }
        if has_pause && !options.include_paused {
            reasons.push("含暂停".into());
        }
        if has_error && !options.include_errors {
            reasons.push("含错误".into());
        }
        decisions.push(RoundDecision {
            round_id: round.id,
            has_pause,
            has_error,
            included: reasons.is_empty(),
            reasons,
        });
    }
    StatisticsSelection {
        options,
        included_count: decisions.iter().filter(|r| r.included).count(),
        rounds: decisions,
    }
}

impl StatisticsSelection {
    pub fn summary(&self) -> String {
        let label = |value| match value {
            true => "纳入",
            false => "排除",
        };
        format!(
            "常规轮次耗时统计：有暂停={}，有错误={}；纳入 {} 轮。逐轮甘特图和动作明细保留全部轮次。",
            label(self.options.include_paused),
            label(self.options.include_errors),
            self.included_count
        )
    }
}

/// 原生报告和导出图共用的轮次耗时，不在界面重新推算统计口径。
pub fn round_timing(round: &Round, flows: &[NavigationFlow]) -> (f64, f64, f64) {
    let total = round.end_ts.map(|end| end - round.start_ts).unwrap_or(0.0);
    let action_pause: f64 = flows
        .iter()
        .filter(|f| f.round_id == round.id)
        .flat_map(|f| f.operations.iter())
        .map(|op| op.total_pause_duration())
        .sum();
    let pause = round.total_pause_duration().max(action_pause);
    (total, pause, (total - pause).max(0.0))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundOverview {
    pub id: String,
    pub label: String,
    pub start_us: i64,
    pub end_us: Option<i64>,
    pub raw_seconds: Option<f64>,
    pub deducted_seconds: f64,
    pub duration_seconds: Option<f64>,
    pub included: bool,
    pub notes: String,
    pub detail: Option<std::path::PathBuf>,
}

pub fn round_overview(
    rounds: &[Round],
    flows: &[NavigationFlow],
    selection: &StatisticsSelection,
    output: &std::path::Path,
) -> Vec<RoundOverview> {
    let width = crate::utils::digit_width(rounds.len());
    rounds
        .iter()
        .map(|round| {
            let decision = selection.rounds.iter().find(|d| d.round_id == round.id);
            let (raw, deducted, duration) = round_timing(round, flows);
            let mut notes = decision.map(|d| d.reasons.clone()).unwrap_or_default();
            if let Some(d) = decision {
                if d.has_pause && !notes.iter().any(|n| n == "含暂停") {
                    notes.push("含暂停".into());
                }
                if d.has_error && !notes.iter().any(|n| n == "含错误") {
                    notes.push("含错误".into());
                }
            }
            let detail = output.join(format!("round_{:0width$}_gantt.png", round.id));
            RoundOverview {
                id: round.id.to_string(),
                label: round.cycle_type.to_string(),
                start_us: (round.start_ts * 1_000_000.0).round() as i64,
                end_us: round.end_ts.map(|t| (t * 1_000_000.0).round() as i64),
                raw_seconds: round.end_ts.map(|_| raw),
                deducted_seconds: deducted,
                duration_seconds: round.end_ts.map(|_| duration),
                included: decision.is_some_and(|d| d.included),
                notes: notes.join("、"),
                detail: detail.is_file().then_some(detail),
            }
        })
        .collect()
}
