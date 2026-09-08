use super::*;
use crate::model::{ContextValue, TaskGraphData, parse_task_graph, serialize_task_graph};
use serde_json::json;

fn document() -> TaskGraphData {
    parse_task_graph(&json!({
        "map_id":"map", "task_id":"task", "unknown_root":{"keep":true},
        "config": {
            "context":{"position":"[0.10,0.20]", "count":3, "large":18446744073709551615_u64},
            "unknown_config":{"keep":[1,2.0]},
            "nodes":[
                {"id":"a","type":"custom_action","inputs":{"expression":"{count}","precise":0.9216510910864573},"unknown":{"large":18446744073709551615_u64}},
                {"id":"group","type":"sequence","nodes":[
                    {"id":"a","type":"nested_action","inputs":"保持原文 { a }"},
                    {"id":"b","type":"nested_action"}
                ],"edges":[{"from":"a","to":"b","nested_unknown":true}]},
                {"id":"b","type":"custom_action","checkpoint":false}
            ],
            "edges":[
                {"from":"_entry","to":"a"},
                {"from":"a","to":"b","unknown":"first"},
                {"from":"a","to":"b","unknown":"parallel"},
                {"from":"b","to":"_exit"}
            ]
        }
    }).to_string()).unwrap()
}

fn apply(data: &mut TaskGraphData, command: GraphCommand) -> bool {
    data.apply_graph_command(data.graph_edit.revision(), command)
        .unwrap()
}

fn root_node(data: &TaskGraphData, id: &str) -> NodeKey {
    data.graph_edit
        .node_key(data.graph_edit.root_key(), id)
        .unwrap()
}

fn saved(data: &TaskGraphData) -> Value {
    serde_json::from_str(&serialize_task_graph(data).unwrap()).unwrap()
}

#[test]
fn 属性事务保留完整未知值和context且保存重载更新投影() {
    let mut data = document();
    let original = data.raw_json.clone();
    let a = root_node(&data, "a");
    assert!(apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node: a,
            property: "inputs".into(),
            value: Some(
                json!({"expression":"{position}","literal":[1,2.0],"wide":18446744073709551615_u64})
            )
        }
    ));
    let output = saved(&data);
    assert_eq!(data.raw_json, original);
    assert_eq!(output["config"]["context"], original["config"]["context"]);
    assert_eq!(
        output["config"]["unknown_config"],
        original["config"]["unknown_config"]
    );
    assert_eq!(
        output["config"]["nodes"][0]["unknown"],
        original["config"]["nodes"][0]["unknown"]
    );
    assert_eq!(output["config"]["nodes"][1], original["config"]["nodes"][1]);
    assert_eq!(output["config"]["edges"], original["config"]["edges"]);
    assert_eq!(
        data.graph.nodes[0].inputs,
        output["config"]["nodes"][0]["inputs"]
    );
    assert_eq!(data.graph_edit.topology_revision(), 0);
    let loaded = parse_task_graph(&output.to_string()).unwrap();
    assert_eq!(loaded.graph.nodes[0].inputs, data.graph.nodes[0].inputs);
    assert!(!loaded.graph_edit.has_changes());
    assert!(!loaded.graph_edit.can_undo());
}

#[test]
fn 参数改动与图撤销互不覆盖且图回到基线清除变化标记() {
    let mut data = document();
    let a = root_node(&data, "a");
    apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node: a,
            property: "checkpoint".into(),
            value: Some(json!(true)),
        },
    );
    data.context_fields
        .iter_mut()
        .find(|field| field.key == "count")
        .unwrap()
        .value = ContextValue::Integer(9);
    data.map_id = "new map".into();
    assert!(data.graph_edit.has_changes());
    assert!(data.undo_graph(1).unwrap());
    assert!(!data.graph_edit.has_changes());
    assert_eq!(saved(&data)["config"]["context"]["count"], 9);
    assert_eq!(saved(&data)["map_id"], "new map");
    assert_eq!(data.graph_edit.revision(), 2);
    assert!(data.redo_graph(2).unwrap());
    assert_eq!(saved(&data)["config"]["context"]["count"], 9);
    assert!(data.graph.nodes[0].checkpoint);
}

#[test]
fn 无操作不污染历史且新事务清除重做但不复用身份() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    let a = root_node(&data, "a");
    assert!(!apply(
        &mut data,
        GraphCommand::RenameNode {
            node: a,
            id: "a".into()
        }
    ));
    assert!(!apply(
        &mut data,
        GraphCommand::DeleteNodes { nodes: vec![] }
    ));
    assert_eq!(data.graph_edit.revision(), 0);
    assert!(!data.graph_edit.can_undo());
    apply(
        &mut data,
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"c","type":"action"}),
        },
    );
    let c = root_node(&data, "c");
    data.undo_graph(1).unwrap();
    assert!(data.graph_edit.can_redo());
    apply(
        &mut data,
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"d","type":"action"}),
        },
    );
    let d = root_node(&data, "d");
    assert_ne!(c, d);
    assert!(!data.graph_edit.can_redo());
    assert!(data.graph_edit.node_json(c).is_none());
    assert_eq!(root_node(&data, "a"), a);
}

#[test]
fn 改名保留身份只更新同层端点跨层同名不受影响() {
    let mut data = document();
    let a = root_node(&data, "a");
    let child = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    let nested = data.graph_edit.node_key(child, "a").unwrap();
    let keys = data.graph_edit.edges(data.graph_edit.root_key());
    apply(
        &mut data,
        GraphCommand::RenameNode {
            node: a,
            id: "renamed".into(),
        },
    );
    assert_eq!(root_node(&data, "renamed"), a);
    assert_eq!(data.graph_edit.node_id(nested), Some("a"));
    assert_eq!(
        data.graph_edit.current_config()["nodes"][1]["edges"][0]["from"],
        "a"
    );
    let edges = data.graph_edit.edges(data.graph_edit.root_key());
    assert_eq!(
        edges.iter().map(|edge| edge.0).collect::<Vec<_>>(),
        keys.iter().map(|edge| edge.0).collect::<Vec<_>>()
    );
    assert_eq!(edges[0].2, "renamed");
    assert_eq!(edges[1].1, "renamed");
    assert_eq!(edges[2].1, "renamed");
    assert_eq!(data.graph_edit.topology_revision(), 1);
    data.undo_graph(1).unwrap();
    assert_eq!(root_node(&data, "a"), a);
}

#[test]
fn 复合节点改名后子图key与路径跟随且子节点key稳定() {
    let mut data = document();
    let group = root_node(&data, "group");
    let child = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    let nodes = data.graph_edit.nodes(child);
    apply(
        &mut data,
        GraphCommand::RenameNode {
            node: group,
            id: "renamed_group".into(),
        },
    );
    assert_eq!(
        data.graph_edit.graph_key_at(&["renamed_group".into()]),
        Some(child)
    );
    assert_eq!(
        data.graph_edit.graph_path(child),
        Some(vec!["renamed_group".into()])
    );
    assert_eq!(data.graph_edit.nodes(child), nodes);
}

#[test]
fn 删除复合子树和关联平行边是单次事务并完整撤销() {
    let mut data = document();
    let original = data.graph_edit.current_config().clone();
    let root = data.graph_edit.root_key();
    let nodes = data.graph_edit.nodes(root);
    let edges = data.graph_edit.edges(root);
    let child = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    let child_nodes = data.graph_edit.nodes(child);
    let a = root_node(&data, "a");
    let group = root_node(&data, "group");
    apply(
        &mut data,
        GraphCommand::DeleteNodes {
            nodes: vec![a, group, a],
        },
    );
    assert_eq!(data.graph_edit.nodes(root).len(), 1);
    assert_eq!(data.graph_edit.edges(root).len(), 1);
    assert!(data.graph_edit.graph_path(child).is_none());
    assert!(data.graph_edit.node_json(child_nodes[0].0).is_none());
    data.undo_graph(1).unwrap();
    assert_eq!(data.graph_edit.current_config(), &original);
    assert_eq!(data.graph_edit.nodes(root), nodes);
    assert_eq!(data.graph_edit.edges(root), edges);
    assert_eq!(data.graph_edit.nodes(child), child_nodes);
    data.redo_graph(2).unwrap();
    assert_eq!(data.graph_edit.nodes(root).len(), 1);
}

