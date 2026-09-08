//! 流程编辑意图、表单草稿与模型事务的统一入口。

use std::collections::BTreeSet;

use bevy::ecs::system::SystemParam;
use bevy::feathers::controls::ButtonVariant;
use bevy::prelude::*;
use bevy::text::{EditableText, TextEditChange};
use bevy::ui_widgets::{Activate, ValueChange};
use serde_json::{Map, Value};

use crate::model::graph_edit::{EdgeKey, GraphCommand, GraphKey, NodeKey};
use crate::model::{ENTRY_ID, EXIT_ID, TaskGraphData};

use super::graph_view::{GraphNav, ViewMode};
use super::widgets;
use super::worker_bridge::AppAction;
use super::{Editor, UiSet};

mod form;

#[cfg(test)]
mod tests;

/// 固定工具栏插槽，切换编辑模式不销毁画布。
#[derive(Component, Default, Clone)]
pub struct GraphEditToolbarSlot;

/// 独立编辑面板；草稿文本存资源，场景跨帧生成也不会丢失。
#[derive(Component, Default, Clone)]
pub struct GraphEditPanelSlot;

#[derive(Debug, Clone)]
pub struct GraphIntent {
    pub document_version: u64,
    pub revision: u64,
    pub operation: GraphOperation,
}

#[derive(Debug, Clone)]
pub enum GraphOperation {
    Command(GraphCommand),
    Undo,
    Redo,
    Control(GraphControl),
}

#[derive(Debug, Clone, Default)]
pub enum GraphControl {
    #[default]
    Toggle,
    NewNode,
    Template(String),
    NewEdge,
    CloneNode(NodeKey),
    DeleteSelection,
    EditSubgraph(NodeKey),
    EnterSubgraph(NodeKey),
    SetEntryPoint(Option<NodeKey>),
    RemoveSubgraph(NodeKey),
    ClearSubgraph(NodeKey),
    Apply,
    Reset,
    AddProperty,
    RemoveProperty(usize),
    Representation {
        index: usize,
        json: bool,
    },
}

