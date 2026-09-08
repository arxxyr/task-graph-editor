//! 图结构诊断与原始流程定义的无损保存回归。

use serde_json::{Value, json};

use super::parse_subgraph;
use crate::model::{ContextValue, parse_task_graph, serialize_task_graph};

fn paths(graph: &crate::model::SubGraph) -> Vec<&str> {
    graph
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.path.as_str())
        .collect()
}

#[test]
fn 非法节点身份逐项报告且重复节点不被合并() {
    let graph = parse_subgraph(
        &json!({
            "nodes": [
                {"type": "task"},
                {"id": 7, "type": "task"},
                {"id": "", "type": "task"},
                {"id": " \t", "type": "task"},
                {"id": "_entry", "type": "task"},
                {"id": "_exit", "type": "task"},
                {"id": "repeat", "type": "task"},
                {"id": "repeat", "type": "task"},
                null
            ],
            "edges": []
        }),
        "$.config",
    );

    assert_eq!(graph.source_counts, Some((9, 0)));
    assert_eq!(graph.nodes.len(), 6);
    assert_eq!(
        graph
            .nodes
            .iter()
            .filter(|node| node.id == "repeat")
            .count(),
        2
    );
    assert_eq!(
        paths(&graph),
        [
            "$.config.nodes[0].id",
            "$.config.nodes[1].id",
            "$.config.nodes[2].id",
            "$.config.nodes[3].id",
            "$.config.nodes[4].id",
            "$.config.nodes[5].id",
            "$.config.nodes[7].id",
            "$.config.nodes[8]"
        ]
    );
    assert!(
        graph.diagnostics[6]
            .message
            .contains("$.config.nodes[6].id")
    );
}

#[test]
fn 图层与节点边数组形状错误均有路径() {
    for invalid in [
        Value::Null,
        json!(true),
        json!(42),
        json!("错误"),
        json!([]),
    ] {
        let graph = parse_subgraph(&invalid, "$.config");
        assert_eq!(paths(&graph), ["$.config"]);
        assert_eq!(graph.diagnostic_count(), 1);
    }
    for invalid in [
        Value::Null,
        json!(true),
        json!(42),
        json!("错误"),
        json!({}),
    ] {
        let graph = parse_subgraph(
            &json!({"nodes": invalid.clone(), "edges": invalid}),
            "$.config",
        );
        assert_eq!(paths(&graph), ["$.config.nodes", "$.config.edges"]);
        assert_eq!(graph.source_counts, Some((0, 0)));
    }
}

#[test]
fn 非法子图保留诊断入口并区别合法空图与叶节点() {
    let graph = parse_subgraph(
        &json!({"nodes": [
            {"id": "bad", "type": "sequence", "nodes": null, "edges": []},
            {"id": "empty", "type": "sequence", "nodes": [], "edges": []},
            {"id": "leaf", "type": "task"},
            {"id": "edge_only", "type": "sequence", "nodes": [], "edges": false}
        ]}),
        "$.config",
    );

    assert!(graph.diagnostics.is_empty());
    let bad = graph.node("bad").unwrap().children.as_ref().unwrap();
    assert_eq!(paths(bad), ["$.config.nodes[0].nodes"]);
    let empty = graph.node("empty").unwrap().children.as_ref().unwrap();
    assert_eq!(empty.source_counts, Some((0, 0)));
    assert_eq!(empty.diagnostic_count(), 0);
    assert!(graph.node("leaf").unwrap().children.is_none());
    assert_eq!(
        paths(graph.node("edge_only").unwrap().children.as_ref().unwrap()),
        ["$.config.nodes[3].edges"]
    );
    assert_eq!(graph.diagnostic_count(), 2);
}

#[test]
fn 非法边同时校验两端并报告本层悬空端点() {
    let graph = parse_subgraph(
        &json!({
            "nodes": [
                {"id": "a", "type": "task"},
                {"id": "group", "type": "sequence", "nodes": [
                    {"id": "child", "type": "task"}
                ]}
            ],
            "edges": [
                {"to": 17},
                {"from": "ghost", "to": "child"},
                {"from": "", "to": " \n"},
                null,
                {"from": "_entry", "to": "a"},
                {"from": "a", "to": "_exit"}
            ]
        }),
        "$.config",
    );

    assert_eq!(graph.source_counts, Some((2, 6)));
    assert_eq!(graph.edges.len(), 4);
    assert_eq!(
        paths(&graph),
        [
            "$.config.edges[0].from",
            "$.config.edges[0].to",
            "$.config.edges[1].from",
            "$.config.edges[1].to",
            "$.config.edges[2].from",
            "$.config.edges[2].to",
            "$.config.edges[3]"
        ]
    );
}

