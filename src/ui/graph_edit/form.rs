//! 编辑表单按草稿代次重建，文字输入与错误反馈只更新已有控件。

use bevy::feathers::controls::{FeathersTextInput, FeathersTextInputContainer};
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor};
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::ScrollArea;

use super::*;
use crate::ui::connect::ActionButton;
use crate::ui::graph_view::{GraphSceneVersion, stale_scene};
use crate::ui::theme;
use crate::ui::widgets::{BoxedScene, boxed};

#[derive(Component)]
pub(super) struct RenderedToolbar(u64, bool);

#[derive(Component)]
pub(super) struct RenderedPanel(u64);

#[derive(Component, Clone, Default)]
pub(super) struct DraftFeedback;

#[derive(Component, Clone, Default)]
pub(super) struct NodeDescription;

#[derive(Component, Clone, Default)]
pub(super) struct EntryDescription;

#[derive(Component, Clone, Default)]
pub(super) struct SubgraphControls(NodeKey);

type FeedbackLabels<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Text,
        Has<NodeDescription>,
        Has<EntryDescription>,
    ),
    Or<(
        With<DraftFeedback>,
        With<NodeDescription>,
        With<EntryDescription>,
    )>,
>;

fn control(label: impl Into<String>, action: GraphControl, editor: &Editor) -> impl Scene {
    operation_button(label, GraphOperation::Control(action), editor)
}