/// 鼠标端口与属性表单共用这一提交边界；保存前统一刷新同帧文本。
pub fn submit_command(
    editor: &Editor,
    command: GraphCommand,
    actions: &mut MessageWriter<AppAction>,
) {
    if let Some(data) = &editor.data {
        actions.write(AppAction::GraphEdit(GraphIntent {
            document_version: editor.document_version,
            revision: data.graph_edit.revision(),
            operation: GraphOperation::Command(command),
        }));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftTarget {
    Node(NodeKey),
    Edge(EdgeKey),
    NewNode(GraphKey),
    NewEdge(GraphKey),
    Subgraph(NodeKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Text,
    Bool,
    Number,
    Json,
}

#[derive(Debug, Clone, PartialEq)]
struct DraftField {
    path: Vec<String>,
    kind: FieldKind,
    original: Option<Value>,
    text: String,
    removed: bool,
}

impl DraftField {
    fn new(path: Vec<String>, value: Option<Value>) -> Self {
        let kind = match &value {
            Some(Value::String(_)) => FieldKind::Text,
            Some(Value::Bool(_)) => FieldKind::Bool,
            Some(Value::Number(_)) => FieldKind::Number,
            _ => FieldKind::Json,
        };
        let text = match &value {
            Some(Value::String(text)) => text.clone(),
            Some(value) => serde_json::to_string_pretty(value).unwrap_or_default(),
            None => "null".into(),
        };
        Self {
            path,
            kind,
            original: value,
            text,
            removed: false,
        }
    }

    fn value(&self) -> Result<Option<Value>, String> {
        if self.removed {
            return Ok(None);
        }
        let value = match self.kind {
            FieldKind::Text => Value::String(self.text.clone()),
            FieldKind::Bool => match self.text.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err("请选择布尔值".into()),
            },
            FieldKind::Number | FieldKind::Json => {
                let value: Value = crate::model::graph_edit::parse_graph_value(&self.text)
                    .map_err(|error| format!("{}：{error}", self.path.join(".")))?;
                if self.kind == FieldKind::Number && !value.is_number() {
                    return Err(format!("{}：应为有效的 JSON 数字", self.path.join(".")));
                }
                value
            }
        };
        Ok(Some(value))
    }

    fn changed(&self) -> bool {
        self.value().map_or(true, |value| value != self.original)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct GraphDraft {
    target: DraftTarget,
    document_version: u64,
    revision: u64,
    original: Value,
    fields: Vec<DraftField>,
    new_property: String,
}

/// 文档异步切换只比较有实际修改的草稿，普通选择变化不算新编辑。
#[derive(Clone, Default, PartialEq)]
pub(super) struct GraphDraftSnapshot(Option<GraphDraft>);

impl GraphDraft {
    fn from_value(target: DraftTarget, editor: &Editor, value: Value) -> Self {
        let mut fields = Vec::new();
        if let Some(object) = value.as_object() {
            for (key, value) in object {
                if matches!(target, DraftTarget::Node(_) | DraftTarget::NewNode(_))
                    && matches!(key.as_str(), "nodes" | "edges" | "branches")
                {
                    continue;
                }
                match (key.as_str(), value) {
                    ("inputs" | "params", Value::Object(inputs)) if !inputs.is_empty() => {
                        for (name, value) in inputs {
                            fields.push(DraftField::new(
                                vec![key.clone(), name.clone()],
                                Some(value.clone()),
                            ));
                        }
                    }
                    _ => fields.push(DraftField::new(vec![key.clone()], Some(value.clone()))),
                }
            }
        }
        fields.sort_by_key(|field| match field.path[0].as_str() {
            "id" | "from" => 0,
            "type" | "to" => 1,
            "checkpoint" => 2,
            "inputs" | "params" => 3,
            _ => 4,
        });
        Self {
            target,
            document_version: editor.document_version,
            revision: editor
                .data
                .as_ref()
                .map_or(0, |data| data.graph_edit.revision()),
            original: value,
            fields,
            new_property: String::new(),
        }
    }

    fn changed(&self) -> bool {
        matches!(
            self.target,
            DraftTarget::NewNode(_) | DraftTarget::NewEdge(_)
        ) || self.fields.iter().any(DraftField::changed)
    }

    fn value(&self) -> Result<Value, String> {
        let mut result = self.original.clone();
        for field in &self.fields {
            if !field.changed() {
                continue;
            }
            let (name, parent_path) = field.path.split_last().ok_or("属性路径为空")?;
            let mut parent = &mut result;
            for segment in parent_path {
                parent = parent
                    .as_object_mut()
                    .ok_or("属性父级不是对象")?
                    .entry(segment.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
            }
            let object = parent.as_object_mut().ok_or("属性父级不是对象")?;
            match field.value()? {
                Some(value) => {
                    object.insert(name.clone(), value);
                }
                None => {
                    object.remove(name);
                }
            }
        }
        Ok(result)
    }

    fn command(&self) -> Result<GraphCommand, String> {
        let value = self.value()?;
        match self.target {
            DraftTarget::NewNode(graph) => Ok(GraphCommand::AddNode { graph, value }),
            DraftTarget::NewEdge(graph) => Ok(GraphCommand::AddEdge { graph, value }),
            DraftTarget::Subgraph(node) => Ok(GraphCommand::SetSubgraph {
                node,
                value: Some(value),
            }),
            DraftTarget::Node(node) => {
                let mut commands = Vec::new();
                for (key, value) in changed_properties(&self.original, &value)? {
                    match key.as_str() {
                        "id" => commands.push(GraphCommand::RenameNode {
                            node,
                            id: value
                                .and_then(|value| value.as_str().map(str::to_owned))
                                .ok_or("节点 ID 必须为字符串")?,
                        }),
                        "nodes" | "edges" | "branches" => {
                            return Err("子图请使用独立子图编辑入口".into());
                        }
                        _ => commands.push(GraphCommand::SetNodeProperty {
                            node,
                            property: key,
                            value,
                        }),
                    }
                }
                Ok(GraphCommand::Batch(commands))
            }
            DraftTarget::Edge(edge) => {
                let mut commands = Vec::new();
                if value.get("from") != self.original.get("from")
                    || value.get("to") != self.original.get("to")
                {
                    commands.push(GraphCommand::ReconnectEdge {
                        edge,
                        from: value
                            .get("from")
                            .and_then(Value::as_str)
                            .ok_or("起点应为字符串")?
                            .to_owned(),
                        to: value
                            .get("to")
                            .and_then(Value::as_str)
                            .ok_or("终点应为字符串")?
                            .to_owned(),
                    });
                }
                for (key, value) in changed_properties(&self.original, &value)? {
                    if !matches!(key.as_str(), "from" | "to") {
                        commands.push(GraphCommand::SetEdgeProperty {
                            edge,
                            property: key,
                            value,
                        });
                    }
                }
                Ok(GraphCommand::Batch(commands))
            }
        }
    }
}

fn changed_properties(
    before: &Value,
    after: &Value,
) -> Result<Vec<(String, Option<Value>)>, String> {
    let before = before.as_object().ok_or("原属性不是对象")?;
    let after = after.as_object().ok_or("属性必须为对象")?;
    Ok(before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| before.get(*key) != after.get(*key))
        .map(|key| (key.clone(), after.get(key).cloned()))
        .collect())
}

#[derive(Resource, Default)]
pub struct GraphEditing {
    pub enabled: bool,
    pub selected_nodes: BTreeSet<NodeKey>,
    pub panel_version: u64,
    document_version: Option<u64>,
    selection: Option<DraftTarget>,
    draft: Option<GraphDraft>,
    error: Option<String>,
}

impl GraphEditing {
    pub fn report_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }
    pub fn has_draft_changes(&self) -> bool {
        self.draft.as_ref().is_some_and(GraphDraft::changed)
    }

    pub(super) fn draft_snapshot(&self) -> GraphDraftSnapshot {
        GraphDraftSnapshot(self.draft.clone().filter(GraphDraft::changed))
    }

    /// 保存与应用按钮使用相同的草稿提交过程，非法输入不能静默退回模型旧值。
    pub fn apply_draft(&mut self, editor: &mut Editor) -> Result<bool, String> {
        let Some(draft) = &self.draft else {
            return Ok(false);
        };
        if !draft.changed() {
            return Ok(false);
        }
        if draft.document_version != editor.document_version {
            return Err("文档已重新加载，请重新选择编辑目标".into());
        }
        let command = draft.command()?;
        let data = editor.data.as_mut().ok_or("请先加载文档")?;
        let changed = data
            .apply_graph_command(draft.revision, command)
            .map_err(|error| error.to_string())?;
        self.error = None;
        match draft.target {
            DraftTarget::NewNode(_) | DraftTarget::NewEdge(_) => {
                self.draft = None;
                self.panel_version += 1;
            }
            target => {
                let value = match target {
                    DraftTarget::Node(key) => data.graph_edit.node_json(key).cloned(),
                    DraftTarget::Edge(key) => data.graph_edit.edge_json(key).cloned(),
                    DraftTarget::Subgraph(key) => {
                        data.graph_edit.node_json(key).map(subgraph_value)
                    }
                    _ => None,
                };
                if let Some(value) = value {
                    // 仅刷新基线与修订，不替换控件，保持用户的焦点和滚动。
                    if let Some(draft) = self.draft.as_mut() {
                        draft.original = value;
                        draft.revision = data.graph_edit.revision();
                        for field in &mut draft.fields {
                            field.original = field.value()?;
                        }
                    }
                }
            }
        }
        Ok(changed)
    }

    pub fn handle(
        &mut self,
        intent: &GraphIntent,
        editor: &mut Editor,
        nav: &mut GraphNav,
    ) -> Result<bool, String> {
        if intent.document_version != editor.document_version {
            return Err("旧文档的流程操作已取消".into());
        }
        let data = editor.data.as_ref().ok_or("请先加载文档")?;
        if intent.revision != data.graph_edit.revision() {
            return Err("流程已变化，请重新操作".into());
        }
        match &intent.operation {
            GraphOperation::Control(control) => self.control(control, editor, nav),
            operation => {
                if !self.enabled {
                    return Err("请先进入流程编辑模式".into());
                }
                if self.has_draft_changes() {
                    return Err("请先应用或重置右侧草稿，再执行其他流程操作".into());
                }
                let data = editor.data.as_mut().ok_or("请先加载文档")?;
                let changed = match operation {
                    GraphOperation::Command(command) => {
                        data.apply_graph_command(intent.revision, command.clone())
                    }
                    GraphOperation::Undo => data.undo_graph(intent.revision),
                    GraphOperation::Redo => data.redo_graph(intent.revision),
                    GraphOperation::Control(_) => unreachable!(),
                }
                .map_err(|error| error.to_string())?;
                self.draft = None;
                self.selection = None;
                self.panel_version += 1;
                self.error = None;
                Ok(changed)
            }
        }
    }

    fn control(
        &mut self,
        control: &GraphControl,
        editor: &mut Editor,
        nav: &mut GraphNav,
    ) -> Result<bool, String> {
        if matches!(control, GraphControl::Apply) {
            return self.apply_draft(editor);
        }
        if matches!(control, GraphControl::Reset) {
            self.draft = None;
            self.selection = None;
            self.error = None;
            self.panel_version += 1;
            return Ok(false);
        }
        if matches!(control, GraphControl::Toggle) {
            if self.has_draft_changes() {
                return Err("请先应用或重置草稿，再退出编辑".into());
            }
            self.enabled = !self.enabled;
            self.selected_nodes.clear();
            if self.enabled
                && let Some(data) = &editor.data
                && let Some(graph) = data.graph_edit.graph_key_at(&nav.path)
                && let Some(key) = nav
                    .selected
                    .as_deref()
                    .and_then(|id| data.graph_edit.node_key(graph, id))
            {
                self.selected_nodes.insert(key);
            }
            self.panel_version += 1;
            return Ok(false);
        }
        if !self.enabled && !matches!(control, GraphControl::EnterSubgraph(_)) {
            return Err("请先进入流程编辑模式".into());
        }
        let data = editor.data.as_ref().ok_or("请先加载文档")?;
        let graph = data
            .graph_edit
            .graph_key_at(&nav.path)
            .ok_or("当前图层已不存在")?;
        if matches!(
            control,
            GraphControl::AddProperty
                | GraphControl::RemoveProperty(_)
                | GraphControl::Representation { .. }
        ) {
            let draft = self.draft.as_mut().ok_or("请先选择编辑目标")?;
            match control {
                GraphControl::RemoveProperty(index) => {
                    draft.fields.get_mut(*index).ok_or("属性已不存在")?.removed = true;
                }
                GraphControl::AddProperty => {
                    let name = draft.new_property.trim();
                    if name.is_empty() {
                        return Err("请输入属性名称".into());
                    }
                    let path = match name.split_once('/') {
                        Some((parent @ ("inputs" | "params"), key)) if !key.is_empty() => {
                            vec![parent.into(), key.into()]
                        }
                        _ => vec![name.into()],
                    };
                    if draft
                        .fields
                        .iter()
                        .any(|field| field.path == path && !field.removed)
                    {
                        return Err("属性已存在".into());
                    }
                    draft.fields.push(DraftField::new(path, None));
                    draft.new_property.clear();
                }
                GraphControl::Representation { index, json } => {
                    let field = draft.fields.get_mut(*index).ok_or("属性已不存在")?;
                    let value = field.value()?.ok_or("属性已被移除")?;
                    match json {
                        true => {
                            field.text = serde_json::to_string_pretty(&value)
                                .map_err(|error| error.to_string())?;
                            field.kind = FieldKind::Json;
                        }
                        false => {
                            field.text = match value {
                                Value::String(value) => value,
                                value => value.to_string(),
                            };
                            field.kind = FieldKind::Text;
                        }
                    }
                }
                _ => {}
            }
            self.panel_version += 1;
            return Ok(false);
        }
        let replacing_template = matches!(control, GraphControl::Template(_))
            && self.draft.as_ref().is_some_and(|draft| {
                matches!(draft.target, DraftTarget::NewNode(_))
                    && !draft.fields.iter().any(DraftField::changed)
            });
        if self.has_draft_changes() && !replacing_template {
            return Err("请先应用或重置当前草稿".into());
        }
        let (target, value) = match control {
            GraphControl::NewNode | GraphControl::Template(_) => {
                let kind = match control {
                    GraphControl::Template(kind) => kind.as_str(),
                    _ => "log",
                };
                let id = unique_id(data, graph, kind);
                let value =
                    crate::model::graph_schema::node_template(kind, &id).ok_or("未知的节点模板")?;
                (DraftTarget::NewNode(graph), value)
            }
            GraphControl::CloneNode(key) => {
                if !data
                    .graph_edit
                    .nodes(graph)
                    .iter()
                    .any(|(candidate, _)| candidate == key)
                {
                    return Err("节点已不在当前图层，请重新选择".into());
                }
                let mut value = data
                    .graph_edit
                    .node_json(*key)
                    .cloned()
                    .ok_or("节点已不存在")?;
                let base = value.get("id").and_then(Value::as_str).unwrap_or("node");
                value["id"] = Value::String(unique_id(data, graph, &format!("{base}_copy")));
                (DraftTarget::NewNode(graph), value)
            }
            GraphControl::NewEdge => (
                DraftTarget::NewEdge(graph),
                serde_json::json!({"from":ENTRY_ID,"to":EXIT_ID}),
            ),
            GraphControl::EditSubgraph(key) => {
                let node = data.graph_edit.node_json(*key).ok_or("节点已不存在")?;
                (DraftTarget::Subgraph(*key), subgraph_value(node))
            }
            GraphControl::EnterSubgraph(key) => {
                if !data
                    .graph_edit
                    .nodes(graph)
                    .iter()
                    .any(|(candidate, _)| candidate == key)
                {
                    return Err("节点已不在当前图层，请重新选择".into());
                }
                let id = data
                    .graph_edit
                    .node_id(*key)
                    .ok_or("节点已不存在")?
                    .to_owned();
                let mut path = nav.path.clone();
                path.push(id);
                if data.graph_edit.graph_key_at(&path).is_none() {
                    return Err("此节点没有内联子图".into());
                }
                nav.path = path;
                nav.selected = None;
                nav.selected_edge = None;
                nav.version += 1;
                self.draft = None;
                self.selection = None;
                self.selected_nodes.clear();
                self.panel_version += 1;
                return Ok(false);
            }
            GraphControl::RemoveSubgraph(key) => {
                let revision = data.graph_edit.revision();
                return editor
                    .data
                    .as_mut()
                    .ok_or("请先加载文档")?
                    .apply_graph_command(
                        revision,
                        GraphCommand::SetSubgraph {
                            node: *key,
                            value: None,
                        },
                    )
                    .map_err(|error| error.to_string());
            }
            GraphControl::ClearSubgraph(key) => {
                let node = data.graph_edit.node_json(*key).ok_or("节点已不存在")?;
                let parallel = node.get("type").and_then(Value::as_str) == Some("parallel");
                let array_key = match parallel && node.get("branches").is_some() {
                    true => "branches",
                    false => "nodes",
                };
                let mut value = Map::new();
                value.insert(array_key.into(), serde_json::json!([]));
                if !parallel || node.get("edges").is_some() {
                    value.insert(
                        "edges".into(),
                        match parallel {
                            true => serde_json::json!([]),
                            false => serde_json::json!([{"from":ENTRY_ID,"to":EXIT_ID}]),
                        },
                    );
                }
                let revision = data.graph_edit.revision();
                return editor
                    .data
                    .as_mut()
                    .ok_or("请先加载文档")?
                    .apply_graph_command(
                        revision,
                        GraphCommand::SetSubgraph {
                            node: *key,
                            value: Some(Value::Object(value)),
                        },
                    )
                    .map_err(|error| error.to_string());
            }
            GraphControl::SetEntryPoint(key) => {
                if graph != data.graph_edit.root_key() {
                    return Err("显式流程入口只用于顶层图".into());
                }
                let id = match key {
                    Some(key) => Some(
                        data.graph_edit
                            .node_id(*key)
                            .ok_or("入口节点已不存在")?
                            .to_owned(),
                    ),
                    None => None,
                };
                let revision = data.graph_edit.revision();
                return editor
                    .data
                    .as_mut()
                    .ok_or("请先加载文档")?
                    .apply_graph_command(revision, GraphCommand::SetEntryPoint { graph, id })
                    .map_err(|error| error.to_string());
            }
            GraphControl::DeleteSelection => {
                let command = match nav
                    .selected_edge
                    .and_then(|index| data.graph_edit.edge_key_at(graph, index))
                {
                    Some(edge) => GraphCommand::DeleteEdges { edges: vec![edge] },
                    None => {
                        let local: BTreeSet<_> = data
                            .graph_edit
                            .nodes(graph)
                            .into_iter()
                            .map(|(key, _)| key)
                            .collect();
                        let mut nodes: Vec<_> =
                            self.selected_nodes.intersection(&local).copied().collect();
                        if nodes.is_empty()
                            && let Some(key) = nav
                                .selected
                                .as_deref()
                                .and_then(|id| data.graph_edit.node_key(graph, id))
                        {
                            nodes.push(key);
                        }
                        GraphCommand::DeleteNodes { nodes }
                    }
                };
                let revision = data.graph_edit.revision();
                return editor
                    .data
                    .as_mut()
                    .ok_or("请先加载文档")?
                    .apply_graph_command(revision, command)
                    .map_err(|error| error.to_string());
            }
            _ => return Ok(false),
        };
        self.draft = Some(GraphDraft::from_value(target, editor, value));
        self.selection = match target {
            DraftTarget::Subgraph(key) => Some(DraftTarget::Node(key)),
            _ => Some(target),
        };
        self.error = None;
        self.panel_version += 1;
        Ok(false)
    }
}

fn unique_id(data: &TaskGraphData, graph: GraphKey, base: &str) -> String {
    let mut index = 1u64;
    loop {
        let id = format!("{base}_{index}");
        if data.graph_edit.node_key(graph, &id).is_none() {
            return id;
        }
        index += 1;
    }
}

fn subgraph_value(node: &Value) -> Value {
    let array_key = match node.get("type").and_then(Value::as_str) == Some("parallel")
        && node.get("branches").is_some()
    {
        true => "branches",
        false => "nodes",
    };
    let mut value = Map::new();
    for key in [array_key, "edges"] {
        if let Some(original) = node.get(key) {
            value.insert(key.into(), original.clone());
        }
    }
    match value.is_empty() {
        true => serde_json::json!({"nodes":[], "edges":[{"from":ENTRY_ID,"to":EXIT_ID}]}),
        false => Value::Object(value),
    }
}

#[derive(Component, Clone, Default)]
struct ControlButton {
    document_version: u64,
    operation: Option<GraphOperation>,
}

#[derive(Component, Clone, Default)]
struct DraftBinding {
    document_version: u64,
    generation: u64,
    index: Option<usize>,
}

#[derive(Component, Clone, Copy, Default)]
struct PanelGeneration(u64);

fn on_control(
    event: On<Activate>,
    buttons: Query<&ControlButton>,
    parents: Query<&ChildOf>,
    panels: Query<&PanelGeneration>,
    editor: Res<Editor>,
    editing: Res<GraphEditing>,
    mut actions: MessageWriter<AppAction>,
) {
    let Some(entity) =
        widgets::self_or_ancestor(event.entity, &parents, |entity| buttons.contains(entity))
    else {
        return;
    };
    let Ok(button) = buttons.get(entity) else {
        return;
    };
    if let Some(panel) =
        widgets::self_or_ancestor(entity, &parents, |entity| panels.contains(entity))
        && let Ok(generation) = panels.get(panel)
        && generation.0 != editing.panel_version
    {
        return;
    }
    let Some(operation) = &button.operation else {
        return;
    };
    let Some(data) = &editor.data else { return };
    let revision = match operation {
        GraphOperation::Control(GraphControl::Apply) => editing
            .draft
            .as_ref()
            .map_or(data.graph_edit.revision(), |draft| draft.revision),
        _ => data.graph_edit.revision(),
    };
    actions.write(AppAction::GraphEdit(GraphIntent {
        document_version: button.document_version,
        revision,
        operation: operation.clone(),
    }));
}

fn on_draft_text(
    event: On<TextEditChange>,
    fields: Query<(&DraftBinding, &EditableText)>,
    mut editing: ResMut<GraphEditing>,
) {
    let Ok((binding, text)) = fields.get(event.event_target()) else {
        return;
    };
    if binding.generation != editing.panel_version {
        return;
    }
    let Some(draft) = &mut editing.draft else {
        return;
    };
    if binding.document_version != draft.document_version {
        return;
    }
    match binding.index {
        Some(index) => {
            let Some(field) = draft.fields.get_mut(index) else {
                return;
            };
            let value = text.value().to_string();
            if field.text == value {
                return;
            }
            field.text = value;
        }
        None => {
            let value = text.value().to_string();
            if draft.new_property == value {
                return;
            }
            draft.new_property = value;
        }
    }
    editing.error = None;
}

fn on_draft_bool(
    event: On<ValueChange<bool>>,
    fields: Query<&DraftBinding>,
    mut editing: ResMut<GraphEditing>,
    mut commands: Commands,
) {
    let Ok(binding) = fields.get(event.event_target()) else {
        return;
    };
    if binding.generation != editing.panel_version {
        return;
    }
    let Some(draft) = &mut editing.draft else {
        return;
    };
    if binding.document_version != draft.document_version {
        return;
    }
    let Some(field) = binding.index.and_then(|index| draft.fields.get_mut(index)) else {
        return;
    };
    let value = event.value.to_string();
    let changed = field.text != value;
    field.text = value;
    match event.value {
        true => {
            commands
                .entity(event.event_target())
                .insert(bevy::ui::Checked);
        }
        false => {
            commands
                .entity(event.event_target())
                .remove::<bevy::ui::Checked>();
        }
    }
    if changed {
        editing.error = None;
    }
}

fn synchronize_selection(
    editor: Res<Editor>,
    nav: Res<GraphNav>,
    mut editing: ResMut<GraphEditing>,
) {
    if editing.document_version != Some(editor.document_version) {
        let enabled = editing.enabled;
        *editing = GraphEditing {
            document_version: Some(editor.document_version),
            enabled,
            panel_version: editing.panel_version + 1,
            ..default()
        };
    }
    if !editing.enabled || editing.has_draft_changes() {
        return;
    }
    let Some(data) = &editor.data else { return };
    let Some(graph) = data.graph_edit.graph_key_at(&nav.path) else {
        return;
    };
    let selected = match nav
        .selected_edge
        .and_then(|index| data.graph_edit.edge_key_at(graph, index))
    {
        Some(key) => Some(DraftTarget::Edge(key)),
        None => nav
            .selected
            .as_deref()
            .and_then(|id| data.graph_edit.node_key(graph, id))
            .map(DraftTarget::Node),
    };
    if selected == editing.selection
        && match (&editing.draft, selected) {
            (None, None) => true,
            (Some(draft), _) => draft.revision == data.graph_edit.revision(),
            (None, Some(_)) => false,
        }
    {
        return;
    }
    let value = match selected {
        Some(DraftTarget::Node(key)) => data.graph_edit.node_json(key),
        Some(DraftTarget::Edge(key)) => data.graph_edit.edge_json(key),
        _ => None,
    };
    editing.draft = selected
        .zip(value)
        .map(|(target, value)| GraphDraft::from_value(target, &editor, value.clone()));
    editing.selection = selected;
    editing.panel_version += 1;
}

/// 文本框保留自己的编辑快捷键；流程快捷键仅在画布视图生效。
#[derive(SystemParam)]
struct ShortcutContext<'w, 's> {
    keys: Res<'w, ButtonInput<KeyCode>>,
    mode: Res<'w, ViewMode>,
    editor: Res<'w, Editor>,
    editing: Res<'w, GraphEditing>,
    focus: Option<Res<'w, bevy::input_focus::InputFocus>>,
    texts: Query<'w, 's, (), With<EditableText>>,
    parents: Query<'w, 's, &'static ChildOf>,
    guard: Option<Res<'w, super::document_guard::DocumentGuard>>,
}

fn keyboard_shortcuts(context: ShortcutContext, mut actions: MessageWriter<AppAction>) {
    let ShortcutContext {
        keys,
        mode,
        editor,
        editing,
        focus,
        texts,
        parents,
        guard,
    } = context;
    if *mode != ViewMode::Graph || guard.as_ref().is_some_and(|guard| guard.active()) {
        return;
    }
    let command = keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    if command && keys.just_pressed(KeyCode::KeyS) {
        actions.write(AppAction::SaveToRemote);
        return;
    }
    if !editing.enabled
        || focus
            .as_ref()
            .and_then(|focus| focus.get())
            .is_some_and(|entity| {
                widgets::self_or_ancestor(entity, &parents, |entity| texts.contains(entity))
                    .is_some()
            })
    {
        return;
    }
    let operation = match (
        command,
        keys.just_pressed(KeyCode::KeyZ),
        keys.just_pressed(KeyCode::KeyY),
    ) {
        (true, true, _) if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) => {
            Some(GraphOperation::Redo)
        }
        (true, true, _) => Some(GraphOperation::Undo),
        (true, _, true) => Some(GraphOperation::Redo),
        _ if keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace) => {
            Some(GraphOperation::Control(GraphControl::DeleteSelection))
        }
        _ => None,
    };
    if let (Some(operation), Some(data)) = (operation, editor.data.as_ref()) {
        actions.write(AppAction::GraphEdit(GraphIntent {
            document_version: editor.document_version,
            revision: data.graph_edit.revision(),
            operation,
        }));
    }
}

pub struct GraphEditPlugin;

impl Plugin for GraphEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GraphEditing>()
            .add_observer(on_control)
            .add_observer(on_draft_text)
            .add_observer(on_draft_bool)
            .add_systems(Update, keyboard_shortcuts.in_set(UiSet::Input))
            .add_systems(
                Update,
                (
                    synchronize_selection,
                    form::rebuild_toolbar,
                    form::rebuild_panel,
                    form::refresh_feedback,
                )
                    .chain()
                    .after(super::graph_view::GraphViewRebuilt)
                    .in_set(UiSet::Rebuild),
            );
    }
}