#[test]
fn 同时删除父子和多层节点不因数组位移误删() {
    let mut data = document();
    let before = data.graph_edit.current_config().clone();
    let group = root_node(&data, "group");
    let child = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    let nested = data.graph_edit.node_key(child, "a").unwrap();
    let a = root_node(&data, "a");
    apply(
        &mut data,
        GraphCommand::DeleteNodes {
            nodes: vec![a, group, nested],
        },
    );
    assert_eq!(
        data.graph_edit.current_config()["nodes"],
        json!([{"id":"b","type":"custom_action","checkpoint":false}])
    );
    data.undo_graph(1).unwrap();
    assert_eq!(data.graph_edit.current_config(), &before);
}

#[test]
fn 平行边分别重连编辑删除且撤销恢复原身份() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    let first = data.graph_edit.edge_key_at(root, 1).unwrap();
    let second = data.graph_edit.edge_key_at(root, 2).unwrap();
    assert_ne!(first, second);
    apply(
        &mut data,
        GraphCommand::ReconnectEdge {
            edge: second,
            from: "b".into(),
            to: "a".into(),
        },
    );
    assert_eq!(data.graph_edit.edge_json(first).unwrap()["from"], "a");
    assert_eq!(
        data.graph_edit.edge_json(second).unwrap()["unknown"],
        "parallel"
    );
    apply(
        &mut data,
        GraphCommand::SetEdgeProperty {
            edge: second,
            property: "description".into(),
            value: Some(json!("返回")),
        },
    );
    apply(&mut data, GraphCommand::DeleteEdges { edges: vec![first] });
    assert!(data.graph_edit.edge_json(first).is_none());
    assert_eq!(data.graph_edit.edge_key_at(root, 1), Some(second));
    data.undo_graph(3).unwrap();
    assert_eq!(data.graph_edit.edge_key_at(root, 1), Some(first));
    assert_eq!(data.graph_edit.edge_key_at(root, 2), Some(second));
}

#[test]
fn 新增子图自环回边和平行边均合法且保持数组顺序() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    let before = data.graph_edit.edges(root);
    for edge in [
        json!({"from":"a","to":"a"}),
        json!({"from":"b","to":"a"}),
        json!({"from":"a","to":"b"}),
    ] {
        apply(
            &mut data,
            GraphCommand::AddEdge {
                graph: root,
                value: edge,
            },
        );
    }
    assert_eq!(
        &data.graph_edit.edges(root)[..before.len()],
        before.as_slice()
    );
    let b = root_node(&data, "b");
    apply(
        &mut data,
        GraphCommand::SetSubgraph {
            node: b,
            value: Some(
                json!({"nodes":[{"id":"x","type":"new_business_kind","unknown":true}],"edges":[{"from":"x","to":"x"}]}),
            ),
        },
    );
    let child = data.graph_edit.graph_key_at(&["b".into()]).unwrap();
    let x = data.graph_edit.node_key(child, "x").unwrap();
    assert_eq!(data.graph_edit.node_json(x).unwrap()["unknown"], true);
    assert!(data.graph.nodes[2].children.is_some());
    apply(
        &mut data,
        GraphCommand::SetSubgraph {
            node: b,
            value: None,
        },
    );
    assert!(data.graph_edit.graph_path(child).is_none());
    data.undo_graph(5).unwrap();
    assert_eq!(data.graph_edit.node_key(child, "x"), Some(x));
}

#[test]
fn 原始坏结构保留诊断且context可存安全属性可修但结构操作拒绝() {
    let mut data=parse_task_graph(&json!({"map_id":"m","task_id":"t","config":{
        "context":{"count":1},"nodes":[{"id":"same","type":"action"},{"id":"same","type":"action"},17],
        "edges":[{"from":null,"to":"same"},{"from":"same","to":"ghost"}]
    }}).to_string()).unwrap();
    let before = data.raw_json.clone();
    data.context_fields[0].value = ContextValue::Integer(2);
    let output = saved(&data);
    assert_eq!(output["config"]["nodes"], before["config"]["nodes"]);
    assert_eq!(output["config"]["edges"], before["config"]["edges"]);
    assert_eq!(output["config"]["context"]["count"], 2);
    let root = data.graph_edit.root_key();
    assert!(data.graph_edit.node_key(root, "same").is_none());
    assert!(data.graph_edit.structure_error(root).is_some());
    assert_eq!(data.graph_edit.edges(root).len(), 1);
    let edge = data.graph_edit.edge_key_at(root, 0).unwrap();
    assert_eq!(data.graph_edit.edge_json(edge).unwrap()["to"], "ghost");
    let a = data.graph_edit.nodes(root)[0].0;
    apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node: a,
            property: "checkpoint".into(),
            value: Some(json!(true)),
        },
    );
    assert!(
        data.apply_graph_command(
            1,
            GraphCommand::AddNode {
                graph: root,
                value: json!({"id":"new","type":"action"})
            }
        )
        .is_err()
    );
    assert_eq!(data.graph_edit.revision(), 1);
}

#[test]
fn 无关坏子图不阻止有效图层编辑且保留其原始_json() {
    let mut data = parse_task_graph(
        &json!({"map_id":"m","task_id":"t","config":{"context":{},"nodes":[
            {"id":"bad","type":"sequence","nodes":null,"edges":[{"from":17,"to":"bad"}]},
            {"id":"ok","type":"action"}
        ]}})
        .to_string(),
    )
    .unwrap();
    let bad = data.raw_json["config"]["nodes"][0].clone();
    let root = data.graph_edit.root_key();
    assert!(data.graph_edit.structure_error(root).is_none());
    apply(
        &mut data,
        GraphCommand::AddEdge {
            graph: root,
            value: json!({"from":"ok","to":"bad"}),
        },
    );
    assert_eq!(saved(&data)["config"]["nodes"][0], bad);
    let child = data.graph_edit.graph_key_at(&["bad".into()]).unwrap();
    assert!(data.graph_edit.structure_error(child).is_some());
}

#[test]
fn 失败事务和过期预期修订不改变_json身份历史及版本() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    let a = root_node(&data, "a");
    let edge = data.graph_edit.edge_key_at(root, 1).unwrap();
    let before = data.graph_edit.current_config().clone();
    let before_nodes = data.graph_edit.nodes(root);
    let invalid = vec![
        GraphCommand::RenameNode {
            node: a,
            id: "b".into(),
        },
        GraphCommand::RenameNode {
            node: a,
            id: "_entry".into(),
        },
        GraphCommand::RenameNode {
            node: a,
            id: " \t".into(),
        },
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"new","type":""}),
        },
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"new","type":"action","checkpoint":"true"}),
        },
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"a","type":"action"}),
        },
        GraphCommand::AddEdge {
            graph: root,
            value: json!({"from":"ghost","to":"a"}),
        },
        GraphCommand::ReconnectEdge {
            edge,
            from: "ghost".into(),
            to: "a".into(),
        },
        GraphCommand::SetNodeProperty {
            node: a,
            property: "id".into(),
            value: Some(json!("changed")),
        },
        GraphCommand::SetNodeProperty {
            node: a,
            property: "nodes".into(),
            value: Some(json!([])),
        },
        GraphCommand::SetNodeProperty {
            node: a,
            property: "type".into(),
            value: None,
        },
        GraphCommand::SetNodeProperty {
            node: a,
            property: "checkpoint".into(),
            value: Some(json!(1)),
        },
        GraphCommand::SetEdgeProperty {
            edge,
            property: "from".into(),
            value: Some(json!("b")),
        },
        GraphCommand::SetSubgraph {
            node: a,
            value: Some(json!({"nodes":[{"id":"x","type":"action"},{"id":"x","type":"action"}]})),
        },
        GraphCommand::SetSubgraph {
            node: a,
            value: Some(json!({"nodes":null})),
        },
        GraphCommand::SetSubgraph {
            node: a,
            value: Some(json!({"type":"sequence","nodes":[]})),
        },
        GraphCommand::DeleteNodes {
            nodes: vec![a, NodeKey(u64::MAX)],
        },
        GraphCommand::DeleteEdges {
            edges: vec![edge, EdgeKey(u64::MAX)],
        },
    ];
    for command in invalid {
        assert!(data.apply_graph_command(0, command).is_err());
        assert_eq!(data.graph_edit.current_config(), &before);
        assert_eq!(data.graph_edit.nodes(root), before_nodes);
        assert_eq!(data.graph_edit.revision(), 0);
        assert_eq!(data.graph_edit.topology_revision(), 0);
        assert!(!data.graph_edit.can_undo());
        assert!(!data.graph_edit.can_redo());
    }
    let error = data
        .apply_graph_command(1, GraphCommand::DeleteNodes { nodes: vec![a] })
        .unwrap_err();
    assert_eq!(
        error,
        GraphEditError::StaleRevision {
            expected: 1,
            actual: 0
        }
    );
    assert!(data.undo_graph(1).is_err());
    assert!(data.redo_graph(1).is_err());
}