fn operation_button(
    label: impl Into<String>,
    operation: GraphOperation,
    editor: &Editor,
) -> impl Scene {
    widgets::button_gated(
        label,
        ButtonVariant::Normal,
        widgets::ButtonGate::Managed,
        ControlButton {
            document_version: editor.document_version,
            operation: Some(operation),
        },
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rebuild_toolbar(
    editor: Res<Editor>,
    editing: Res<GraphEditing>,
    nav: Res<GraphNav>,
    slots: Query<(Entity, Option<&RenderedToolbar>), With<GraphEditToolbarSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    mut commands: Commands,
) {
    for (slot, rendered) in &slots {
        if stale_scene(slot, nav.version, &parents, &versions)
            || pending.contains(slot)
            || rendered.is_some_and(|rendered| {
                rendered.0 == editor.document_version && rendered.1 == editing.enabled
            })
        {
            continue;
        }
        let mut controls = vec![boxed(control(
            match editing.enabled {
                true => "退出流程编辑",
                false => "编辑流程",
            },
            GraphControl::Toggle,
            &editor,
        ))];
        if editing.enabled {
            controls.extend([
                boxed(control("新增节点", GraphControl::NewNode, &editor)),
                boxed(control("新增连接", GraphControl::NewEdge, &editor)),
                boxed(control("删除所选", GraphControl::DeleteSelection, &editor)),
                boxed(operation_button("撤销流程", GraphOperation::Undo, &editor)),
                boxed(operation_button("重做流程", GraphOperation::Redo, &editor)),
                boxed(widgets::button(
                    "保存",
                    ButtonVariant::Primary,
                    ActionButton(AppAction::SaveToRemote),
                )),
            ]);
            controls.push(boxed(bsn! {
                Text("")
                EntryDescription
                ThemeTextColor({theme::READONLY_TEXT})
                TextFont { font_size: px(10.5) }
                TextLayout { linebreak: {LineBreak::AnyCharacter} }
            }));
        }
        let content = vec![boxed(bsn! {
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(6),
                row_gap: px(5),
                padding: {UiRect::axes(px(10), px(6))},
                align_items: AlignItems::Center,
            }
            Children [{controls}]
        })];
        widgets::replace_slot_children(&mut commands, slot, content);
        commands
            .entity(slot)
            .insert(RenderedToolbar(editor.document_version, editing.enabled));
    }
}

fn field_editor(
    field: &DraftField,
    index: usize,
    editor: &Editor,
    editing: &GraphEditing,
) -> BoxedScene {
    let binding = DraftBinding {
        document_version: editor.document_version,
        generation: editing.panel_version,
        index: Some(index),
    };
    let widget = match field.kind {
        FieldKind::Bool => boxed(widgets::checkbox(field.text == "true", binding)),
        _ => {
            let multiline = matches!(field.kind, FieldKind::Json) || field.text.contains('\n');
            let mut editable = EditableText::new(&field.text);
            editable.allow_newlines = multiline;
            editable.visible_lines = Some(match multiline {
                true => 7.0,
                false => 1.0,
            });
            boxed(bsn! {
                @FeathersTextInputContainer
                Node {
                    width: percent(100),
                    min_width: px(0),
                    height: {match multiline { true => px(132), false => px(28) }},
                    border: {UiRect::all(px(1))},
                    padding: {UiRect::all(px(4))},
                    overflow: {Overflow::clip()},
                }
                ThemeBorderColor({theme::INPUT_BORDER})
                Children [(
                    @FeathersTextInput
                    Node { width: percent(100), min_width: px(0) }
                    template_value(editable)
                    template_value(binding)
                    TextLayout { linebreak: {match multiline { true => LineBreak::AnyCharacter, false => LineBreak::NoWrap }} }
                )]
            })
        }
    };
    let mut tools: Vec<BoxedScene> = vec![boxed(match field.kind {
        FieldKind::Json => control(
            "纯文本",
            GraphControl::Representation { index, json: false },
            editor,
        ),
        _ => control(
            "JSON / 表达式",
            GraphControl::Representation { index, json: true },
            editor,
        ),
    })];
    if !matches!(field.path[0].as_str(), "id" | "type" | "from" | "to") {
        tools.push(boxed(control(
            "移除",
            GraphControl::RemoveProperty(index),
            editor,
        )));
    }
    boxed(bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            min_width: px(0),
            row_gap: px(4),
            flex_shrink: 0.0,
        }
        Children [
            (widgets::hint(field.path.join(" / "))),
            (widget),
            (
                Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(4), row_gap: px(3) }
                Children [{tools}]
            ),
        ]
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rebuild_panel(
    editor: Res<Editor>,
    nav: Res<GraphNav>,
    editing: Res<GraphEditing>,
    slots: Query<(Entity, Option<&RenderedPanel>), With<GraphEditPanelSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    mut commands: Commands,
) {
    for (slot, rendered) in &slots {
        if stale_scene(slot, nav.version, &parents, &versions)
            || pending.contains(slot)
            || rendered.is_some_and(|rendered| rendered.0 == editing.panel_version)
        {
            continue;
        }
        let mut body: Vec<BoxedScene> = vec![boxed(bsn! {
            Text("流程编辑")
            ThemeTextColor({theme::SECTION_TEXT})
            TextFont { font_size: px(14) }
        })];
        match &editing.draft {
            None => {
                body.push(boxed(widgets::hint(
                    "选择节点或连接以编辑属性；也可新增节点、连接，或在画布上拖动端口连线。",
                )));
                if let Some(data) = &editor.data
                    && let Some(graph) = data.graph_edit.graph_key_at(&nav.path)
                    && let Some(error) = data.graph_edit.structure_error(graph)
                {
                    body.push(boxed(widgets::hint(error)));
                }
            }
            Some(draft) => {
                let title = match draft.target {
                    DraftTarget::Node(_) => "节点属性",
                    DraftTarget::Edge(_) => "连接属性",
                    DraftTarget::NewNode(_) => "新增节点草稿",
                    DraftTarget::NewEdge(_) => "新增连接草稿",
                    DraftTarget::Subgraph(_) => "内联子图 JSON",
                };
                body.push(boxed(widgets::hint(title)));
                if let Some(kind) = draft
                    .fields
                    .iter()
                    .find(|field| field.path.as_slice() == ["type"])
                    .map(|field| field.text.as_str())
                {
                    let description = crate::model::graph_schema::node_types()
                        .iter()
                        .find(|definition| definition.type_id == kind)
                        .map_or(
                            "此节点类型尚未核验，保留其全部属性；执行规则以对应执行器为准。",
                            |definition| definition.description,
                        );
                    body.push(boxed(bsn! {
                        Text(description)
                        NodeDescription
                        TextFont { font_size: px(11) }
                        TextLayout { linebreak: {LineBreak::AnyCharacter} }
                        ThemeTextColor({theme::READONLY_TEXT})
                    }));
                }
                if matches!(draft.target, DraftTarget::NewNode(_)) {
                    body.push(boxed(widgets::hint(
                        "节点模板（切换模板会重新填充未修改的草稿）",
                    )));
                    let templates: Vec<_> = crate::model::graph_schema::node_types()
                        .iter()
                        .map(|definition| {
                            boxed(control(
                                definition.name,
                                GraphControl::Template(definition.type_id.into()),
                                &editor,
                            ))
                        })
                        .collect();
                    body.push(boxed(bsn! {
                        Node {
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: px(3),
                            row_gap: px(3),
                            max_height: px(148),
                            overflow: {Overflow::scroll_y()},
                            flex_shrink: 0.0,
                        }
                        ScrollArea
                        Children [{templates}]
                    }));
                }
                body.push(boxed(widgets::hint("文本直接输入；对象、数组和新增属性填写 JSON。应用后可撤销；保存也会提交当前有效草稿。")));
                for (index, field) in draft
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(_, field)| !field.removed)
                {
                    body.push(field_editor(field, index, &editor, &editing));
                }
                body.push(boxed(widgets::hint(
                    "新增属性名；输入参数可写 inputs/参数名 或 params/参数名",
                )));
                body.push(boxed(widgets::text_field(
                    &draft.new_property,
                    DraftBinding {
                        document_version: editor.document_version,
                        generation: editing.panel_version,
                        index: None,
                    },
                )));
                body.push(boxed(control(
                    "添加属性",
                    GraphControl::AddProperty,
                    &editor,
                )));
                if let DraftTarget::Node(key) = draft.target {
                    body.push(boxed(control(
                        "克隆节点",
                        GraphControl::CloneNode(key),
                        &editor,
                    )));
                    body.push(boxed(bsn! {
                        Node { flex_direction: FlexDirection::Column, row_gap: px(5), flex_shrink: 0.0 }
                        SubgraphControls(key)
                        Children [
                            (control("进入子图", GraphControl::EnterSubgraph(key), &editor)),
                            (control("编辑 / 创建子图", GraphControl::EditSubgraph(key), &editor)),
                            (control("清空子图", GraphControl::ClearSubgraph(key), &editor)),
                            (control("移除子图", GraphControl::RemoveSubgraph(key), &editor)),
                            (widgets::hint("顺序、循环与并行容器必须保留子图；可清空内容或删除整个节点。")),
                        ]
                    }));
                    if nav.path.is_empty() {
                        body.push(boxed(control(
                            "设为流程入口",
                            GraphControl::SetEntryPoint(Some(key)),
                            &editor,
                        )));
                        body.push(boxed(control(
                            "按连线确定入口",
                            GraphControl::SetEntryPoint(None),
                            &editor,
                        )));
                    }
                }
            }
        }
        let mut header: Vec<BoxedScene> = Vec::new();
        if editing.draft.is_some() {
            header.push(boxed(control("应用修改", GraphControl::Apply, &editor)));
            header.push(boxed(control("重置草稿", GraphControl::Reset, &editor)));
        }
        let contents = boxed(bsn! {
            Node {
                width: percent(100),
                flex_grow: 1.0,
                min_height: px(0),
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(9),
                padding: {UiRect::all(px(10))},
                overflow: {Overflow::scroll_y()},
            }
            ScrollArea
            ThemeBackgroundColor({theme::SIDEBAR_BG})
            Children [{body}]
        });
        widgets::replace_slot_children(
            &mut commands,
            slot,
            vec![boxed(bsn! {
                Node { width: percent(100), height: percent(100), min_width: px(0), flex_direction: FlexDirection::Column }
                PanelGeneration({editing.panel_version})
                ThemeBackgroundColor({theme::SIDEBAR_BG})
                Children [
                    (
                        Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: px(4), padding: {UiRect::all(px(8))}, flex_shrink: 0.0,
                        }
                        Children [
                            (
                                Node {
                                    flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap,
                                    column_gap: px(6), row_gap: px(4),
                                }
                                Children [{header}]
                            ),
                            (
                                Node {
                                    width: percent(100), min_width: px(0), min_height: px(0),
                                    max_height: px(80), overflow: {Overflow::scroll_y()},
                                }
                                ScrollArea
                                Children [(
                                    Text("")
                                    DraftFeedback
                                    ThemeTextColor({theme::STATUS_ERROR})
                                    TextFont { font_size: px(11) }
                                    TextLayout { linebreak: {LineBreak::AnyCharacter} }
                                )]
                            ),
                        ]
                    ),
                    (contents)
                ]
            })],
        );
        commands
            .entity(slot)
            .insert(RenderedPanel(editing.panel_version));
    }
}

