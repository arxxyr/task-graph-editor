//! 流程图宽松读取与逐层结构诊断；原始 JSON 始终由父模块保留。

use std::collections::HashMap;

use serde_json::Value;

use super::graph_edit::{
    has_subgraph, inactive_subgraph_reason, node_array_key, required_subgraph_arrays,
};
use super::{ENTRY_ID, EXIT_ID, GraphDiagnostic, GraphEdge, SubGraph, TaskNode};

fn diagnose(diagnostics: &mut Vec<GraphDiagnostic>, path: String, message: impl Into<String>) {
    diagnostics.push(GraphDiagnostic {
        path,
        message: message.into(),
    });
}

/// 字段缺省兼容不含流程定义的文档；显式提供非数组值必须报告。
fn array_field<'a>(
    container: &'a Value,
    key: &str,
    path: &str,
    diagnostics: &mut Vec<GraphDiagnostic>,
) -> &'a [Value] {
    match container.get(key) {
        Some(Value::Array(values)) => values,
        Some(_) => {
            diagnose(diagnostics, format!("{path}.{key}"), "应为 JSON 数组");
            &[]
        }
        None => &[],
    }
}

fn string_field<'a>(
    container: &'a Value,
    key: &str,
    path: &str,
    diagnostics: &mut Vec<GraphDiagnostic>,
) -> Option<&'a str> {
    match container.get(key) {
        Some(Value::String(value)) => {
            if value.trim().is_empty() {
                diagnose(
                    diagnostics,
                    format!("{path}.{key}"),
                    "字符串不能为空或只有空白",
                );
            }
            Some(value)
        }
        Some(_) => {
            diagnose(diagnostics, format!("{path}.{key}"), "应为字符串");
            None
        }
        None => {
            diagnose(diagnostics, format!("{path}.{key}"), "缺少必要字符串字段");
            None
        }
    }
}

/// 无法建立节点身份时，将其子图诊断移到可见祖先，避免诊断跟着节点消失。
fn retain_unreachable_diagnostics(graph: SubGraph, diagnostics: &mut Vec<GraphDiagnostic>) {
    diagnostics.extend(graph.diagnostics);
    for node in graph.nodes {
        if let Some(children) = node.children {
            retain_unreachable_diagnostics(children, diagnostics);
        }
    }
}

fn parse_node(
    value: &Value,
    path: &str,
    diagnostics: &mut Vec<GraphDiagnostic>,
) -> Option<TaskNode> {
    if !value.is_object() {
        diagnose(
            diagnostics,
            path.to_string(),
            "节点应为 JSON 对象，无法展示此条目",
        );
        return None;
    }

    let id = string_field(value, "id", path, diagnostics);
    let node_type = string_field(value, "type", path, diagnostics)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown");
    // 活跃容器即使数组错误也保留诊断入口；执行器忽略的内联键只在父层报告。
    if let Some(reason) = inactive_subgraph_reason(value) {
        diagnose(diagnostics, path.to_owned(), reason);
    }
    let children = has_subgraph(value).then(|| parse_subgraph(value, path));
    let Some(id) = id else {
        if let Some(children) = children {
            retain_unreachable_diagnostics(children, diagnostics);
        }
        return None;
    };

    if matches!(id, ENTRY_ID | EXIT_ID) {
        diagnose(
            diagnostics,
            format!("{path}.id"),
            format!("节点 ID {id:?} 是虚拟入口或出口的保留名称"),
        );
    }

    Some(TaskNode {
        id: id.to_string(),
        node_type: node_type.to_string(),
        inputs: value.get("inputs").cloned().unwrap_or(Value::Null),
        checkpoint: match value.get("checkpoint") {
            Some(Value::Bool(enabled)) => *enabled,
            Some(Value::Object(fields)) if node_type == "subtask" => fields
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            _ => false,
        },
        children,
    })
}

fn parse_edge(
    value: &Value,
    path: &str,
    known: &HashMap<String, String>,
    diagnostics: &mut Vec<GraphDiagnostic>,
) -> Option<GraphEdge> {
    if !value.is_object() {
        diagnose(
            diagnostics,
            path.to_string(),
            "边应为 JSON 对象，无法展示此条目",
        );
        return None;
    }

    // 两端分别校验，避免首个字段错误遮蔽另一个字段的问题。
    let from = string_field(value, "from", path, diagnostics);
    let to = string_field(value, "to", path, diagnostics);
    for (key, endpoint) in [("from", from), ("to", to)] {
        if let Some(endpoint) = endpoint
            && !endpoint.trim().is_empty()
            && !matches!(endpoint, ENTRY_ID | EXIT_ID)
            && !known.contains_key(endpoint)
        {
            diagnose(
                diagnostics,
                format!("{path}.{key}"),
                format!("端点 {endpoint:?} 在本层不存在，此边无法绘制"),
            );
        }
    }
    let (Some(from), Some(to)) = (from, to) else {
        return None;
    };
    Some(GraphEdge {
        from: from.to_string(),
        to: to.to_string(),
    })
}

/// 按原文数组顺序建立只读模型，不丢弃重复 ID，也不改写未知业务字段。
pub(super) fn parse_subgraph(container: &Value, path: &str) -> SubGraph {
    let mut graph = SubGraph::default();
    if !container.is_object() {
        diagnose(
            &mut graph.diagnostics,
            path.to_string(),
            "图定义应为 JSON 对象",
        );
        graph.source_counts = Some((0, 0));
        return graph;
    }

    let node_field = node_array_key(container);
    for key in required_subgraph_arrays(container) {
        if container.get(*key).is_none() {
            diagnose(
                &mut graph.diagnostics,
                format!("{path}.{key}"),
                "容器缺少必要数组，执行器无法构建此子图",
            );
        }
    }
    let raw_nodes = array_field(container, node_field, path, &mut graph.diagnostics);
    let raw_edges = array_field(container, "edges", path, &mut graph.diagnostics);
    graph.source_counts = Some((raw_nodes.len(), raw_edges.len()));
    let mut first_paths = HashMap::new();
    for (index, value) in raw_nodes.iter().enumerate() {
        let node_path = format!("{path}.{node_field}[{index}]");
        if let Some(node) = parse_node(value, &node_path, &mut graph.diagnostics) {
            let id_path = format!("{node_path}.id");
            match first_paths.entry(node.id.clone()) {
                std::collections::hash_map::Entry::Occupied(first) => diagnose(
                    &mut graph.diagnostics,
                    id_path,
                    format!(
                        "节点 ID {:?} 在本层重复；首次定义位于 {}",
                        node.id,
                        first.get()
                    ),
                ),
                std::collections::hash_map::Entry::Vacant(first) => {
                    first.insert(id_path);
                }
            }
            graph.nodes.push(node);
        }
    }
    for (index, value) in raw_edges.iter().enumerate() {
        if let Some(edge) = parse_edge(
            value,
            &format!("{path}.edges[{index}]"),
            &first_paths,
            &mut graph.diagnostics,
        ) {
            graph.edges.push(edge);
        }
    }
    graph
}

#[cfg(test)]
mod tests;