#[test]
fn 缺省数组不被无关属性操作补齐且撤销恢复键缺省() {
    let mut data =
        parse_task_graph(r#"{"map_id":"m","task_id":"t","config":{"context":{}}}"#).unwrap();
    let original = saved(&data);
    let root = data.graph_edit.root_key();
    apply(
        &mut data,
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"a","type":"action"}),
        },
    );
    assert!(data.graph_edit.current_config().get("edges").is_none());
    assert!(serialize_task_graph(&data).is_err());
    data.undo_graph(1).unwrap();
    assert_eq!(saved(&data), original);
    let mut no_config = parse_task_graph(r#"{"map_id":"m","task_id":"t"}"#).unwrap();
    assert!(saved(&no_config).get("config").is_none());
    let root = no_config.graph_edit.root_key();
    apply(
        &mut no_config,
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"a","type":"action"}),
        },
    );
    apply(
        &mut no_config,
        GraphCommand::AddEdge {
            graph: root,
            value: json!({"from":"_entry","to":"a"}),
        },
    );
    assert_eq!(saved(&no_config)["config"]["nodes"][0]["id"], "a");
}

#[test]
fn 多属性批量提交只生成一次历史且末项失败全部回滚() {
    let mut data = document();
    let a = root_node(&data, "a");
    let before = data.graph_edit.current_config().clone();
    let commands = vec![
        GraphCommand::SetNodeProperty {
            node: a,
            property: "checkpoint".into(),
            value: Some(json!(true)),
        },
        GraphCommand::SetNodeProperty {
            node: a,
            property: "inputs".into(),
            value: Some(json!(false)),
        },
        GraphCommand::RenameNode {
            node: a,
            id: "a2".into(),
        },
    ];
    assert!(apply(&mut data, GraphCommand::Batch(commands)));
    assert_eq!(data.graph_edit.revision(), 1);
    assert_eq!(data.graph_edit.topology_revision(), 1);
    assert_eq!(root_node(&data, "a2"), a);
    data.undo_graph(1).unwrap();
    assert_eq!(data.graph_edit.current_config(), &before);
    assert!(!data.graph_edit.can_undo());
    let failed = GraphCommand::Batch(vec![
        GraphCommand::SetNodeProperty {
            node: a,
            property: "inputs".into(),
            value: Some(json!([1, 2])),
        },
        GraphCommand::RenameNode {
            node: a,
            id: "b".into(),
        },
    ]);
    assert!(data.apply_graph_command(2, failed).is_err());
    assert_eq!(data.graph_edit.current_config(), &before);
    assert_eq!(data.graph_edit.revision(), 2);
    assert!(data.graph_edit.can_redo());
}

#[test]
fn 改名维护已确认执行引用但保留同名字串和context表达式() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    // 直接在输入夹具里声明根入口与条件分支，模拟执行器真实配置。
    let mut raw = data.raw_json.clone();
    raw["config"]["entry_point"] = json!("a");
    raw["config"]["nodes"][2] = json!({"id":"b","type":"condition","condition":"{a}","if_true":"a","if_false":"group","inputs":{"a":"a","literal":"a"}});
    data = parse_task_graph(&raw.to_string()).unwrap();
    let a = root_node(&data, "a");
    apply(
        &mut data,
        GraphCommand::RenameNode {
            node: a,
            id: "renamed".into(),
        },
    );
    let output = saved(&data);
    assert_eq!(output["config"]["entry_point"], "renamed");
    assert_eq!(output["config"]["nodes"][2]["if_true"], "renamed");
    assert_eq!(output["config"]["nodes"][2]["if_false"], "group");
    assert_eq!(output["config"]["nodes"][2]["condition"], "{a}");
    assert_eq!(
        output["config"]["nodes"][2]["inputs"],
        raw["config"]["nodes"][2]["inputs"]
    );
    assert_eq!(output["config"]["context"], raw["config"]["context"]);
    assert_eq!(data.graph_edit.root_key(), root);
    data.undo_graph(1).unwrap();
    assert_eq!(saved(&data), raw);
}

#[test]
fn 删除执行入口和条件关联引用可撤销且新增缺入口拒绝保存() {
    let mut raw = document().raw_json;
    raw["config"]["entry_point"] = json!("a");
    raw["config"]["nodes"][2] =
        json!({"id":"b","type":"condition","condition":"true","if_true":"a","if_false":"a"});
    let mut data = parse_task_graph(&raw.to_string()).unwrap();
    let a = root_node(&data, "a");
    apply(&mut data, GraphCommand::DeleteNodes { nodes: vec![a] });
    let config = data.graph_edit.current_config();
    assert!(config.get("entry_point").is_none());
    assert!(config["nodes"][1].get("if_true").is_none());
    assert!(config["nodes"][1].get("if_false").is_none());
    assert!(data.graph_edit.validate_for_save().is_err());
    assert!(serialize_task_graph(&data).is_err());
    assert!(data.graph_edit.can_undo());
    data.undo_graph(1).unwrap();
    assert_eq!(saved(&data), raw);
    data.redo_graph(2).unwrap();
    let root = data.graph_edit.root_key();
    apply(
        &mut data,
        GraphCommand::AddEdge {
            graph: root,
            value: json!({"from":"_entry","to":"b"}),
        },
    );
    assert!(serialize_task_graph(&data).is_ok());
}

fn condition_document(edges: Value) -> TaskGraphData {
    parse_task_graph(
        &json!({"map_id":"m","task_id":"t","config":{
            "context":{},"nodes":[{"id":"switch","type":"condition","nodes":[
                {"id":"a","type":"custom"},{"id":"b","type":"custom"},{"id":"z","type":"custom"}
            ],"edges":edges}],"edges":[{"from":"_entry","to":"switch"}]
        }})
        .to_string(),
    )
    .unwrap()
}

#[test]
fn 条件入口改名保持优先级字典排序语义并同事务撤销() {
    let mut data = condition_document(json!([
        {"from":"_entry","to":"b","condition":"{second}","unknown":"keep"},
        {"from":"_entry","to":"a","condition":"{first}","priority":0},
        {"from":"_entry","to":"z","condition":"default","priority":2147483647}
    ]));
    let before = data.graph_edit.current_config().clone();
    let graph = data.graph_edit.graph_key_at(&["switch".into()]).unwrap();
    let a = data.graph_edit.node_key(graph, "a").unwrap();
    let edges = data.graph_edit.edges(graph);
    apply(
        &mut data,
        GraphCommand::RenameNode {
            node: a,
            id: "c".into(),
        },
    );
    let raw = data.graph_edit.current_config();
    assert_eq!(raw["nodes"][0]["edges"][0]["priority"], 1);
    assert_eq!(raw["nodes"][0]["edges"][1]["priority"], 2);
    assert_eq!(raw["nodes"][0]["edges"][2]["priority"], 2147483647);
    assert_eq!(raw["nodes"][0]["edges"][0]["unknown"], "keep");
    assert_eq!(raw["nodes"][0]["edges"][1]["condition"], "{first}");
    assert_eq!(
        data.graph_edit
            .edges(graph)
            .iter()
            .map(|edge| edge.0)
            .collect::<Vec<_>>(),
        edges.iter().map(|edge| edge.0).collect::<Vec<_>>()
    );
    data.undo_graph(1).unwrap();
    assert_eq!(data.graph_edit.current_config(), &before);
}

#[test]
fn 不影响条件入口顺序的改名不补默认优先级字段() {
    let mut data = condition_document(json!([
        {"from":"_entry","to":"a","condition":"{first}"},
        {"from":"_entry","to":"b","condition":"{second}"}
    ]));
    let graph = data.graph_edit.graph_key_at(&["switch".into()]).unwrap();
    let b = data.graph_edit.node_key(graph, "b").unwrap();
    apply(
        &mut data,
        GraphCommand::RenameNode {
            node: b,
            id: "c".into(),
        },
    );
    for edge in data.graph_edit.current_config()["nodes"][0]["edges"]
        .as_array()
        .unwrap()
    {
        assert!(edge.get("priority").is_none());
    }
}