pub(super) fn refresh_feedback(
    editor: Res<Editor>,
    editing: Res<GraphEditing>,
    mut labels: FeedbackLabels,
    fields: Query<(&DraftBinding, &ChildOf)>,
    mut subgraphs: Query<(&SubgraphControls, &mut Node)>,
    history: Query<(Entity, &ControlButton)>,
    mut commands: Commands,
) {
    for (control, mut node) in &mut subgraphs {
        let definition = editor
            .data
            .as_ref()
            .and_then(|data| data.graph_edit.node_json(control.0))
            .and_then(|node| node.get("type"))
            .and_then(Value::as_str)
            .and_then(|kind| {
                crate::model::graph_schema::node_types()
                    .iter()
                    .find(|definition| definition.type_id == kind)
            });
        node.display = match definition.is_none_or(|definition| definition.container) {
            true => Display::Flex,
            false => Display::None,
        };
    }
    let message = editing
        .error
        .clone()
        .or_else(|| editing.draft.as_ref().and_then(|draft| draft.value().err()))
        .unwrap_or_default();
    let description = editing
        .draft
        .as_ref()
        .and_then(|draft| {
            draft
                .fields
                .iter()
                .find(|field| field.path.as_slice() == ["type"])
        })
        .and_then(|field| {
            crate::model::graph_schema::node_types()
                .iter()
                .find(|definition| definition.type_id == field.text)
        })
        .map_or(
            "此节点类型尚未核验，保留其全部属性；执行规则以对应执行器为准。",
            |definition| definition.description,
        );
    let entry = editor
        .data
        .as_ref()
        .and_then(|data| data.graph_edit.current_config().get("entry_point"))
        .and_then(Value::as_str)
        .map_or_else(
            || "顶层入口：按首条入口连线确定".to_owned(),
            |id| format!("顶层入口：{id}"),
        );
    for (mut text, is_description, is_entry) in &mut labels {
        let value = match (is_description, is_entry) {
            (true, _) => description,
            (_, true) => &entry,
            _ => &message,
        };
        if text.0 != value {
            text.0 = value.to_owned();
        }
    }
    for (binding, parent) in &fields {
        let Some(field) = binding
            .index
            .and_then(|index| editing.draft.as_ref()?.fields.get(index))
        else {
            continue;
        };
        if binding.generation != editing.panel_version {
            continue;
        }
        commands
            .entity(parent.parent())
            .insert(ThemeBorderColor(match field.value() {
                Ok(_) => theme::INPUT_BORDER,
                Err(_) => theme::STATUS_ERROR,
            }));
    }
    for (entity, button) in &history {
        let redo = match button.operation {
            Some(GraphOperation::Undo) => false,
            Some(GraphOperation::Redo) => true,
            Some(GraphOperation::Control(GraphControl::RemoveSubgraph(key))) => {
                let required = editor
                    .data
                    .as_ref()
                    .and_then(|data| data.graph_edit.node_json(key))
                    .and_then(|node| node.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "sequence" | "loop" | "parallel"));
                match required {
                    true => {
                        commands.entity(entity).insert(InteractionDisabled);
                    }
                    false => {
                        commands.entity(entity).remove::<InteractionDisabled>();
                    }
                }
                continue;
            }
            _ => continue,
        };
        let available = editor.data.as_ref().is_some_and(|data| match redo {
            true => data.graph_edit.can_redo(),
            false => data.graph_edit.can_undo(),
        }) && !editing.has_draft_changes();
        match available {
            true => {
                commands.entity(entity).remove::<InteractionDisabled>();
            }
            false => {
                commands.entity(entity).insert(InteractionDisabled);
            }
        }
    }
}
