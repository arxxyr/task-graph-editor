//! 流程图完整 JSON 工作副本、稳定身份和原子编辑历史。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::{ENTRY_ID, EXIT_ID};

mod commands;
use commands::apply_command;

/// 图层身份仅在当前文档内有效，不写入机器人文件。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GraphKey(pub u64);

/// 节点身份独立于业务 ID 和数组位置。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeKey(pub u64);

/// 平行边也拥有独立身份。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeKey(pub u64);

#[derive(Debug, Clone)]
pub enum GraphCommand {
    /// 多字段表单一次提交，任意子命令失败都会撤销整个候选事务。
    Batch(Vec<GraphCommand>),
    /// 只有根流程读取 entry_point；None 恢复执行器按首条 _entry 边确定入口的规则。
    SetEntryPoint {
        graph: GraphKey,
        id: Option<String>,
    },
    SetNodeProperty {
        node: NodeKey,
        property: String,
        value: Option<Value>,
    },
    RenameNode {
        node: NodeKey,
        id: String,
    },
    AddNode {
        graph: GraphKey,
        value: Value,
    },
    DeleteNodes {
        nodes: Vec<NodeKey>,
    },
    AddEdge {
        graph: GraphKey,
        value: Value,
    },
    DeleteEdges {
        edges: Vec<EdgeKey>,
    },
    ReconnectEdge {
        edge: EdgeKey,
        from: String,
        to: String,
    },
    SetEdgeProperty {
        edge: EdgeKey,
        property: String,
        value: Option<Value>,
    },
    /// 对象包含 nodes/edges，parallel 还接受 branches；不能同时提交 nodes 与 branches。
    /// 已有 branches 的 parallel 会把标准 nodes 入参写回 branches，保留原来被遮蔽的 nodes。
    /// None 移除全部子图声明，parallel 同时移除 branches/nodes，避免激活被遮蔽的旧节点。
    SetSubgraph {
        node: NodeKey,
        value: Option<Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphEditError {
    #[error("流程已变化，请重新操作（预期修订 {expected}，当前修订 {actual}）")]
    StaleRevision { expected: u64, actual: u64 },
    #[error("编辑目标已不存在，请重新选择")]
    MissingTarget,
    #[error("流程结构不合法：{0}")]
    InvalidStructure(String),
    #[error("属性值不合法：{0}")]
    InvalidProperty(String),
}

/// 图属性 JSON 不经过数值控件：允许完整 i64/u64，拒绝 serde 静默降为浮点的大整数。
/// 显式带小数点或指数的数仍按 JSON 浮点数处理；字符串内容不参与数字校验。
pub fn parse_graph_value(text: &str) -> Result<Value, GraphEditError> {
    let value = serde_json::from_str(text)
        .map_err(|error| GraphEditError::InvalidProperty(format!("JSON 解析失败：{error}")))?;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index += 2,
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && matches!(bytes[index], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    index += 1;
                }
                let number = &text[start..index];
                if !number.contains(['.', 'e', 'E'])
                    && number.parse::<i64>().is_err()
                    && number.parse::<u64>().is_err()
                {
                    return Err(GraphEditError::InvalidProperty(format!(
                        "整数 {number} 超出 i64/u64 范围，无法无损保存；请按执行器协议使用字符串"
                    )));
                }
            }
            _ => index += 1,
        }
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NodeIdentity {
    key: NodeKey,
    child: Option<Box<GraphIdentity>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphIdentity {
    key: GraphKey,
    nodes: Vec<NodeIdentity>,
    edges: Vec<EdgeKey>,
}

#[derive(Debug, Clone)]
struct GraphState {
    config: Value,
    identity: GraphIdentity,
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    before: GraphState,
    after: GraphState,
    topology: bool,
}

/// JSON 是唯一可写来源，身份树只记录同位置元素的内部 key。
/// 事务在候选副本上完整校验，成功后才替换当前状态。
#[derive(Debug, Clone)]
pub struct GraphDocument {
    initial: Value,
    initial_identity: GraphIdentity,
    state: GraphState,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    next_key: u64,
    revision: u64,
    topology_revision: u64,
}

impl Default for GraphDocument {
    fn default() -> Self {
        Self::new(&Value::Object(Map::new()))
    }
}

impl GraphDocument {
    pub fn new(config: &Value) -> Self {
        // 根层只拥有执行图字段。context 和其他配置由 TaskGraphData::raw_json 保留，
        // 避免根配置里未知的 type/branches 字段被误认成 parallel 节点的子图声明。
        let config = match config {
            Value::Object(fields) => Value::Object(
                ["nodes", "edges", "entry_point"]
                    .into_iter()
                    .filter_map(|key| fields.get(key).map(|value| (key.to_owned(), value.clone())))
                    .collect(),
            ),
            _ => config.clone(),
        };
        let mut next_key = 1;
        let identity = build_identity(&config, &mut next_key);
        Self {
            initial: config.clone(),
            initial_identity: identity.clone(),
            state: GraphState {
                config: config.clone(),
                identity,
            },
            undo: Vec::new(),
            redo: Vec::new(),
            next_key,
            revision: 0,
            topology_revision: 0,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn topology_revision(&self) -> u64 {
        self.topology_revision
    }
    pub fn root_key(&self) -> GraphKey {
        self.state.identity.key
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn has_changes(&self) -> bool {
        self.state.config != self.initial
    }
    pub fn current_config(&self) -> &Value {
        &self.state.config
    }

    pub fn graph_key_at(&self, path: &[String]) -> Option<GraphKey> {
        if path.is_empty() {
            return Some(self.root_key());
        }
        let mut raw = &self.state.config;
        let mut identity = &self.state.identity;
        for id in path {
            let index = unique_node_index(raw, id)?;
            raw = node_at(raw, index)?;
            identity = identity.nodes.get(index)?.child.as_deref()?;
        }
        Some(identity.key)
    }

    pub fn node_key(&self, graph: GraphKey, id: &str) -> Option<NodeKey> {
        let path = graph_location(&self.state.identity, graph)?;
        let raw = graph_at(&self.state.config, &path)?;
        let identity = identity_at(&self.state.identity, &path)?;
        Some(identity.nodes.get(unique_node_index(raw, id)?)?.key)
    }

    /// 索引与派生 SubGraph::edges 一致，错误的非字符串端点不占展示索引。
    pub fn edge_key_at(&self, graph: GraphKey, index: usize) -> Option<EdgeKey> {
        self.edges(graph).get(index).map(|(key, _, _)| *key)
    }

    pub fn graph_json(&self, graph: GraphKey) -> Option<&Value> {
        let path = graph_location(&self.state.identity, graph)?;
        graph_at(&self.state.config, &path)
    }

    pub fn node_json(&self, node: NodeKey) -> Option<&Value> {
        let (path, index) = node_location(&self.state.identity, node)?;
        node_at(graph_at(&self.state.config, &path)?, index)
    }

    pub fn edge_json(&self, edge: EdgeKey) -> Option<&Value> {
        let (path, index) = edge_location(&self.state.identity, edge)?;
        graph_at(&self.state.config, &path)?
            .get("edges")?
            .get(index)
    }

    pub fn node_id(&self, node: NodeKey) -> Option<&str> {
        self.node_json(node)?.get("id")?.as_str()
    }

    pub fn graph_path(&self, graph: GraphKey) -> Option<Vec<String>> {
        let path = graph_location(&self.state.identity, graph)?;
        let mut raw = &self.state.config;
        let mut result = Vec::with_capacity(path.len());
        for index in path {
            raw = node_at(raw, index)?;
            result.push(raw.get("id")?.as_str()?.to_owned());
        }
        Some(result)
    }

    pub fn nodes(&self, graph: GraphKey) -> Vec<(NodeKey, String)> {
        let Some(path) = graph_location(&self.state.identity, graph) else {
            return Vec::new();
        };
        let Some(raw) = graph_at(&self.state.config, &path) else {
            return Vec::new();
        };
        let Some(identity) = identity_at(&self.state.identity, &path) else {
            return Vec::new();
        };
        array(raw, "nodes")
            .iter()
            .zip(&identity.nodes)
            .filter_map(|(node, identity)| {
                Some((identity.key, node.get("id")?.as_str()?.to_owned()))
            })
            .collect()
    }

    pub fn edges(&self, graph: GraphKey) -> Vec<(EdgeKey, String, String)> {
        let Some(path) = graph_location(&self.state.identity, graph) else {
            return Vec::new();
        };
        let Some(raw) = graph_at(&self.state.config, &path) else {
            return Vec::new();
        };
        let Some(identity) = identity_at(&self.state.identity, &path) else {
            return Vec::new();
        };
        array(raw, "edges")
            .iter()
            .zip(&identity.edges)
            .filter_map(|(edge, key)| {
                Some((
                    *key,
                    edge.get("from")?.as_str()?.to_owned(),
                    edge.get("to")?.as_str()?.to_owned(),
                ))
            })
            .collect()
    }

    /// 校验当前层，不因另一个子图的既有问题封锁本层操作。
    pub fn structure_error(&self, graph: GraphKey) -> Option<String> {
        let result = graph_location(&self.state.identity, graph)
            .and_then(|path| graph_at(&self.state.config, &path))
            .ok_or(GraphEditError::MissingTarget)
            .and_then(validate_layer);
        result.err().map(|error| error.to_string())
    }

    pub(super) fn apply(
        &mut self,
        expected: u64,
        command: GraphCommand,
    ) -> Result<bool, GraphEditError> {
        self.check_revision(expected)?;
        let targets = attribute_targets(&command);
        let subgraph_targets = subgraph_targets(&command);
        let mut candidate = self.state.clone();
        let mut next_key = self.next_key;
        apply_command(&mut candidate, &mut next_key, command)?;
        validate_subgraph_commands(&self.state, &candidate, &subgraph_targets)?;
        if candidate.config == self.state.config {
            return Ok(false);
        }
        restore_unchanged_subgraphs(&self.state, &mut candidate);
        validate_changed_nodes(&self.state, &candidate, &targets)?;
        let topology = !same_topology(
            &self.state.config,
            &self.state.identity,
            &candidate.config,
            &candidate.identity,
            true,
        );
        self.undo.push(HistoryEntry {
            before: self.state.clone(),
            after: candidate.clone(),
            topology,
        });
        self.redo.clear();
        self.state = candidate;
        self.next_key = next_key;
        self.advance(topology);
        Ok(true)
    }

    pub(super) fn undo(&mut self, expected: u64) -> Result<bool, GraphEditError> {
        self.check_revision(expected)?;
        let Some(entry) = self.undo.pop() else {
            return Ok(false);
        };
        self.state = entry.before.clone();
        self.advance(entry.topology);
        self.redo.push(entry);
        Ok(true)
    }

    pub(super) fn redo(&mut self, expected: u64) -> Result<bool, GraphEditError> {
        self.check_revision(expected)?;
        let Some(entry) = self.redo.pop() else {
            return Ok(false);
        };
        self.state = entry.after.clone();
        self.advance(entry.topology);
        self.undo.push(entry);
        Ok(true)
    }

    fn check_revision(&self, expected: u64) -> Result<(), GraphEditError> {
        match expected == self.revision {
            true => Ok(()),
            false => Err(GraphEditError::StaleRevision {
                expected,
                actual: self.revision,
            }),
        }
    }

    fn advance(&mut self, topology: bool) {
        self.revision += 1;
        if topology {
            self.topology_revision += 1;
        }
    }

    /// 只拒绝当前编辑新增的执行入口错误；未改流程及既有问题不阻止 context 保存。
    pub fn validate_for_save(&self) -> Result<(), GraphEditError> {
        if !self.has_changes() {
            return Ok(());
        }
        let mut baseline = BTreeMap::new();
        let mut current = BTreeMap::new();
        entry_problems(
            &self.initial,
            &self.initial_identity,
            true,
            "$.config",
            &mut baseline,
        );
        entry_problems(
            &self.state.config,
            &self.state.identity,
            true,
            "$.config",
            &mut current,
        );
        match current
            .into_iter()
            .find(|(key, _)| !baseline.contains_key(key))
        {
            Some((_, message)) => Err(structure_error(message)),
            None => Ok(()),
        }
    }

    /// 只合并流程数组及实际变化的入口，不回写加载时的 context 或其他配置。
    pub(super) fn merge_into(&self, document: &mut Value) {
        if !self.has_changes() {
            return;
        }
        let Some(root) = document.as_object_mut() else {
            return;
        };
        let config = root
            .entry("config")
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(config) = config.as_object_mut() else {
            return;
        };
        for key in ["nodes", "edges", "entry_point"] {
            // 根入口也是执行器的节点引用；其他根配置不属于流程编辑器。
            if self.state.config.get(key) == self.initial.get(key) {
                continue;
            }
            match self.state.config.get(key) {
                Some(value) => {
                    config.insert(key.to_owned(), value.clone());
                }
                None => {
                    config.remove(key);
                }
            }
        }
    }
}

fn attribute_targets(command: &GraphCommand) -> BTreeSet<NodeKey> {
    match command {
        GraphCommand::Batch(commands) => commands.iter().flat_map(attribute_targets).collect(),
        GraphCommand::SetNodeProperty { node, .. } | GraphCommand::SetSubgraph { node, .. } => {
            BTreeSet::from([*node])
        }
        _ => BTreeSet::new(),
    }
}

fn subgraph_targets(command: &GraphCommand) -> BTreeMap<NodeKey, bool> {
    match command {
        GraphCommand::Batch(commands) => commands.iter().flat_map(subgraph_targets).collect(),
        GraphCommand::SetSubgraph { node, value } => BTreeMap::from([(*node, value.is_some())]),
        _ => BTreeMap::new(),
    }
}

/// 子图和类型可以在一个 Batch 内转换，是否启用必须以整个事务的最终状态为准。
fn validate_subgraph_commands(
    before: &GraphState,
    after: &GraphState,
    targets: &BTreeMap<NodeKey, bool>,
) -> Result<(), GraphEditError> {
    for (&key, &replace) in targets {
        let Some((path, index)) = node_location(&after.identity, key) else {
            // 同一事务随后删除目标时，无需校验已经不存在的子图。
            continue;
        };
        let node = node_at(
            graph_at(&after.config, &path).ok_or(GraphEditError::MissingTarget)?,
            index,
        )
        .ok_or(GraphEditError::MissingTarget)?;
        let original = node_location(&before.identity, key)
            .and_then(|(path, index)| node_at(graph_at(&before.config, &path)?, index));
        match replace {
            true if !has_subgraph(node) => {
                return Err(structure_error(
                    inactive_subgraph_reason(node).unwrap_or_else(|| {
                        "当前节点未启用内联子图；condition 需要同时提供 nodes 和 edges 数组".into()
                    }),
                ));
            }
            false if original.is_some_and(|node| !has_subgraph(node)) => {
                return Err(structure_error(
                    "原节点没有启用内联子图，同名扩展字段不能通过子图操作删除",
                ));
            }
            _ => (),
        }
    }
    Ok(())
}

/// Batch 可能先替换再恢复子图。最终内容和来源都没变时，不能提交临时新分配的身份。
fn restore_unchanged_subgraphs(before: &GraphState, after: &mut GraphState) {
    fn visit(raw: &Value, identity: &mut GraphIdentity, before: &GraphState) {
        for (node, identity) in array(raw, "nodes").iter().zip(&mut identity.nodes) {
            let original =
                node_location(&before.identity, identity.key).and_then(|(path, index)| {
                    Some((
                        node_at(graph_at(&before.config, &path)?, index)?,
                        identity_at(&before.identity, &path)?.nodes.get(index)?,
                    ))
                });
            if let Some((original, previous)) = original
                && previous.child.is_some()
                && identity.child.is_some()
                && node_array_key(original) == node_array_key(node)
                && original.get(node_array_key(original)) == node.get(node_array_key(node))
                && original.get("edges") == node.get("edges")
            {
                identity.child.clone_from(&previous.child);
                continue;
            }
            if let Some(child) = identity.child.as_deref_mut() {
                visit(node, child, before);
            }
        }
    }
    visit(&after.config, &mut after.identity, before);
}

/// 仅最终拓扑差异推进布局版本；被同一 Batch 抵消的改名或子图切换仍只算属性修改。
fn same_topology(
    before: &Value,
    before_ids: &GraphIdentity,
    after: &Value,
    after_ids: &GraphIdentity,
    root: bool,
) -> bool {
    if before_ids != after_ids || (root && before.get("entry_point") != after.get("entry_point")) {
        return false;
    }
    for ((before, after), identity) in array(before, "nodes")
        .iter()
        .zip(array(after, "nodes"))
        .zip(&before_ids.nodes)
    {
        if before.get("id") != after.get("id") {
            return false;
        }
        if let Some(child) = identity.child.as_deref()
            && !same_topology(before, child, after, child, false)
        {
            return false;
        }
    }
    array(before, "edges")
        .iter()
        .zip(array(after, "edges"))
        .all(|(before, after)| {
            before.get("from") == after.get("from") && before.get("to") == after.get("to")
        })
}

/// 多属性表单完成后再校验业务节点及引用，避免 type/必填属性分步写入时误拒绝事务。
fn validate_changed_nodes(
    before: &GraphState,
    after: &GraphState,
    targets: &BTreeSet<NodeKey>,
) -> Result<(), GraphEditError> {
    fn visit(
        raw: &Value,
        identity: &GraphIdentity,
        before: &GraphState,
        targets: &BTreeSet<NodeKey>,
    ) -> Result<(), GraphEditError> {
        if graph_location(&before.identity, identity.key).is_none() {
            validate_layer(raw)?;
        }
        let known = array(raw, "nodes")
            .iter()
            .filter_map(|node| node.get("id").and_then(Value::as_str))
            .collect();
        for (node, identity) in array(raw, "nodes").iter().zip(&identity.nodes) {
            let original = node_location(&before.identity, identity.key)
                .and_then(|(path, index)| node_at(graph_at(&before.config, &path)?, index));
            if original.is_none() || (targets.contains(&identity.key) && original != Some(node)) {
                validate_node(node)?;
                if original.is_none()
                    && let Some(reason) = inactive_subgraph_reason(node)
                {
                    return Err(structure_error(format!(
                        "新增节点不能携带未启用的子图：{reason}"
                    )));
                }
                super::graph_schema::validate_node(node)
                    .map_err(GraphEditError::InvalidProperty)?;
                if node.get("type").and_then(Value::as_str) == Some("condition") {
                    for field in ["if_true", "if_false"] {
                        if let Some(target) = node.get(field) {
                            validate_reference(target, &known, field)?;
                        }
                    }
                }
            }
            if let Some(child) = identity.child.as_deref() {
                visit(node, child, before, targets)?;
            }
        }
        Ok(())
    }
    visit(&after.config, &after.identity, before, targets)
}

fn entry_problems(
    raw: &Value,
    identity: &GraphIdentity,
    root: bool,
    path: &str,
    output: &mut BTreeMap<(GraphKey, &'static str), String>,
) {
    let nodes = array(raw, "nodes");
    let actual_target = |id: &str| unique_node_index(raw, id).is_some();
    let body_target = |id: &str| id == EXIT_ID || actual_target(id);
    let problem = match (root, raw.get("type").and_then(Value::as_str)) {
        (true, _) if nodes.is_empty() => {
            output.insert(
                (identity.key, "empty"),
                format!("{path} 根流程没有节点，请添加实际执行节点"),
            );
            None
        }
        (true, _) => {
            let valid = match raw.get("entry_point") {
                Some(entry) => entry.as_str().is_some_and(actual_target),
                None => array(raw, "edges")
                    .iter()
                    .find_map(|edge| {
                        let from = edge.get("from").and_then(Value::as_str)?;
                        let to = edge.get("to").and_then(Value::as_str)?;
                        (from == ENTRY_ID && to != EXIT_ID).then_some(to)
                    })
                    .is_some_and(actual_target),
            };
            (!valid).then_some("缺少有效入口，请连接 _entry 到实际节点，或设置有效 entry_point")
        }
        (false, Some("sequence" | "loop")) => {
            let valid = array(raw, "edges").iter().any(|edge| {
                edge.get("from").and_then(Value::as_str) == Some(ENTRY_ID)
                    && edge.get("label").is_none()
                    && edge
                        .get("to")
                        .and_then(Value::as_str)
                        .is_some_and(body_target)
            });
            (!valid).then_some("缺少不带 label 字段的 _entry 连接；空子图可连接 _entry 到 _exit")
        }
        (false, Some("condition")) => {
            let valid = array(raw, "edges").iter().any(|edge| {
                edge.get("from").and_then(Value::as_str) == Some(ENTRY_ID)
                    && edge
                        .get("to")
                        .and_then(Value::as_str)
                        .is_some_and(body_target)
            });
            (!valid).then_some("条件子图缺少 _entry 连接；空子图可用 default 分支连接到 _exit")
        }
        _ => None,
    };
    if let Some(problem) = problem {
        output.insert((identity.key, "entry"), format!("{path} {problem}"));
    }
    for (index, (node, node_identity)) in nodes.iter().zip(&identity.nodes).enumerate() {
        if let Some(child) = node_identity.child.as_deref() {
            entry_problems(
                node,
                child,
                false,
                &format!("{path}.{}[{index}]", node_array_key(raw)),
                output,
            );
        }
    }
}

fn allocate(next: &mut u64) -> u64 {
    let value = *next;
    *next += 1;
    value
}

fn build_identity(raw: &Value, next: &mut u64) -> GraphIdentity {
    GraphIdentity {
        key: GraphKey(allocate(next)),
        nodes: array(raw, "nodes")
            .iter()
            .map(|node| build_node_identity(node, next))
            .collect(),
        edges: array(raw, "edges")
            .iter()
            .map(|_| EdgeKey(allocate(next)))
            .collect(),
    }
}

fn build_node_identity(node: &Value, next: &mut u64) -> NodeIdentity {
    NodeIdentity {
        key: NodeKey(allocate(next)),
        child: has_subgraph(node).then(|| Box::new(build_identity(node, next))),
    }
}

/// parallel 显式声明 branches 时执行器优先使用它，其他节点沿用 nodes。
/// 即使 branches 是 null，也不能回退到可能已被执行器忽略的 nodes。
pub(super) fn node_array_key(container: &Value) -> &'static str {
    match (
        container.get("type").and_then(Value::as_str),
        container.get("branches"),
    ) {
        (Some("parallel"), Some(_)) => "branches",
        _ => "nodes",
    }
}

pub(super) fn has_subgraph(node: &Value) -> bool {
    match node.get("type").and_then(Value::as_str) {
        Some("sequence" | "loop" | "parallel") => true,
        Some("condition") => node.get("nodes").is_some() && node.get("edges").is_some(),
        Some(kind) if known_leaf(kind) => false,
        _ => node.get(node_array_key(node)).is_some() || node.get("edges").is_some(),
    }
}

fn known_leaf(kind: &str) -> bool {
    super::graph_schema::node_types()
        .iter()
        .any(|definition| definition.type_id == kind && !definition.container)
}

/// 未启用的同名扩展留在原节点中，只在父层提示，不能创建可编辑图层。
pub(super) fn inactive_subgraph_reason(node: &Value) -> Option<String> {
    if has_subgraph(node)
        || !["nodes", "edges", "branches"]
            .iter()
            .any(|key| node.get(*key).is_some())
    {
        return None;
    }
    match node.get("type").and_then(Value::as_str) {
        Some("condition") => Some(
            "condition 的 nodes 与 edges 需同时声明；执行器按叶节点处理，内联内容未启用，原始字段保留"
                .into(),
        ),
        Some(kind) if known_leaf(kind) => Some(format!(
            "类型 {kind} 不执行内联子图；内联内容未启用，同名原始字段保留"
        )),
        _ => None,
    }
}

/// 专用容器的必要数组与 node_factory 实际分派一致；根图仍允许既有缺省文档。
pub(super) fn required_subgraph_arrays(node: &Value) -> &'static [&'static str] {
    match node.get("type").and_then(Value::as_str) {
        Some("sequence" | "loop") => &["nodes", "edges"],
        Some("condition") if has_subgraph(node) => &["nodes", "edges"],
        Some("parallel") if node_array_key(node) == "branches" => &["branches"],
        Some("parallel") => &["nodes"],
        _ => &[],
    }
}

fn node_at(container: &Value, index: usize) -> Option<&Value> {
    container.get(node_array_key(container))?.get(index)
}

fn node_at_mut(container: &mut Value, index: usize) -> Option<&mut Value> {
    let field = node_array_key(container);
    container.get_mut(field)?.get_mut(index)
}

fn array<'a>(raw: &'a Value, key: &str) -> &'a [Value] {
    let key = match key {
        "nodes" => node_array_key(raw),
        _ => key,
    };
    raw.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn unique_node_index(raw: &Value, id: &str) -> Option<usize> {
    let mut matching = array(raw, "nodes")
        .iter()
        .enumerate()
        .filter(|(_, node)| node.get("id").and_then(Value::as_str) == Some(id));
    let (index, _) = matching.next()?;
    matching.next().is_none().then_some(index)
}

fn graph_at<'a>(mut raw: &'a Value, path: &[usize]) -> Option<&'a Value> {
    for index in path {
        raw = node_at(raw, *index)?;
    }
    Some(raw)
}

fn graph_at_mut<'a>(mut raw: &'a mut Value, path: &[usize]) -> Option<&'a mut Value> {
    for index in path {
        raw = node_at_mut(raw, *index)?;
    }
    Some(raw)
}

