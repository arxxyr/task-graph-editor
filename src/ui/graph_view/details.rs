//! 详情全文查看与复制：只更新文字，保留滚动容器、按钮和焦点。

use std::sync::Arc;

use bevy::clipboard::{Clipboard, ClipboardError};
use bevy::feathers::controls::{ButtonVariant, FeathersButton};
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemedText};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui_widgets::{Activate, ScrollArea};
use serde_json::Value;

use super::super::{UiSet, theme, widgets};
use super::{DETAIL_W, DrillButton, TaskNode, category_token};
use widgets::{BoxedScene, boxed};

const SUMMARY_CHARS: usize = 300;

/// 全文与摘要共享一个文本实体；展开、收起不重建详情树。
#[derive(Component, Clone, Default)]
struct TextSection {
    full: Arc<str>,
    summary: String,
    expanded: bool,
}

impl TextSection {
    fn displayed(&self) -> &str {
        match self.expanded {
            true => &self.full,
            false => &self.summary,
        }
    }

    fn toggle_label(&self) -> &'static str {
        match self.expanded {
            true => "收起全文",
            false => "展开全文",
        }
    }
}

#[derive(Component, Clone, Copy, Default)]
enum SectionText {
    #[default]
    Value,
    ToggleLabel,
}

#[derive(Component, Clone, Default)]
struct ToggleText;

#[derive(Component, Clone, Default)]
struct CopyText {
    full: Arc<str>,
    feedback: CopyFeedback,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum CopyFeedback {
    #[default]
    Ready,
    Copied,
    Failed,
}

impl CopyFeedback {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "复制全文",
            Self::Copied => "已复制",
            Self::Failed => "复制失败 · 重试",
        }
    }
}

impl CopyText {
    fn copy_with(&mut self, write: impl FnOnce(&str) -> Result<(), ClipboardError>) {
        // 空字符串也是合法参数，复制时应准确清空剪贴板。
        self.feedback = match write(&self.full) {
            Ok(()) => CopyFeedback::Copied,
            Err(error) => {
                warn!(%error, "复制流程图详情失败");
                CopyFeedback::Failed
            }
        };
    }
}

#[derive(Component, Clone, Default)]
struct CopyLabel;

/// 标签在上、内容在下；长内容先显示摘要，复制始终使用完整原文。
pub(super) fn text_section(key: impl Into<String>, text: impl Into<String>) -> impl Scene {
    section(key.into(), text.into(), false)
}

fn section(key: String, text: String, collapsed: bool) -> impl Scene {
    let summary = match collapsed {
        true => format!("完整 JSON · {} 字符", text.chars().count()),
        false => summarize(&text),
    };
    let expandable = collapsed || summary != text;
    let full: Arc<str> = text.into();
    let state = TextSection {
        full: Arc::clone(&full),
        summary,
        expanded: false,
    };
    let mut controls: Vec<BoxedScene> = Vec::new();
    if expandable {
        controls.push(boxed(bsn! {
            @FeathersButton {
                @variant: ButtonVariant::Plain,
                @caption: {bsn! {
                    Text("展开全文")
                    template_value(SectionText::ToggleLabel)
                    ThemedText
                }}
            }
            ToggleText
            AccessibleLabel("展开或收起完整内容")
        }));
    }
    controls.push(boxed(bsn! {
        @FeathersButton {
            @variant: ButtonVariant::Plain,
            @caption: {bsn! { Text("复制全文") CopyLabel ThemedText }}
        }
        template_value(CopyText { full, feedback: CopyFeedback::Ready })
        AccessibleLabel("复制完整原文")
    }));
    let initial = state.displayed().to_string();
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            min_width: px(0),
            flex_shrink: 0.0,
            row_gap: px(3),
            padding: {UiRect::vertical(px(3.0))},
        }
        template_value(state)
        Children [
            (
                Text(key)
                ThemeTextColor({theme::FIELD_LABEL})
                TextFont { font_size: px(10.5) }
                TextLayout { linebreak: {LineBreak::AnyCharacter} }
            ),
            (
                Text(initial)
                template_value(SectionText::Value)
                ThemeTextColor({theme::READONLY_TEXT})
                TextFont { font_size: px(11.5) }
                TextLayout { linebreak: {LineBreak::AnyCharacter} }
            ),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(4),
                    row_gap: px(3),
                }
                Children [{controls}]
            )
        ]
    }
}

fn summarize(text: &str) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(SUMMARY_CHARS).collect();
    match chars.next() {
        None => prefix,
        Some(_) => prefix
            .chars()
            .take(SUMMARY_CHARS - 1)
            .chain(['…'])
            .collect(),
    }
}

fn formatted_json(value: &Value) -> String {
    // Value 不含不可序列化数据；保留兜底，避免展示辅助功能造成崩溃。
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn input_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => formatted_json(value),
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "布尔",
        Value::Number(_) => "数值",
        Value::String(_) => "字符串",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}

fn input_sections(node: &TaskNode, raw_node: Option<&Value>) -> Vec<BoxedScene> {
    let inputs = match raw_node {
        Some(raw) => raw.get("inputs"),
        None => Some(&node.inputs),
    };
    match inputs {
        None => vec![boxed(widgets::hint("该节点未定义 inputs"))],
        Some(Value::Object(map)) if map.is_empty() => {
            vec![boxed(text_section("inputs · 空对象", "{}"))]
        }
        Some(Value::Object(map)) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            entries
                .into_iter()
                .map(|(key, value)| boxed(text_section(key, input_text(value))))
                .collect()
        }
        Some(value) => vec![boxed(text_section(
            format!("inputs · {}", json_type(value)),
            input_text(value),
        ))],
    }
}