#[test]
fn 不同层同名与平行边循环保持合法() {
    let graph = parse_subgraph(
        &json!({
            "nodes": [
                {"id": "same", "type": "sequence", "nodes": [
                    {"id": "same", "type": "task"}
                ], "edges": [
                    {"from": "_entry", "to": "same"},
                    {"from": "same", "to": "same"},
                    {"from": "same", "to": "_exit"}
                ]},
                {"id": "other", "type": "sequence", "nodes": [
                    {"id": "same", "type": "task"}
                ], "edges": []}
            ],
            "edges": [
                {"from": "same", "to": "other", "condition": true},
                {"from": "same", "to": "other", "condition": false},
                {"from": "other", "to": "same"}
            ]
        }),
        "$.config",
    );

    assert_eq!(graph.diagnostic_count(), 0);
    assert_eq!(graph.total_nodes(), 4);
    assert_eq!(graph.edges.len(), 3);
    assert_eq!(graph.edges[0], graph.edges[1]);
    let child = graph.subgraph_at(&["same".into()]).unwrap();
    assert_eq!(child.nodes[0].id, "same");
    assert_eq!(child.edges.len(), 3);
}

#[test]
fn 无法展示的父节点仍将深层诊断留在可见层() {
    let graph = parse_subgraph(
        &json!({"nodes": [
            {"type": "sequence", "nodes": [
                {"id": "child", "type": "sequence", "nodes": [
                    {"id": "child", "type": "sequence", "nodes": false, "edges": []}
                ], "edges": [
                    {"from": "child", "to": "ghost"}
                ]}
            ], "edges": []}
        ]}),
        "$.config",
    );

    assert!(graph.nodes.is_empty());
    assert_eq!(graph.diagnostic_count(), 3);
    assert_eq!(
        paths(&graph),
        [
            "$.config.nodes[0].id",
            "$.config.nodes[0].nodes[0].edges[0].to",
            "$.config.nodes[0].nodes[0].nodes[0].nodes"
        ]
    );
}

#[test]
fn 未知业务类型原样保留而缺失或非法类型有解释() {
    let graph = parse_subgraph(
        &json!({"nodes": [
            {"id": "missing"},
            {"id": "number", "type": 1},
            {"id": "blank", "type": " \t"},
            {"id": "future", "type": "robot_future_type", "inputs": [1, 2]}
        ]}),
        "$.config",
    );

    assert_eq!(
        paths(&graph),
        [
            "$.config.nodes[0].type",
            "$.config.nodes[1].type",
            "$.config.nodes[2].type"
        ]
    );
    assert_eq!(graph.node("missing").unwrap().node_type, "unknown");
    let future = graph.node("future").unwrap();
    assert_eq!(future.node_type, "robot_future_type");
    assert_eq!(future.inputs, json!([1, 2]));
}

#[test]
fn 修改上下文后未知节点边扩展和损坏结构保持原值() {
    let raw = json!({
        "map_id": "map", "task_id": "task", "document_extension": [1, {"flag": true}],
        "config": {
            "context": {"count": 1, "stringified": "[ 1, 2, 3 ]", "text": "  保留原文\n"},
            "graph_extension": {"version": "future"},
            "nodes": [
                {"id": "same", "type": "robot_future_type", "outputs": {"result": "${context.count}"},
                 "inputs": "  未知结构\n", "checkpoint": "未来形式", "custom": [1, null],
                 "nodes": [{"id": "same", "type": "task", "unknown": "{ \"a\": 1 }"}],
                 "edges": [{"from": "same", "to": "same", "port": "success", "weight": 1.0}]},
                {"id": "same", "type": "task", "nodes": null},
                {"type": "future", "extensions": {"binary": "00FF"}}
            ],
            "edges": [
                {"from": "same", "to": "ghost", "condition": "x > 0", "extension": [1, 2]},
                {"from": null, "to": false, "opaque": "保留损坏边"}
            ]
        }
    });
    let mut data = parse_task_graph(&raw.to_string()).unwrap();
    assert!(data.graph.diagnostic_count() >= 5);

    let unchanged: Value = serde_json::from_str(&serialize_task_graph(&data).unwrap()).unwrap();
    assert_eq!(unchanged, raw);
    data.context_fields
        .iter_mut()
        .find(|field| field.key == "count")
        .unwrap()
        .value = ContextValue::Integer(42);
    let saved: Value = serde_json::from_str(&serialize_task_graph(&data).unwrap()).unwrap();
    let mut expected = raw.clone();
    expected["config"]["context"]["count"] = json!(42);
    assert_eq!(saved, expected);
    assert_eq!(data.raw_json, raw);
    let reloaded = parse_task_graph(&saved.to_string()).unwrap();
    assert_eq!(reloaded.graph.diagnostics, data.graph.diagnostics);
    assert_eq!(
        reloaded.graph.diagnostic_count(),
        data.graph.diagnostic_count()
    );
}

#[test]
fn 未声明流程的上下文文档保持兼容() {
    for raw in [
        json!({"map_id": "map", "task_id": "task"}),
        json!({"map_id": "map", "task_id": "task", "config": {"context": {"number": 1}}}),
    ] {
        let data = parse_task_graph(&raw.to_string()).unwrap();
        assert!(data.graph.nodes.is_empty());
        assert!(data.graph.edges.is_empty());
        assert_eq!(data.graph.diagnostic_count(), 0);
        let saved: Value = serde_json::from_str(&serialize_task_graph(&data).unwrap()).unwrap();
        assert_eq!(saved, raw);
    }
}