fn identity_at<'a>(mut identity: &'a GraphIdentity, path: &[usize]) -> Option<&'a GraphIdentity> {
    for index in path {
        identity = identity.nodes.get(*index)?.child.as_deref()?;
    }
    Some(identity)
}

fn identity_at_mut<'a>(
    mut identity: &'a mut GraphIdentity,
    path: &[usize],
) -> Option<&'a mut GraphIdentity> {
    for index in path {
        identity = identity.nodes.get_mut(*index)?.child.as_deref_mut()?;
    }
    Some(identity)
}

fn graph_location(identity: &GraphIdentity, key: GraphKey) -> Option<Vec<usize>> {
    if identity.key == key {
        return Some(Vec::new());
    }
    for (index, node) in identity.nodes.iter().enumerate() {
        if let Some(mut path) = node
            .child
            .as_deref()
            .and_then(|child| graph_location(child, key))
        {
            path.insert(0, index);
            return Some(path);
        }
    }
    None
}

fn node_location(identity: &GraphIdentity, key: NodeKey) -> Option<(Vec<usize>, usize)> {
    for (index, node) in identity.nodes.iter().enumerate() {
        if node.key == key {
            return Some((Vec::new(), index));
        }
        if let Some((mut path, slot)) = node
            .child
            .as_deref()
            .and_then(|child| node_location(child, key))
        {
            path.insert(0, index);
            return Some((path, slot));
        }
    }
    None
}

