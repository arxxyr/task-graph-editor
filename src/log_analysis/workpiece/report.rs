//! 同一份逐轮数据驱动汇总、交互图和证据导出，避免各页面统计口径漂移。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{Cycle, Dataset, beijing};
use serde::Serialize;

#[derive(Serialize)]
struct Summary {
    count: usize,
    mean_us: Option<f64>,
    median_us: Option<f64>,
    min_us: Option<i64>,
    max_us: Option<i64>,
    min_ids: Vec<String>,
    max_ids: Vec<String>,
    excluded: usize,
    reasons: BTreeMap<String, usize>,
    waiting_count: usize,
    deducted_us: i64,
    uncertain_wait_count: usize,
}
fn summarize(cycles: &[&Cycle]) -> Summary {
    let included: Vec<_> = cycles.iter().copied().filter(|c| c.included).collect();
    let mut values: Vec<_> = included.iter().filter_map(|c| c.net_us).collect();
    values.sort_unstable();
    let count = values.len();
    let median = match count {
        0 => None,
        n if n % 2 == 0 => Some(values[n / 2 - 1] as f64 / 2.0 + values[n / 2] as f64 / 2.0),
        n => Some(values[n / 2] as f64),
    };
    let mut reasons = BTreeMap::new();
    for cycle in cycles {
        for reason in &cycle.reasons {
            *reasons.entry(reason.clone()).or_default() += 1;
        }
    }
    Summary {
        count,
        mean_us: (count > 0)
            .then(|| values.iter().map(|v| *v as i128).sum::<i128>() as f64 / count as f64),
        median_us: median,
        min_us: values.first().copied(),
        max_us: values.last().copied(),
        min_ids: included
            .iter()
            .filter(|c| c.net_us == values.first().copied())
            .map(|c| c.id.clone())
            .collect(),
        max_ids: included
            .iter()
            .filter(|c| c.net_us == values.last().copied())
            .map(|c| c.id.clone())
            .collect(),
        excluded: cycles.len() - count,
        reasons,
        waiting_count: included
            .iter()
            .filter(|c| {
                c.waits
                    .iter()
                    .any(|w| w.reliable && w.observed_end_us > w.start.timestamp_us)
            })
            .count(),
        deducted_us: included.iter().map(|c| c.wait_us).sum(),
        uncertain_wait_count: included
            .iter()
            .filter(|c| c.waits.iter().any(|w| !w.reliable))
            .count(),
    }
}
fn seconds(value: Option<f64>) -> String {
    value
        .map(|v| format!("{:.6}", v / 1_000_000.0))
        .unwrap_or_else(|| "—".into())
}
fn json<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| e.to_string())
}
fn script_safe(value: &str) -> String {
    value
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

pub(super) fn write(dataset: &Dataset, output: &Path) -> Result<(String, Vec<PathBuf>), String> {
    let all: Vec<_> = dataset.cycles.iter().collect();
    let total = summarize(&all);
    let mut groups: BTreeMap<String, Vec<&Cycle>> = BTreeMap::new();
    for cycle in &dataset.cycles {
        groups
            .entry(format!(
                "工位{}",
                cycle
                    .station
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "未知".into())
            ))
            .or_default()
            .push(cycle);
        groups
            .entry(format!("{} 时", &beijing(cycle.start.timestamp_us)[..13]))
            .or_default()
            .push(cycle);
    }
    let mut stats = BTreeMap::from([("全部".to_string(), total)]);
    stats.extend(
        groups
            .into_iter()
            .map(|(key, value)| (key, summarize(&value))),
    );
    let total = &stats["全部"];
    let first = dataset
        .sources
        .iter()
        .filter_map(|s| s.first_us)
        .min()
        .map(beijing)
        .unwrap_or_else(|| "无可识别时间".into());
    let last = dataset
        .sources
        .iter()
        .filter_map(|s| s.last_us)
        .max()
        .map(beijing)
        .unwrap_or_else(|| "无可识别时间".into());
    let summary = format!(
        "北京时间日志范围：{first} 至 {last}（各进程独立，仅代表已采集时段）。\n起点日期：{}；有效 {} 轮，排除 {} 轮。\n统计周期平均 {} 秒，最小 {} 秒，最大 {} 秒，中位数 {} 秒；扣除搬框等待 {} 秒。",
        match dataset.options.date.is_empty() {
            true => "全部已采集日期",
            false => &dataset.options.date,
        },
        total.count,
        total.excluded,
        seconds(total.mean_us),
        seconds(total.min_us.map(|v| v as f64)),
        seconds(total.max_us.map(|v| v as f64)),
        seconds(total.median_us),
        seconds(Some(total.deducted_us as f64))
    );
    let mut markdown = format!(
        "# 工件节拍分析\n\n{summary}\n\n暂停纳入：{}；执行错误纳入：{}；人工干预纳入：{}；扣除可靠搬框等待：{}。\n\n这是工件流转节拍，不是按工件 ID 追踪的生命周期或合格品产量。四段时间不代表单一模块独占耗时。排除原因可重叠，不能相加为排除总数。未以时长阈值剔除极值。\n\n",
        dataset.statistics.include_paused,
        dataset.statistics.include_errors,
        dataset.options.include_interventions,
        dataset.options.deduct_wait
    );
    markdown.push_str("| 分组 | 有效数 | 排除数 | 平均秒 | 最小秒 | 最大秒 | 中位秒 | 扣除秒 | 不确定等待轮数 |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    for (key, s) in &stats {
        markdown.push_str(&format!(
            "| {key} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            s.count,
            s.excluded,
            seconds(s.mean_us),
            seconds(s.min_us.map(|v| v as f64)),
            seconds(s.max_us.map(|v| v as f64)),
            seconds(s.median_us),
            seconds(Some(s.deducted_us as f64)),
            s.uncertain_wait_count
        ));
    }
    markdown.push_str("\n## 排除原因\n\n");
    for (reason, count) in &total.reasons {
        markdown.push_str(&format!("- {reason}：{count} 轮\n"));
    }
    markdown.push_str("\n## 数据边界与审计\n\n");
    for source in &dataset.sources {
        markdown.push_str(&format!(
            "- {}：{} → {}，{} 个进程段，忽略末尾半行={}，未解析非空行={}。\n",
            source.source.display(),
            source.first_us.map(beijing).unwrap_or_default(),
            source.last_us.map(beijing).unwrap_or_default(),
            source.processes,
            source.ignored_tail,
            source.ignored_lines
        ));
        for warning in &source.warnings {
            markdown.push_str(&format!("  - {warning}\n"));
        }
    }
    for duplicate in &dataset.duplicates {
        markdown.push_str(&format!("- 去重：{duplicate}\n"));
    }
    markdown.push_str("\n## 极值证据索引\n\n");
    let longest_wait = all
        .iter()
        .filter(|c| c.included)
        .max_by_key(|c| c.wait_us)
        .map(|c| c.id.as_str());
    for cycle in &all {
        if total.min_ids.contains(&cycle.id)
            || total.max_ids.contains(&cycle.id)
            || longest_wait == Some(cycle.id.as_str())
        {
            markdown.push_str(&format!("- {}，工位 {:?}，{} → {}，源 {}，行 {} → {:?}，原始 {:?} 微秒，扣除 {} 微秒，净值 {:?} 微秒。\n", cycle.id, cycle.station, beijing(cycle.start.timestamp_us), cycle.end.as_ref().map(|e| beijing(e.timestamp_us)).unwrap_or_default(), cycle.source.display(), cycle.start.line, cycle.end.as_ref().map(|e| e.line), cycle.raw_us, cycle.wait_us, cycle.net_us));
        }
    }
    let data = json(dataset)?;
    let flow = serde_json::to_string(include_str!("../../../assets/log_analysis/cycle-flow.html"))
        .map_err(|e| e.to_string())?;
    let template = include_str!("../../../assets/log_analysis/report.html");
    let (before, rest) = template
        .split_once("/*DATA*/ null")
        .expect("报告模板必须包含数据插槽");
    let (middle, after) = rest
        .split_once("/*FLOW*/ null")
        .expect("报告模板必须包含流程插槽");
    let html = format!(
        "{before}{}{middle}{}{after}",
        script_safe(&data),
        script_safe(&flow)
    );
    let outputs = [
        ("cycles.json", data),
        ("statistics.json", json(&stats)?),
        ("report.md", markdown),
        ("interactive.html", html),
    ];
    let mut files = Vec::new();
    for (name, content) in outputs {
        let path = output.join(name);
        fs::write(&path, content).map_err(|e| e.to_string())?;
        files.push(path);
    }
    Ok((summary, files))
}