#[test]
fn 无法保持的多个默认分支或同优先级同目标歧义明确拒绝改名() {
    for edges in [
        json!([{"from":"_entry","to":"a"},{"from":"_entry","to":"b","condition":"default"}]),
        json!([{"from":"_entry","to":"a","condition":"{one}"},{"from":"_entry","to":"a","condition":"{two}"},{"from":"_entry","to":"b","condition":"{three}"}]),
    ] {
        let mut data = condition_document(edges);
        let before = data.graph_edit.current_config().clone();
        let graph = data.graph_edit.graph_key_at(&["switch".into()]).unwrap();
        let a = data.graph_edit.node_key(graph, "a").unwrap();
        assert!(
            data.apply_graph_command(
                0,
                GraphCommand::RenameNode {
                    node: a,
                    id: "c".into()
                }
            )
            .is_err()
        );
        assert_eq!(data.graph_edit.current_config(), &before);
        assert_eq!(data.graph_edit.revision(), 0);
    }
}

#[test]
fn 分支引用和边条件优先级属性不能绕过协议类型校验() {
    let mut data = condition_document(json!([{"from":"_entry","to":"a","condition":"true"}]));
    let graph = data.graph_edit.graph_key_at(&["switch".into()]).unwrap();
    let edge = data.graph_edit.edge_key_at(graph, 0).unwrap();
    for value in [json!(1.0), json!(2147483648_i64), json!("1"), json!(null)] {
        assert!(
            data.apply_graph_command(
                0,
                GraphCommand::SetEdgeProperty {
                    edge,
                    property: "priority".into(),
                    value: Some(value)
                }
            )
            .is_err()
        );
    }
    assert!(
        data.apply_graph_command(
            0,
            GraphCommand::SetEdgeProperty {
                edge,
                property: "condition".into(),
                value: Some(json!(true))
            }
        )
        .is_err()
    );
    let switch = root_node(&data, "switch");
    assert!(
        data.apply_graph_command(
            0,
            GraphCommand::SetNodeProperty {
                node: switch,
                property: "if_true".into(),
                value: Some(json!("ghost"))
            }
        )
        .is_err()
    );
    // 内联容器不读取 entry_point，同名扩展不参与根入口或分支引用校验。
    apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node: switch,
            property: "entry_point".into(),
            value: Some(json!("extension")),
        },
    );
    assert!(data.graph_edit.structure_error(graph).is_none());
    assert_eq!(data.graph_edit.topology_revision(), 0);
}

#[test]
fn 保存按执行器区别序列循环条件并行入口且允许空体直达出口() {
    for node_type in ["sequence", "loop", "condition", "parallel"] {
        let mut data = document();
        let root = data.graph_edit.root_key();
        apply(
            &mut data,
            GraphCommand::AddNode {
                graph: root,
                value: json!({"id":"new_group","type":node_type,"inputs":{"condition":"false"},"nodes":[],"edges":[]}),
            },
        );
        let needs_entry = node_type != "parallel";
        assert_eq!(
            data.graph_edit.validate_for_save().is_err(),
            needs_entry,
            "{node_type}"
        );
        if needs_entry {
            let graph = data.graph_edit.graph_key_at(&["new_group".into()]).unwrap();
            apply(
                &mut data,
                GraphCommand::AddEdge {
                    graph,
                    value: json!({"from":"_entry","to":"_exit"}),
                },
            );
            assert!(data.graph_edit.validate_for_save().is_ok(), "{node_type}");
        }
    }
}

#[test]
fn 空label不算序列普通入口且用户必须能删除这个字段() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    apply(
        &mut data,
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"new_group","type":"sequence","nodes":[],"edges":[{"from":"_entry","to":"_exit","label":""}]}),
        },
    );
    assert!(data.graph_edit.validate_for_save().is_err());
    let graph = data.graph_edit.graph_key_at(&["new_group".into()]).unwrap();
    let edge = data.graph_edit.edge_key_at(graph, 0).unwrap();
    apply(
        &mut data,
        GraphCommand::SetEdgeProperty {
            edge,
            property: "label".into(),
            value: None,
        },
    );
    assert!(data.graph_edit.validate_for_save().is_ok());
    assert!(
        data.graph_edit
            .edge_json(edge)
            .unwrap()
            .get("label")
            .is_none()
    );
}

#[test]
fn 原有缺入口子图在祖先改名数组移动后仍属于基线问题() {
    let mut data = document();
    let group = root_node(&data, "group");
    let child = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    assert!(data.graph_edit.validate_for_save().is_ok());
    apply(
        &mut data,
        GraphCommand::RenameNode {
            node: group,
            id: "renamed".into(),
        },
    );
    assert_eq!(
        data.graph_edit.graph_key_at(&["renamed".into()]),
        Some(child)
    );
    assert!(data.graph_edit.validate_for_save().is_ok());
    let a = root_node(&data, "a");
    let root = data.graph_edit.root_key();
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::DeleteNodes { nodes: vec![a] },
            GraphCommand::AddEdge {
                graph: root,
                value: json!({"from":"_entry","to":"renamed"}),
            },
        ]),
    );
    assert_eq!(
        data.graph_edit.graph_key_at(&["renamed".into()]),
        Some(child)
    );
    assert!(data.graph_edit.validate_for_save().is_ok());
}

#[test]
fn 直接修改已知属性保留未知字段并可恢复缺省() {
    let mut data = document();
    let a = root_node(&data, "a");
    let before = data.graph_edit.node_json(a).unwrap().clone();
    for inputs in [
        json!(null),
        json!(true),
        json!(17),
        json!(2.0),
        json!("{a}"),
        json!([1, "a"]),
    ] {
        apply(
            &mut data,
            GraphCommand::SetNodeProperty {
                node: a,
                property: "inputs".into(),
                value: Some(inputs.clone()),
            },
        );
        assert_eq!(data.graph_edit.node_json(a).unwrap()["inputs"], inputs);
        assert_eq!(
            data.graph_edit.node_json(a).unwrap()["unknown"],
            before["unknown"]
        );
    }
    apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node: a,
            property: "checkpoint".into(),
            value: Some(json!(true)),
        },
    );
    apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node: a,
            property: "checkpoint".into(),
            value: None,
        },
    );
    assert!(
        data.graph_edit
            .node_json(a)
            .unwrap()
            .get("checkpoint")
            .is_none()
    );
}

#[test]
fn 图属性_json解析保留整数边界且拒绝隐式浮点降级() {
    let raw = r#"{"min":-9223372036854775808,"max":18446744073709551615,"precise":0.9216510910864573,"integer":9007199254740993,"string":"\"18446744073709551616\""}"#;
    let value = parse_graph_value(raw).unwrap();
    assert_eq!(value["min"].as_i64(), Some(i64::MIN));
    assert_eq!(value["max"].as_u64(), Some(u64::MAX));
    assert_eq!(value["integer"].as_u64(), Some(9007199254740993));
    assert_eq!(value["precise"].as_f64(), Some(0.9216510910864573));
    for raw in [
        "18446744073709551616",
        "-9223372036854775809",
        "[0,{\"x\":18446744073709551616}]",
    ] {
        assert!(parse_graph_value(raw).is_err(), "{raw}");
    }
    assert!(parse_graph_value("[1.0,2e20,-0.0]").is_ok());
    assert!(parse_graph_value("{bad json}").is_err());
}