fn edge_location(identity: &GraphIdentity, key: EdgeKey) -> Option<(Vec<usize>, usize)> {
    if let Some(index) = identity.edges.iter().position(|edge| *edge == key) {
        return Some((Vec::new(), index));
    }
    for (index, node) in identity.nodes.iter().enumerate() {
        if let Some((mut path, slot)) = node
            .child
            .as_deref()
            .and_then(|child| edge_location(child, key))
        {
            path.insert(0, index);
            return Some((path, slot));
        }
    }
    None
}

fn structure_error(message: impl Into<String>) -> GraphEditError {
    GraphEditError::InvalidStructure(message.into())
}

fn validate_id(id: &str) -> Result<(), GraphEditError> {
    match id.trim().is_empty() || matches!(id, ENTRY_ID | EXIT_ID) {
        true => Err(structure_error(
            "节点 ID 不能为空、只有空白或使用 _entry/_exit",
        )),
        false => Ok(()),
    }
}

fn validate_node(node: &Value) -> Result<(), GraphEditError> {
    let node = node
        .as_object()
        .ok_or_else(|| structure_error("节点必须是 JSON 对象"))?;
    let id = node
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| structure_error("节点缺少字符串 ID"))?;
    validate_id(id)?;
    match node.get("type").and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => (),
        _ => {
            return Err(structure_error(format!(
                "节点 {id:?} 的 type 必须是非空字符串"
            )));
        }
    }
    if let Some(value) = node.get("checkpoint") {
        let valid = match value {
            Value::Bool(_) => true,
            Value::Object(fields)
                if node.get("type").and_then(Value::as_str) == Some("subtask") =>
            {
                fields.get("enabled").is_none_or(Value::is_boolean)
            }
            _ => false,
        };
        if !valid {
            return Err(structure_error(format!(
                "节点 {id:?} 的 checkpoint 必须是布尔值；subtask 还支持含 enabled 布尔值的对象"
            )));
        }
    }
    Ok(())
}

