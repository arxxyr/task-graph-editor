//! 报告先呈现逐轮用时，原始产物和分析过程说明按需展开。
use super::*;
use crate::log_analysis::{FileReport, RoundOverview};
use crate::ui::theme;
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeTextColor};

fn seconds(value: Option<f64>) -> String {
    value
        .map(|v| format!("{:.3} s", if v == 0.0 { 0.0 } else { v }))
        .unwrap_or_else(|| "—".into())
}

fn time(value: i64) -> String {
    chrono::DateTime::from_timestamp_micros(value)
        .map(|t| {
            t.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
                .format("%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "时间无效".into())
}

fn cell(text: impl Into<String>, width: f32) -> impl Scene {
    bsn! {
        Node { width: px(width), flex_shrink: 0.0, padding: px(4) }
        Children [report_text(text)]
    }
}

fn metric(label: &str, value: String) -> impl Scene {
    bsn! {
        Node { flex_direction: FlexDirection::Column, row_gap: px(6), flex_grow: 1.0, min_width: px(120), padding: px(12) }
        Children [
            widgets::hint(label),
            (Text(value) ThemedText TextFont { font_size: px(21) }),
        ]
    }
}

/// 所有统计只使用本次分析明确纳入的闭合轮次，翻页不改变总体口径。
fn durations(item: &FileReport) -> Vec<f64> {
    item.rounds
        .iter()
        .filter(|r| r.included)
        .filter_map(|r| r.duration_seconds)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .collect()
}

fn row(round: &RoundOverview, max: f64) -> impl Scene {
    let token = if round.included {
        theme::DOT_CONNECTED
    } else {
        theme::READONLY_TEXT
    };
    let width = round
        .duration_seconds
        .map(|v| {
            if max > 0.0 {
                (v / max * 100.0).clamp(0.0, 100.0) as f32
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);
    let state = if round.included { "纳入" } else { "排除" };
    let mut detail = vec![boxed(report_text(format!(
        "{} → {}（北京时间）\n{}{}",
        time(round.start_us),
        round.end_us.map(time).unwrap_or_else(|| "未闭合".into()),
        if round.notes.is_empty() {
            "无暂停或错误标记"
        } else {
            &round.notes
        },
        if round.included {
            " · 参与本次统计"
        } else {
            " · 不参与本次统计"
        }
    )))];
    if let Some(path) = &round.detail {
        detail.push(boxed(action_button(
            "查看本轮甘特图",
            LogAction::Open(path.clone()),
        )));
    }
    bsn! {
        Node { flex_direction: FlexDirection::Column, flex_shrink: 0.0, row_gap: px(2), padding: {UiRect::vertical(px(3))} }
        widgets::Collapsible { open: false }
        Children [
            (Node { align_items: AlignItems::Center, min_width: px(700) } widgets::CollapseHeader Children [
                (Node { width: px(150), flex_shrink: 0.0, align_items: AlignItems::Center, column_gap: px(4) } Children [
                    (Text("▸") widgets::CollapseChevron ThemedText TextFont { font_size: px(10) }),
                    report_text(format!("#{}  {}", round.id, round.label)),
                ]),
                cell(time(round.start_us), 120.0),
                cell(seconds(round.raw_seconds), 90.0),
                cell(seconds(Some(round.deducted_seconds)), 90.0),
                (Node { flex_grow: 1.0, min_width: px(140), padding: px(4), flex_direction: FlexDirection::Column, row_gap: px(4) } Children [
                    (Text({seconds(round.duration_seconds)}) ThemedText TextFont { font_size: px(16) }),
                    (Node { height: px(4), width: percent(width), border_radius: px(2) } ThemeBackgroundColor({token.clone()})),
                ]),
                (Node { width: px(95), flex_shrink: 0.0 } Children [
                    (Text(state) ThemeTextColor(token) TextFont { font_size: px(12) }),
                ]),
            ]),
            (Node { display: Display::None, flex_direction: FlexDirection::Column, padding: px(10), row_gap: px(6) }
                widgets::CollapseBody Children [{detail}]),
        ]
    }
}

pub(super) fn content(logs: &Logs, report: &AnalysisReport) -> Vec<BoxedScene> {
    let Some(item) = report.reports.get(logs.report_index) else {
        return Vec::new();
    };
    let values = durations(item);
    let average = (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64);
    let fastest = values.iter().copied().reduce(f64::min);
    let slowest = values.iter().copied().reduce(f64::max);
    let max = item
        .rounds
        .iter()
        .filter_map(|r| r.duration_seconds)
        .fold(0.0_f64, f64::max);
    let mut result = vec![boxed(widgets::subheading("分析结果"))];
    result.push(boxed(bsn! {
        Node { align_items: AlignItems::Center, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(6) }
        Children [
            widgets::hint(format!("日志 {} / {} · {}", logs.report_index + 1, report.reports.len(), item.input.file_name().unwrap_or_default().to_string_lossy())),
            widgets::spacer(),
            action_button("上一份日志", LogAction::ReportLog(false)),
            action_button("下一份日志", LogAction::ReportLog(true)),
        ]
    }));
    if let Some(error) = &item.error {
        result.push(boxed(report_text(format!("分析失败：{error}"))));
    }
    if !item.rounds.is_empty() {
        result.push(boxed(bsn! {
            Node { flex_wrap: FlexWrap::Wrap, column_gap: px(12), flex_shrink: 0.0 }
            Children [
                metric("纳入 / 全部轮次", format!("{} / {}", values.len(), item.rounds.len())),
                metric("平均耗时", seconds(average)),
                metric("最快", seconds(fastest)),
                metric("最慢", seconds(slowest)),
            ]
        }));
        result.push(boxed(widgets::hint(format!(
            "{} · 灰色为排除轮次；条形按本份日志统一比例显示，分页不改变统计。",
            item.duration_label
        ))));
        let rows: Vec<_> = item
            .rounds
            .iter()
            .skip(logs.report_page * PAGE_SIZE)
            .take(PAGE_SIZE)
            .map(|r| boxed(row(r, max)))
            .collect();
        result.push(boxed(bsn! {
            Node { flex_direction: FlexDirection::Column, overflow: {Overflow::scroll_x()}, flex_shrink: 0.0 }
            ScrollArea
            Children [
                (Node { min_width: px(700) } Children [
                    cell("轮次 / 类型", 150.0), cell("开始（北京时间）", 120.0),
                    cell("原始耗时", 90.0), cell("扣除量", 90.0),
                    (Node { flex_grow: 1.0, min_width: px(140) } Children [widgets::hint("统计耗时")]),
                    cell("统计状态", 95.0),
                ]),
                (Node { flex_direction: FlexDirection::Column, max_height: px(420), min_width: px(700), overflow: {Overflow::scroll_y()} } ScrollArea Children [{rows}]),
            ]
        }));
        result.push(boxed(bsn! {
            Node { column_gap: px(8), align_items: AlignItems::Center }
            Children [
                widgets::hint(format!("逐轮明细 · 第 {} / {} 页", logs.report_page + 1, item.rounds.len().div_ceil(PAGE_SIZE).max(1))),
                widgets::spacer(), action_button("上一页", LogAction::ReportPage(false)), action_button("下一页", LogAction::ReportPage(true)),
            ]
        }));
    } else if item.error.is_none() {
        result.push(boxed(widgets::hint(
            "未获得逐轮概览，可展开原始报告；旧版报告需重新分析生成。",
        )));
    }
    let mut files = vec![boxed(report_text(item.summary.clone()))];
    for path in &item.files {
        // 每轮图片已经放在轮次详情，避免再次铺开几十个重复入口。
        if path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .ends_with("_gantt.png")
        {
            continue;
        }
        files.push(boxed(action_button(
            path.file_name().unwrap_or_default().to_string_lossy(),
            LogAction::Open(path.clone()),
        )));
    }
    if let Some(path) = &report.merged {
        files.push(boxed(action_button(
            "合并时间线",
            LogAction::Open(path.clone()),
        )));
    }
    result.push(boxed(widgets::collapsible(
        "原始报告与导出文件",
        None,
        false,
        files,
    )));
    result
}