#[test]
fn 注册节点必填字段在完整批量事务后验证且失败不丢历史() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    assert!(
        data.apply_graph_command(
            0,
            GraphCommand::AddNode {
                graph: root,
                value: json!({"id":"new","type":"ros2_action","inputs":{}})
            }
        )
        .is_err()
    );
    let a = root_node(&data, "a");
    // type 先改为有必填字段的类型，随后在同一事务填入实际配置。
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::SetNodeProperty {
                node: a,
                property: "type".into(),
                value: Some(json!("condition")),
            },
            GraphCommand::SetNodeProperty {
                node: a,
                property: "inputs".into(),
                value: Some(json!({"condition":"{ready}"})),
            },
            GraphCommand::SetNodeProperty {
                node: a,
                property: "if_true".into(),
                value: Some(json!("b")),
            },
        ]),
    );
    assert_eq!(data.graph_edit.revision(), 1);
    let before = data.graph_edit.current_config().clone();
    assert!(
        data.apply_graph_command(
            1,
            GraphCommand::SetNodeProperty {
                node: a,
                property: "inputs".into(),
                value: Some(json!({}))
            }
        )
        .is_err()
    );
    assert_eq!(data.graph_edit.current_config(), &before);
    assert_eq!(data.graph_edit.revision(), 1);
    data.undo_graph(1).unwrap();
    assert_eq!(
        data.graph_edit.node_json(a).unwrap()["type"],
        "custom_action"
    );
}

#[test]
fn 类型切换不能激活悬空分支但可在同一事务修复引用() {
    let mut raw = document().raw_json;
    raw["config"]["nodes"][0]["if_true"] = json!("ghost");
    raw["config"]["nodes"][0]["inputs"] = json!({"condition":"{ready}"});
    let mut data = parse_task_graph(&raw.to_string()).unwrap();
    let a = root_node(&data, "a");
    assert!(
        data.apply_graph_command(
            0,
            GraphCommand::SetNodeProperty {
                node: a,
                property: "type".into(),
                value: Some(json!("condition"))
            }
        )
        .is_err()
    );
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::SetNodeProperty {
                node: a,
                property: "type".into(),
                value: Some(json!("condition")),
            },
            GraphCommand::SetNodeProperty {
                node: a,
                property: "if_true".into(),
                value: Some(json!("b")),
            },
        ]),
    );
    assert_eq!(data.graph_edit.node_json(a).unwrap()["if_true"], "b");
}

#[test]
fn 子任务checkpoint对象保留扩展且enabled类型有校验() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    apply(
        &mut data,
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"external","type":"subtask","source":"child.json","checkpoint":{"enabled":true,"unknown":"keep"}}),
        },
    );
    let node = root_node(&data, "external");
    apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node,
            property: "checkpoint".into(),
            value: Some(json!({"enabled":false,"unknown":"keep"})),
        },
    );
    assert_eq!(
        data.graph_edit.node_json(node).unwrap()["checkpoint"]["unknown"],
        "keep"
    );
    assert!(
        data.apply_graph_command(
            2,
            GraphCommand::SetNodeProperty {
                node,
                property: "checkpoint".into(),
                value: Some(json!({"enabled":1}))
            }
        )
        .is_err()
    );
    assert_eq!(
        saved(&data)["config"]["nodes"][3]["checkpoint"]["enabled"],
        false
    );
}

#[test]
fn 编辑禁止执行器不支持的反向虚拟端点但旧文件无关保存不受影响() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    for edge in [
        json!({"from":"a","to":"_entry"}),
        json!({"from":"_exit","to":"a"}),
    ] {
        assert!(
            data.apply_graph_command(
                0,
                GraphCommand::AddEdge {
                    graph: root,
                    value: edge
                }
            )
            .is_err()
        );
    }
    let mut raw = data.raw_json.clone();
    raw["config"]["edges"][0] = json!({"from":"_exit","to":"a"});
    let mut loaded = parse_task_graph(&raw.to_string()).unwrap();
    let a = root_node(&loaded, "a");
    apply(
        &mut loaded,
        GraphCommand::SetNodeProperty {
            node: a,
            property: "description".into(),
            value: Some(json!("查看原有异常")),
        },
    );
    assert_eq!(saved(&loaded)["config"]["edges"], raw["config"]["edges"]);
}

fn branches_document() -> TaskGraphData {
    parse_task_graph(&json!({"map_id":"m","task_id":"t","config":{
        "context":{"keep":2},"nodes":[{"id":"p","type":"parallel","inputs":{"policy":"all_success"},
            "nodes":[{"id":"shadowed","type":"custom","unknown":"保持被遮蔽的原值"}],
            "branches":[{"id":"a","type":"custom","unknown":18446744073709551615_u64},
                {"id":"group","type":"sequence","nodes":[{"id":"a","type":"custom"}],"edges":[{"from":"_entry","to":"a"},{"from":"a","to":"_exit"}]}],
            "edges":[]
        }],"edges":[{"from":"_entry","to":"p"}]
    }}).to_string()).unwrap()
}

#[test]
fn 并行branches优先展示且各层身份_json和路径均对应真实节点() {
    let data = branches_document();
    let graph = data.graph_edit.graph_key_at(&["p".into()]).unwrap();
    assert_eq!(
        data.graph_edit
            .nodes(graph)
            .iter()
            .map(|(_, id)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "group"]
    );
    assert!(data.graph_edit.node_key(graph, "shadowed").is_none());
    let a = data.graph_edit.node_key(graph, "a").unwrap();
    assert_eq!(
        data.graph_edit.node_json(a).unwrap()["unknown"].as_u64(),
        Some(u64::MAX)
    );
    let nested = data
        .graph_edit
        .graph_key_at(&["p".into(), "group".into()])
        .unwrap();
    let nested_a = data.graph_edit.node_key(nested, "a").unwrap();
    assert_ne!(a, nested_a);
    assert_eq!(
        data.graph_edit.graph_path(nested),
        Some(vec!["p".into(), "group".into()])
    );
    assert_eq!(
        data.graph_edit.graph_json(graph).unwrap()["type"],
        "parallel"
    );
    let projection = data.graph.nodes[0].children.as_ref().unwrap();
    assert_eq!(
        projection
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "group"]
    );
    assert_eq!(projection.source_counts, Some((2, 0)));
}

#[test]
fn 并行分支增删改写回原branches键且不触碰遮蔽nodes并可完整撤销() {
    let mut data = branches_document();
    let before = saved(&data);
    let graph = data.graph_edit.graph_key_at(&["p".into()]).unwrap();
    let a = data.graph_edit.node_key(graph, "a").unwrap();
    let group = data.graph_edit.node_key(graph, "group").unwrap();
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::RenameNode {
                node: a,
                id: "renamed".into(),
            },
            GraphCommand::AddNode {
                graph,
                value: json!({"id":"b","type":"custom"}),
            },
            GraphCommand::DeleteNodes { nodes: vec![group] },
        ]),
    );
    let output = saved(&data);
    assert_eq!(
        output["config"]["nodes"][0]["nodes"],
        before["config"]["nodes"][0]["nodes"]
    );
    assert_eq!(output["config"]["nodes"][0]["branches"][0]["id"], "renamed");
    assert_eq!(
        output["config"]["nodes"][0]["branches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(data.graph_edit.node_key(graph, "renamed"), Some(a));
    data.undo_graph(1).unwrap();
    assert_eq!(saved(&data), before);
    assert_eq!(data.graph_edit.node_key(graph, "group"), Some(group));
}

#[test]
fn 整体替换并行子图标准nodes入参写回branches并保留遮蔽数据() {
    let mut data = branches_document();
    let before = data.graph_edit.current_config().clone();
    let p = root_node(&data, "p");
    let original_graph = data.graph_edit.graph_key_at(&["p".into()]).unwrap();
    apply(
        &mut data,
        GraphCommand::SetSubgraph {
            node: p,
            value: Some(json!({"nodes":[{"id":"replacement","type":"custom"}],"edges":[]})),
        },
    );
    assert_eq!(
        data.graph_edit.node_json(p).unwrap()["nodes"],
        before["nodes"][0]["nodes"]
    );
    assert_eq!(
        data.graph_edit.node_json(p).unwrap()["branches"][0]["id"],
        "replacement"
    );
    assert!(data.graph_edit.graph_path(original_graph).is_none());
    data.undo_graph(1).unwrap();
    assert_eq!(
        data.graph_edit.graph_key_at(&["p".into()]),
        Some(original_graph)
    );
    assert_eq!(data.graph_edit.current_config(), &before);
}

#[test]
fn 显式branches子图输入有类型守卫且移除全部子图不会激活遮蔽节点() {
    let mut data = branches_document();
    let p = root_node(&data, "p");
    assert!(
        data.apply_graph_command(
            0,
            GraphCommand::SetNodeProperty {
                node: p,
                property: "branches".into(),
                value: Some(json!([]))
            }
        )
        .is_err()
    );
    assert!(
        data.apply_graph_command(
            0,
            GraphCommand::SetSubgraph {
                node: p,
                value: Some(json!({"branches":[],"nodes":[]}))
            }
        )
        .is_err()
    );
    apply(
        &mut data,
        GraphCommand::SetSubgraph {
            node: p,
            value: Some(json!({"branches":[],"edges":[]})),
        },
    );
    assert!(
        data.graph_edit.node_json(p).unwrap()["branches"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(data.graph_edit.node_json(p).unwrap().get("nodes").is_some());
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::SetSubgraph {
                node: p,
                value: None,
            },
            GraphCommand::SetNodeProperty {
                node: p,
                property: "type".into(),
                value: Some(json!("custom_leaf")),
            },
        ]),
    );
    let raw = data.graph_edit.node_json(p).unwrap();
    assert!(raw.get("branches").is_none());
    assert!(raw.get("nodes").is_none());
    assert!(data.graph.nodes[0].children.is_none());
    assert!(data.graph_edit.graph_key_at(&["p".into()]).is_none());
    assert!(
        data.apply_graph_command(
            2,
            GraphCommand::SetSubgraph {
                node: p,
                value: Some(json!({"branches":[]}))
            }
        )
        .is_err()
    );
}