fn validate_layer(raw: &Value) -> Result<(), GraphEditError> {
    if !raw.is_object() {
        return Err(structure_error("图定义必须是 JSON 对象"));
    }
    for key in required_subgraph_arrays(raw) {
        if raw.get(*key).is_none() {
            return Err(structure_error(format!("容器缺少必要的 {key} 数组")));
        }
    }
    for key in [node_array_key(raw), "edges"] {
        if raw.get(key).is_some_and(|value| !value.is_array()) {
            return Err(structure_error(format!("{key} 必须是数组")));
        }
    }
    let mut known = BTreeSet::from([ENTRY_ID, EXIT_ID]);
    for node in array(raw, "nodes") {
        validate_node(node)?;
        let id = node["id"].as_str().ok_or(GraphEditError::MissingTarget)?;
        if !known.insert(id) {
            return Err(structure_error(format!("节点 ID {id:?} 在本层重复")));
        }
    }
    for edge in array(raw, "edges") {
        validate_edge(edge, &known)?;
    }
    // GraphDocument 根工作副本不含 type。内联容器不读取 entry_point，不能把
    // 它们已有的同名扩展当成执行引用，阻止与入口无关的编辑或类型切换。
    if raw.get("type").is_none()
        && let Some(entry) = raw.get("entry_point")
    {
        validate_reference(entry, &known, "entry_point")?;
    }
    for node in array(raw, "nodes") {
        if node.get("type").and_then(Value::as_str) == Some("condition") {
            for field in ["if_true", "if_false"] {
                if let Some(target) = node.get(field) {
                    validate_reference(target, &known, field)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_tree(raw: &Value) -> Result<(), GraphEditError> {
    validate_layer(raw)?;
    for node in array(raw, "nodes") {
        if has_subgraph(node) {
            validate_tree(node)?;
        }
    }
    Ok(())
}

fn validate_edge(edge: &Value, known: &BTreeSet<&str>) -> Result<(), GraphEditError> {
    if !edge.is_object() {
        return Err(structure_error("连接必须是 JSON 对象"));
    }
    validate_edge_property(edge, "condition")?;
    validate_edge_property(edge, "priority")?;
    validate_edge_property(edge, "label")?;
    for key in ["from", "to"] {
        let endpoint = edge
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| structure_error(format!("连接 {key} 必须是字符串")))?;
        if !known.contains(endpoint) {
            return Err(structure_error(format!(
                "连接端点 {endpoint:?} 在本层不存在"
            )));
        }
        if matches!((key, endpoint), ("from", EXIT_ID) | ("to", ENTRY_ID)) {
            return Err(structure_error("_entry 只接受出边，_exit 只接受入边"));
        }
    }
    Ok(())
}

fn validate_reference(
    value: &Value,
    known: &BTreeSet<&str>,
    field: &str,
) -> Result<(), GraphEditError> {
    let id = value
        .as_str()
        .ok_or_else(|| structure_error(format!("{field} 必须是节点 ID 字符串")))?;
    if matches!(id, ENTRY_ID | EXIT_ID) || !known.contains(id) {
        return Err(structure_error(format!(
            "{field} 引用的节点 {id:?} 在本层不存在"
        )));
    }
    Ok(())
}

fn validate_edge_property(edge: &Value, field: &str) -> Result<(), GraphEditError> {
    let Some(value) = edge.get(field) else {
        return Ok(());
    };
    match field {
        "condition" | "label" if !value.is_string() => Err(GraphEditError::InvalidProperty(
            format!("连接 {field} 必须是字符串"),
        )),
        "priority"
            if !value
                .as_i64()
                .is_some_and(|priority| i32::try_from(priority).is_ok()) =>
        {
            Err(GraphEditError::InvalidProperty(
                "连接 priority 必须是 32 位整数".into(),
            ))
        }
        _ => Ok(()),
    }
}

/// condition 容器按 priority 降序、目标 ID 字典序检查入口分支。
/// 改名影响顺序时，用独立优先级保留原检查次序，绝不改写条件表达式。
fn preserve_condition_order(
    raw: &Value,
    old: &str,
    new: &str,
) -> Result<Vec<(usize, i32)>, GraphEditError> {
    if raw.get("type").and_then(Value::as_str) != Some("condition") {
        return Ok(Vec::new());
    }
    let mut conditional = Vec::new();
    let mut defaults = Vec::new();
    for (index, edge) in array(raw, "edges").iter().enumerate() {
        if edge.get("from").and_then(Value::as_str) != Some(ENTRY_ID) {
            continue;
        }
        validate_edge_property(edge, "condition")?;
        validate_edge_property(edge, "priority")?;
        let target = edge
            .get("to")
            .and_then(Value::as_str)
            .ok_or(GraphEditError::MissingTarget)?;
        let priority = edge.get("priority").and_then(Value::as_i64).unwrap_or(0);
        match edge
            .get("condition")
            .and_then(Value::as_str)
            .unwrap_or("default")
        {
            "default" => defaults.push((index, target)),
            _ => conditional.push((index, priority, target)),
        }
    }
    let renamed = |target| if target == old { new } else { target };
    let mut default_after = defaults.clone();
    defaults.sort_by_key(|(_, target)| *target);
    default_after.sort_by_key(|(_, target)| renamed(*target));
    if defaults
        .iter()
        .map(|(index, _)| index)
        .ne(default_after.iter().map(|(index, _)| index))
    {
        return Err(structure_error(
            "改名会改变多个 default 入口的选择次序；请先整理默认分支",
        ));
    }
    let mut after = conditional.clone();
    conditional.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(b.2)));
    after.sort_by(|a, b| b.1.cmp(&a.1).then(renamed(a.2).cmp(renamed(b.2))));
    if conditional
        .iter()
        .map(|edge| edge.0)
        .eq(after.iter().map(|edge| edge.0))
    {
        return Ok(Vec::new());
    }
    if conditional
        .windows(2)
        .any(|pair| pair[0].1 == pair[1].1 && pair[0].2 == pair[1].2)
    {
        return Err(structure_error(
            "同目标且同 priority 的入口分支顺序不明确；请先设置不同 priority",
        ));
    }
    let count = i32::try_from(conditional.len())
        .map_err(|_| structure_error("入口分支过多，无法分配 priority"))?;
    Ok(conditional
        .into_iter()
        .zip((1..=count).rev())
        .map(|((index, _, _), priority)| (index, priority))
        .collect())
}

fn editable_layer<'a>(state: &'a GraphState, path: &[usize]) -> Result<&'a Value, GraphEditError> {
    let raw = graph_at(&state.config, path).ok_or(GraphEditError::MissingTarget)?;
    validate_layer(raw)?;
    Ok(raw)
}

fn array_mut<'a>(raw: &'a mut Value, key: &str) -> Result<&'a mut Vec<Value>, GraphEditError> {
    let key = match key {
        "nodes" => node_array_key(raw),
        _ => key,
    };
    raw.as_object_mut()
        .ok_or(GraphEditError::MissingTarget)?
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| structure_error(format!("{key} 必须是数组")))
}

fn set_property(
    raw: &mut Value,
    property: String,
    value: Option<Value>,
) -> Result<(), GraphEditError> {
    let map = raw.as_object_mut().ok_or(GraphEditError::MissingTarget)?;
    match value {
        Some(value) => {
            map.insert(property, value);
        }
        None => {
            map.remove(&property);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