/// 右侧详情同时呈现可读参数与完整原始节点，未知扩展字段仍可核验和复制。
pub(super) fn detail_panel(node: &TaskNode, raw_node: Option<&Value>) -> impl Scene {
    let mut body: Vec<BoxedScene> = vec![
        boxed(bsn! {
            Text({node.id.clone()})
            ThemeTextColor({theme::SECTION_TEXT})
            TextFont { font_size: px(12.0), weight: FontWeight::BOLD }
            TextLayout { linebreak: {LineBreak::AnyCharacter} }
        }),
        boxed(widgets::row(
            6.0,
            vec![
                boxed(bsn! {
                    Node {
                        width: px(9),
                        height: px(9),
                        border_radius: {BorderRadius::all(px(2.0))},
                        flex_shrink: 0.0,
                    }
                    ThemeBackgroundColor({category_token(&node.node_type)})
                }),
                boxed(widgets::readonly_value(node.node_type.clone())),
            ],
        )),
    ];
    if node.checkpoint {
        body.push(boxed(widgets::badge("checkpoint")));
    }
    body.push(boxed(bsn! {
        Node {
            width: percent(100),
            height: px(1),
            margin: {UiRect::vertical(px(4.0))},
            flex_shrink: 0.0,
        }
        ThemeBackgroundColor({theme::DIVIDER})
    }));
    body.extend(input_sections(node, raw_node));
    if let Some(children) = &node.children {
        body.push(boxed(widgets::button(
            format!("进入子图 · {} 节点", children.nodes.len()),
            ButtonVariant::Primary,
            DrillButton(node.id.clone()),
        )));
    }
    match raw_node {
        Some(raw) => body.push(boxed(section(
            "原始节点 JSON".into(),
            formatted_json(raw),
            true,
        ))),
        None => body.push(boxed(widgets::hint("原始节点 JSON 不可用"))),
    }
    bsn! {
        Node {
            width: {px(DETAIL_W)},
            flex_shrink: 0.0,
            height: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(6),
            padding: {UiRect::all(px(theme::PAD))},
            border: {UiRect::left(px(1.0))},
            overflow: {Overflow::scroll_y()},
        }
        ScrollArea
        ThemeBackgroundColor({theme::SIDEBAR_BG})
        ThemeBorderColor({theme::DIVIDER})
        Children [{body}]
    }
}

fn on_toggle(
    event: On<Activate>,
    toggles: Query<(), With<ToggleText>>,
    parents: Query<&ChildOf>,
    mut sections: Query<&mut TextSection>,
) {
    let Some(button) = widgets::self_or_ancestor(event.entity, &parents, |e| toggles.contains(e))
    else {
        return;
    };
    let Some(section) = widgets::self_or_ancestor(button, &parents, |e| sections.contains(e))
    else {
        return;
    };
    if let Ok(mut section) = sections.get_mut(section) {
        section.expanded = !section.expanded;
    }
}

fn on_copy(
    event: On<Activate>,
    parents: Query<&ChildOf>,
    mut buttons: Query<&mut CopyText>,
    mut clipboard: Option<ResMut<Clipboard>>,
) {
    let Some(button) = widgets::self_or_ancestor(event.entity, &parents, |e| buttons.contains(e))
    else {
        return;
    };
    if let Ok(mut button) = buttons.get_mut(button) {
        button.copy_with(|text| match clipboard.as_deref_mut() {
            Some(clipboard) => clipboard.set_text(text),
            None => Err(ClipboardError::ClipboardNotSupported),
        });
    }
}

fn sync_section_text(
    parents: Query<&ChildOf>,
    sections: Query<Ref<TextSection>>,
    mut texts: Query<(Entity, Ref<SectionText>, &mut Text)>,
) {
    for (entity, role, mut text) in &mut texts {
        let Some(section) = widgets::self_or_ancestor(entity, &parents, |e| sections.contains(e))
        else {
            continue;
        };
        let Ok(section) = sections.get(section) else {
            continue;
        };
        // BSN 文本可能迟于状态变化生成，新增文字仍需按当前状态初始化。
        if !section.is_changed() && !role.is_added() && !text.is_added() {
            continue;
        }
        let expected = match *role {
            SectionText::Value => section.displayed(),
            SectionText::ToggleLabel => section.toggle_label(),
        };
        if text.0 != expected {
            expected.clone_into(&mut text.0);
        }
    }
}

fn sync_copy_labels(
    parents: Query<&ChildOf>,
    buttons: Query<Ref<CopyText>>,
    mut labels: Query<(Entity, &mut Text), With<CopyLabel>>,
) {
    for (entity, mut text) in &mut labels {
        let Some(button) = widgets::self_or_ancestor(entity, &parents, |e| buttons.contains(e))
        else {
            continue;
        };
        let Ok(button) = buttons.get(button) else {
            continue;
        };
        if !button.is_changed() && !text.is_added() {
            continue;
        }
        let expected = button.feedback.label();
        if text.0 != expected {
            expected.clone_into(&mut text.0);
        }
    }
}

pub(super) fn register(app: &mut App) {
    app.add_observer(on_toggle)
        .add_observer(on_copy)
        .add_systems(
            Update,
            (sync_section_text, sync_copy_labels).in_set(UiSet::Rebuild),
        );
}

#[cfg(test)]
mod tests;