#[test]
fn 非数组branches不能回退nodes且诊断路径使用真实源键() {
    let mut raw = branches_document().raw_json;
    raw["config"]["nodes"][0]["branches"] = Value::Null;
    let mut data = parse_task_graph(&raw.to_string()).unwrap();
    let child = data.graph.nodes[0].children.as_ref().unwrap();
    assert!(child.nodes.is_empty());
    assert!(
        child
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.path == "$.config.nodes[0].branches")
    );
    let graph = data.graph_edit.graph_key_at(&["p".into()]).unwrap();
    assert!(data.graph_edit.nodes(graph).is_empty());
    assert!(data.graph_edit.structure_error(graph).is_some());
    let p = root_node(&data, "p");
    apply(
        &mut data,
        GraphCommand::SetSubgraph {
            node: p,
            value: Some(json!({"nodes":[],"edges":[]})),
        },
    );
    assert_eq!(saved(&data)["config"]["nodes"][0]["branches"], json!([]));
    assert_eq!(
        saved(&data)["config"]["nodes"][0]["nodes"],
        raw["config"]["nodes"][0]["nodes"]
    );
}

#[test]
fn 切换节点类型导致数组来源改变时刷新子图身份并保留所有原始字段() {
    let mut data = branches_document();
    let p = root_node(&data, "p");
    let old_graph = data.graph_edit.graph_key_at(&["p".into()]).unwrap();
    let before = data.graph_edit.current_config().clone();
    apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node: p,
            property: "type".into(),
            value: Some(json!("sequence")),
        },
    );
    let new_graph = data.graph_edit.graph_key_at(&["p".into()]).unwrap();
    assert_ne!(old_graph, new_graph);
    assert_eq!(data.graph_edit.nodes(new_graph)[0].1, "shadowed");
    assert_eq!(
        data.graph_edit.node_json(p).unwrap()["branches"],
        before["nodes"][0]["branches"]
    );
    assert!(data.graph_edit.validate_for_save().is_err());
    data.undo_graph(1).unwrap();
    assert_eq!(data.graph_edit.graph_key_at(&["p".into()]), Some(old_graph));
    assert_eq!(data.graph_edit.current_config(), &before);
}

#[test]
fn 根配置未知type与branches不会遮蔽实际nodes且序列化保留原值() {
    let mut raw = document().raw_json;
    raw["config"]["type"] = json!("parallel");
    raw["config"]["branches"] = json!([{"id":"fake","type":"custom"}]);
    let mut data = parse_task_graph(&raw.to_string()).unwrap();
    assert_eq!(data.graph.nodes[0].id, "a");
    assert!(
        data.graph_edit
            .node_key(data.graph_edit.root_key(), "fake")
            .is_none()
    );
    let a = root_node(&data, "a");
    apply(
        &mut data,
        GraphCommand::RenameNode {
            node: a,
            id: "new".into(),
        },
    );
    assert_eq!(
        saved(&data)["config"]["branches"],
        raw["config"]["branches"]
    );
    assert_eq!(saved(&data)["config"]["type"], raw["config"]["type"]);
}

#[test]
fn 删除最后节点新增空根图错误且连接label只允许字符串或移除() {
    let mut data=parse_task_graph(r#"{"map_id":"m","task_id":"t","config":{"nodes":[{"id":"a","type":"custom"}],"edges":[{"from":"_entry","to":"a"}]}}"#).unwrap();
    let a = root_node(&data, "a");
    apply(&mut data, GraphCommand::DeleteNodes { nodes: vec![a] });
    assert!(serialize_task_graph(&data).is_err());
    data.undo_graph(1).unwrap();
    let root = data.graph_edit.root_key();
    let edge = data.graph_edit.edge_key_at(root, 0).unwrap();
    assert!(
        data.apply_graph_command(
            2,
            GraphCommand::SetEdgeProperty {
                edge,
                property: "label".into(),
                value: Some(json!([]))
            }
        )
        .is_err()
    );
    assert!(
        data.apply_graph_command(
            2,
            GraphCommand::AddEdge {
                graph: root,
                value: json!({"from":"a","to":"_exit","label":true})
            }
        )
        .is_err()
    );
}

#[test]
fn 设置根入口保持节点边身份且原子保存撤销恢复缺省() {
    let mut data = document();
    let original = saved(&data);
    let root = data.graph_edit.root_key();
    let nodes = data.graph_edit.nodes(root);
    let edges = data.graph_edit.edges(root);
    apply(
        &mut data,
        GraphCommand::SetEntryPoint {
            graph: root,
            id: Some("b".into()),
        },
    );
    let output = saved(&data);
    assert_eq!(output["config"]["entry_point"], "b");
    assert_eq!(output["config"]["nodes"], original["config"]["nodes"]);
    assert_eq!(output["config"]["edges"], original["config"]["edges"]);
    assert_eq!(output["config"]["context"], original["config"]["context"]);
    assert_eq!(data.graph_edit.nodes(root), nodes);
    assert_eq!(data.graph_edit.edges(root), edges);
    assert_eq!(data.graph_edit.revision(), 1);
    assert_eq!(data.graph_edit.topology_revision(), 1);
    assert!(!apply(
        &mut data,
        GraphCommand::SetEntryPoint {
            graph: root,
            id: Some("b".into())
        }
    ));
    assert_eq!(data.graph_edit.revision(), 1);
    data.undo_graph(1).unwrap();
    assert_eq!(saved(&data), original);
    assert!(!data.graph_edit.has_changes());
    data.redo_graph(2).unwrap();
    assert_eq!(saved(&data), output);
}

#[test]
fn 移除显式入口回退连线且无有效入口时保留可继续编辑的草稿() {
    let mut raw = document().raw_json;
    raw["config"]["entry_point"] = json!("b");
    let mut data = parse_task_graph(&raw.to_string()).unwrap();
    let root = data.graph_edit.root_key();
    apply(
        &mut data,
        GraphCommand::SetEntryPoint {
            graph: root,
            id: None,
        },
    );
    assert!(saved(&data)["config"].get("entry_point").is_none());
    assert_eq!(saved(&data)["config"]["edges"], raw["config"]["edges"]);
    data.undo_graph(1).unwrap();
    assert_eq!(saved(&data), raw);
    let entry = data.graph_edit.edge_key_at(root, 0).unwrap();
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::DeleteEdges { edges: vec![entry] },
            GraphCommand::SetEntryPoint {
                graph: root,
                id: None,
            },
        ]),
    );
    assert!(serialize_task_graph(&data).is_err());
    apply(
        &mut data,
        GraphCommand::SetEntryPoint {
            graph: root,
            id: Some("group".into()),
        },
    );
    assert_eq!(saved(&data)["config"]["entry_point"], "group");
}

#[test]
fn 根入口拒绝子图虚拟不存在及重复身份且失败无副作用() {
    let mut data = document();
    let original = data.graph_edit.current_config().clone();
    let root = data.graph_edit.root_key();
    let child = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    for command in [
        GraphCommand::SetEntryPoint {
            graph: child,
            id: Some("a".into()),
        },
        GraphCommand::SetEntryPoint {
            graph: child,
            id: None,
        },
        GraphCommand::SetEntryPoint {
            graph: GraphKey(u64::MAX),
            id: Some("a".into()),
        },
        GraphCommand::SetEntryPoint {
            graph: root,
            id: Some("_entry".into()),
        },
        GraphCommand::SetEntryPoint {
            graph: root,
            id: Some("_exit".into()),
        },
        GraphCommand::SetEntryPoint {
            graph: root,
            id: Some("ghost".into()),
        },
        GraphCommand::SetEntryPoint {
            graph: root,
            id: Some(" ".into()),
        },
    ] {
        assert!(data.apply_graph_command(0, command).is_err());
        assert_eq!(data.graph_edit.current_config(), &original);
        assert_eq!(data.graph_edit.revision(), 0);
        assert!(!data.graph_edit.can_undo());
    }
    let mut raw = data.raw_json.clone();
    raw["config"]["nodes"][1]["id"] = json!("a");
    let mut duplicate = parse_task_graph(&raw.to_string()).unwrap();
    assert!(
        duplicate
            .apply_graph_command(
                0,
                GraphCommand::SetEntryPoint {
                    graph: duplicate.graph_edit.root_key(),
                    id: Some("a".into())
                }
            )
            .is_err()
    );
}

#[test]
fn 既有无效显式入口可以直接修复且缺省移除无操作不产生历史() {
    let mut data = document();
    let root = data.graph_edit.root_key();
    assert!(!apply(
        &mut data,
        GraphCommand::SetEntryPoint {
            graph: root,
            id: None
        }
    ));
    assert_eq!(data.graph_edit.revision(), 0);
    let mut raw = data.raw_json.clone();
    raw["config"]["entry_point"] = json!("ghost");
    let mut invalid = parse_task_graph(&raw.to_string()).unwrap();
    let root = invalid.graph_edit.root_key();
    assert!(invalid.graph_edit.structure_error(root).is_some());
    apply(
        &mut invalid,
        GraphCommand::SetEntryPoint {
            graph: root,
            id: Some("a".into()),
        },
    );
    assert!(invalid.graph_edit.structure_error(root).is_none());
    assert_eq!(saved(&invalid)["config"]["entry_point"], "a");
    invalid.undo_graph(1).unwrap();
    assert_eq!(saved(&invalid), raw);
    apply(
        &mut invalid,
        GraphCommand::SetEntryPoint {
            graph: root,
            id: None,
        },
    );
    assert!(invalid.graph_edit.structure_error(root).is_none());
}

#[test]
fn 批量属性修改中的无操作子图替换保留整个子树身份和布局版本() {
    let mut data = document();
    let group = root_node(&data, "group");
    let a = root_node(&data, "a");
    let child = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    let children = data.graph_edit.nodes(child);
    let edges = data.graph_edit.edges(child);
    let node = data.graph_edit.node_json(group).unwrap();
    let unchanged = json!({"nodes":node["nodes"],"edges":node["edges"]});
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::SetSubgraph {
                node: group,
                value: Some(unchanged),
            },
            GraphCommand::SetNodeProperty {
                node: a,
                property: "description".into(),
                value: Some(json!("只改属性")),
            },
        ]),
    );
    assert_eq!(data.graph_edit.graph_key_at(&["group".into()]), Some(child));
    assert_eq!(data.graph_edit.nodes(child), children);
    assert_eq!(data.graph_edit.edges(child), edges);
    assert_eq!(data.graph_edit.topology_revision(), 0);
    assert_eq!(data.graph_edit.revision(), 1);
}

#[test]
fn 批量中类型数组来源切换又恢复不会刷新未变化的分支身份() {
    let mut data = branches_document();
    let p = root_node(&data, "p");
    let child = data.graph_edit.graph_key_at(&["p".into()]).unwrap();
    let children = data.graph_edit.nodes(child);
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::SetNodeProperty {
                node: p,
                property: "type".into(),
                value: Some(json!("sequence")),
            },
            GraphCommand::SetNodeProperty {
                node: p,
                property: "type".into(),
                value: Some(json!("parallel")),
            },
            GraphCommand::SetNodeProperty {
                node: p,
                property: "description".into(),
                value: Some(json!("最终只有属性修改")),
            },
        ]),
    );
    assert_eq!(data.graph_edit.graph_key_at(&["p".into()]), Some(child));
    assert_eq!(data.graph_edit.nodes(child), children);
    assert_eq!(data.graph_edit.topology_revision(), 0);
}

#[test]
fn 批量抵消改名但修改别的属性不会推进拓扑版本() {
    let mut data = document();
    let a = root_node(&data, "a");
    let root = data.graph_edit.root_key();
    let edges = data.graph_edit.edges(root);
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::RenameNode {
                node: a,
                id: "temporary".into(),
            },
            GraphCommand::RenameNode {
                node: a,
                id: "a".into(),
            },
            GraphCommand::SetNodeProperty {
                node: a,
                property: "description".into(),
                value: Some(json!("属性")),
            },
        ]),
    );
    assert_eq!(data.graph_edit.edges(root), edges);
    assert_eq!(data.graph_edit.topology_revision(), 0);
    data.undo_graph(1).unwrap();
    assert_eq!(data.graph_edit.topology_revision(), 0);
}

fn inactive_document() -> TaskGraphData {
    parse_task_graph(&json!({
        "map_id":"m", "task_id":"t", "config":{
            "context":{"count":1},
            "nodes":[
                {"id":"leaf","type":"log","inputs":{"message":"保留"},
                 "nodes":[{"id":"hidden","type":"custom","unknown":[1,2.0]}],
                 "edges":[{"from":"hidden","to":"hidden"}],"branches":{"extension":true},
                 "entry_point":{"not_a_reference":true}},
                {"id":"nodes_only","type":"condition","inputs":{"condition":"true"},"nodes":null},
                {"id":"edges_only","type":"condition","inputs":{"condition":"false"},"edges":[]}
            ],
            "edges":[{"from":"_entry","to":"leaf"}]
        }
    }).to_string()).unwrap()
}

#[test]
fn 已知叶节点和单键条件子图不生成活动身份且扩展与上下文保存保持原样() {
    let mut data = inactive_document();
    let before = data.raw_json.clone();
    assert_eq!(data.graph.total_nodes(), 3);
    assert_eq!(data.graph.diagnostic_count(), 3);
    for node in &data.graph.nodes {
        assert!(node.children.is_none());
        assert!(
            data.graph_edit
                .graph_key_at(std::slice::from_ref(&node.id))
                .is_none()
        );
    }
    assert!(
        data.graph
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.message.contains("未启用"))
    );
    data.context_fields[0].value = ContextValue::Integer(2);
    let output = saved(&data);
    assert_eq!(output["config"]["nodes"], before["config"]["nodes"]);
    assert_eq!(output["config"]["context"]["count"], 2);
    for id in ["leaf", "nodes_only", "edges_only"] {
        let node = root_node(&data, id);
        apply(
            &mut data,
            GraphCommand::SetNodeProperty {
                node,
                property: "description".into(),
                value: Some(json!("只修改说明")),
            },
        );
    }
    let output = saved(&data);
    for (index, original) in before["config"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let mut expected = original.clone();
        expected["description"] = json!("只修改说明");
        assert_eq!(output["config"]["nodes"][index], expected);
    }
    assert_eq!(data.graph_edit.topology_revision(), 0);
    assert_eq!(data.raw_json, before);
}

#[test]
fn 删除含未启用内联内容的叶节点完整撤销恢复原始字段和节点身份() {
    let mut data = inactive_document();
    let before = data.graph_edit.current_config().clone();
    let leaf = root_node(&data, "leaf");
    let edges = data.graph_edit.edges(data.graph_edit.root_key());
    apply(&mut data, GraphCommand::DeleteNodes { nodes: vec![leaf] });
    assert!(data.graph_edit.node_json(leaf).is_none());
    data.undo_graph(1).unwrap();
    assert_eq!(data.graph_edit.current_config(), &before);
    assert_eq!(root_node(&data, "leaf"), leaf);
    assert_eq!(data.graph_edit.edges(data.graph_edit.root_key()), edges);
    assert!(data.graph_edit.graph_key_at(&["leaf".into()]).is_none());
}

#[test]
fn 未启用子图不能被结构操作改写或删除且新节点不能携带假子图() {
    let mut data = inactive_document();
    let before = data.graph_edit.current_config().clone();
    let leaf = root_node(&data, "leaf");
    let partial = root_node(&data, "nodes_only");
    let root = data.graph_edit.root_key();
    for command in [
        GraphCommand::SetSubgraph {
            node: leaf,
            value: Some(json!({"nodes":[],"edges":[]})),
        },
        GraphCommand::SetSubgraph {
            node: leaf,
            value: None,
        },
        GraphCommand::SetSubgraph {
            node: partial,
            value: None,
        },
        GraphCommand::SetSubgraph {
            node: partial,
            value: Some(json!({"nodes":[]})),
        },
        GraphCommand::SetSubgraph {
            node: partial,
            value: Some(json!({"edges":[]})),
        },
        GraphCommand::SetSubgraph {
            node: partial,
            value: Some(json!({"nodes":[],"edges":null})),
        },
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"new","type":"log","inputs":{"message":""},"nodes":[],"edges":[]}),
        },
        GraphCommand::AddNode {
            graph: root,
            value: json!({"id":"new","type":"condition","inputs":{"condition":"true"},"nodes":[]}),
        },
    ] {
        assert!(data.apply_graph_command(0, command).is_err());
        assert_eq!(data.graph_edit.current_config(), &before);
        assert_eq!(data.graph_edit.revision(), 0);
        assert_eq!(data.graph_edit.topology_revision(), 0);
        assert!(!data.graph_edit.can_undo());
    }
}

#[test]
fn 条件节点同时提供有效数组才激活子图且撤销恢复原缺省键() {
    let mut data = inactive_document();
    let node = root_node(&data, "nodes_only");
    let before = data.graph_edit.node_json(node).unwrap().clone();
    apply(
        &mut data,
        GraphCommand::SetSubgraph {
            node,
            value: Some(json!({"nodes":[],"edges":[{"from":"_entry","to":"_exit"}]})),
        },
    );
    let child = data
        .graph_edit
        .graph_key_at(&["nodes_only".into()])
        .unwrap();
    assert!(data.graph_edit.structure_error(child).is_none());
    assert!(data.graph.node("nodes_only").unwrap().children.is_some());
    assert!(data.graph_edit.validate_for_save().is_ok());
    apply(&mut data, GraphCommand::SetSubgraph { node, value: None });
    assert!(
        data.graph_edit
            .graph_key_at(&["nodes_only".into()])
            .is_none()
    );
    assert!(
        data.graph_edit
            .node_json(node)
            .unwrap()
            .get("nodes")
            .is_none()
    );
    data.undo_graph(2).unwrap();
    assert_eq!(
        data.graph_edit.graph_key_at(&["nodes_only".into()]),
        Some(child)
    );
    data.undo_graph(3).unwrap();
    assert_eq!(data.graph_edit.node_json(node), Some(&before));
    assert!(
        data.graph_edit
            .graph_key_at(&["nodes_only".into()])
            .is_none()
    );
    assert!(!data.graph_edit.has_changes());
}

#[test]
fn 类型切换停用子图不丢原文且撤销恢复子树身份重新激活分配新身份() {
    let mut data = document();
    let group = root_node(&data, "group");
    let original = data.graph_edit.node_json(group).unwrap().clone();
    let graph = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    let children = data.graph_edit.nodes(graph);
    apply(
        &mut data,
        GraphCommand::Batch(vec![
            GraphCommand::SetNodeProperty {
                node: group,
                property: "type".into(),
                value: Some(json!("log")),
            },
            GraphCommand::SetNodeProperty {
                node: group,
                property: "inputs".into(),
                value: Some(json!({"message":"叶节点"})),
            },
        ]),
    );
    let leaf = data.graph_edit.node_json(group).unwrap().clone();
    assert_eq!(leaf["nodes"], original["nodes"]);
    assert_eq!(leaf["edges"], original["edges"]);
    assert!(data.graph_edit.graph_key_at(&["group".into()]).is_none());
    assert!(data.graph_edit.node_json(children[0].0).is_none());
    assert_eq!(data.graph_edit.topology_revision(), 1);
    assert_eq!(saved(&data)["config"]["nodes"][1], leaf);
    data.undo_graph(1).unwrap();
    assert_eq!(data.graph_edit.graph_key_at(&["group".into()]), Some(graph));
    assert_eq!(data.graph_edit.nodes(graph), children);
    data.redo_graph(2).unwrap();
    apply(
        &mut data,
        GraphCommand::SetNodeProperty {
            node: group,
            property: "type".into(),
            value: Some(json!("sequence")),
        },
    );
    let reactivated = data.graph_edit.graph_key_at(&["group".into()]).unwrap();
    assert_ne!(reactivated, graph);
    assert_eq!(data.graph_edit.nodes(reactivated).len(), children.len());
    assert_ne!(data.graph_edit.nodes(reactivated)[0].0, children[0].0);
    assert_eq!(data.graph_edit.topology_revision(), 4);
}

#[test]
fn 类型和子图同事务转换按最终状态校验且不受子命令顺序影响() {
    for type_first in [true, false] {
        let mut data = inactive_document();
        let node = root_node(&data, "leaf");
        let mut commands = vec![
            GraphCommand::SetNodeProperty {
                node,
                property: "type".into(),
                value: Some(json!("sequence")),
            },
            GraphCommand::SetSubgraph {
                node,
                value: Some(json!({"nodes":[],"edges":[{"from":"_entry","to":"_exit"}]})),
            },
        ];
        if !type_first {
            commands.reverse();
        }
        apply(&mut data, GraphCommand::Batch(commands));
        assert!(data.graph_edit.graph_key_at(&["leaf".into()]).is_some());
        assert_eq!(data.graph_edit.revision(), 1);
        assert_eq!(data.graph_edit.topology_revision(), 1);
        assert_eq!(
            saved(&data)["config"]["nodes"][0]["branches"],
            json!({"extension":true})
        );
        data.undo_graph(1).unwrap();
        assert!(!data.graph_edit.has_changes());
        assert!(data.graph_edit.graph_key_at(&["leaf".into()]).is_none());
    }
}

#[test]
fn 专用容器缺少数组仍保留诊断入口并禁止在无效层编辑() {
    let mut data = parse_task_graph(
        &json!({"map_id":"m","task_id":"t","config":{"nodes":[
            {"id":"sequence","type":"sequence"},
            {"id":"loop","type":"loop","inputs":{"condition":"false"}},
            {"id":"parallel","type":"parallel"},
            {"id":"condition","type":"condition","nodes":null,"edges":false}
        ]}})
        .to_string(),
    )
    .unwrap();
    assert_eq!(data.graph.diagnostic_count(), 7);
    let before = data.graph_edit.current_config().clone();
    for id in ["sequence", "loop", "parallel", "condition"] {
        let graph = data.graph_edit.graph_key_at(&[id.into()]).unwrap();
        assert!(data.graph.node(id).unwrap().children.is_some());
        assert!(data.graph_edit.structure_error(graph).is_some());
        assert!(
            data.apply_graph_command(
                0,
                GraphCommand::AddNode {
                    graph,
                    value: json!({"id":"new","type":"custom"})
                }
            )
            .is_err()
        );
        assert_eq!(data.graph_edit.current_config(), &before);
    }
    assert!(data.graph_edit.validate_for_save().is_ok());
}

#[test]
fn 未知节点类型的子图保持宽容且未知字段编辑保存撤销保真() {
    let mut data = parse_task_graph(&json!({"map_id":"m","task_id":"t","config":{"nodes":[
        {"id":"future","type":"future_container","nodes":[{"id":"child","type":"future_action","vendor":[1,2.0]}],"edges":[],"vendor":"unknown"}
    ],"edges":[{"from":"_entry","to":"future"}]}}).to_string()).unwrap();
    let before = data.raw_json.clone();
    let graph = data.graph_edit.graph_key_at(&["future".into()]).unwrap();
    let node = data.graph_edit.node_key(graph, "child").unwrap();
    apply(
        &mut data,
        GraphCommand::RenameNode {
            node,
            id: "renamed".into(),
        },
    );
    assert_eq!(data.graph_edit.node_key(graph, "renamed"), Some(node));
    assert_eq!(saved(&data)["config"]["nodes"][0]["vendor"], "unknown");
    data.undo_graph(1).unwrap();
    assert_eq!(saved(&data), before);
}
