//! 编辑命令只修改候选状态，由父模块统一提交或回滚。

use super::*;

pub(super) fn apply_command(
    state: &mut GraphState,
    next: &mut u64,
    command: GraphCommand,
) -> Result<bool, GraphEditError> {
    match command {
        GraphCommand::Batch(commands) => {
            let mut topology = false;
            for command in commands {
                topology |= apply_command(state, next, command)?;
            }
            Ok(topology)
        }
        GraphCommand::SetEntryPoint { graph, id } => {
            if graph != state.identity.key {
                return match graph_location(&state.identity, graph) {
                    Some(_) => Err(GraphEditError::InvalidProperty(
                        "执行器只读取根流程的 entry_point；子图请通过 _entry 连接设置入口".into(),
                    )),
                    None => Err(GraphEditError::MissingTarget),
                };
            }
            if let Some(id) = &id {
                validate_id(id)?;
                if unique_node_index(&state.config, id).is_none() {
                    return Err(structure_error(format!(
                        "入口节点 {id:?} 必须在根流程中唯一存在"
                    )));
                }
            }
            // 精确验证目标即可：旧 entry_point 无效时也必须允许用户修正或移除它。
            set_property(
                &mut state.config,
                "entry_point".into(),
                id.map(Value::String),
            )?;
            Ok(true)
        }
        GraphCommand::SetNodeProperty {
            node,
            property,
            value,
        } => {
            match property.as_str() {
                "id" | "nodes" | "edges" | "branches" | "" => {
                    return Err(GraphEditError::InvalidProperty(
                        "身份和子图必须使用对应结构操作".into(),
                    ));
                }
                "type"
                    if !value
                        .as_ref()
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty()) =>
                {
                    return Err(GraphEditError::InvalidProperty(
                        "type 必须是非空字符串".into(),
                    ));
                }
                _ => (),
            }
            let (path, index) =
                node_location(&state.identity, node).ok_or(GraphEditError::MissingTarget)?;
            let raw = graph_at_mut(&mut state.config, &path)
                .and_then(|raw| node_at_mut(raw, index))
                .ok_or(GraphEditError::MissingTarget)?;
            let previous_source = node_array_key(raw);
            let previous_child = has_subgraph(raw);
            set_property(raw, property, value)?;
            let topology =
                previous_source != node_array_key(raw) || previous_child != has_subgraph(raw);
            if topology {
                let child = has_subgraph(raw).then(|| Box::new(build_identity(raw, next)));
                identity_at_mut(&mut state.identity, &path)
                    .and_then(|identity| identity.nodes.get_mut(index))
                    .ok_or(GraphEditError::MissingTarget)?
                    .child = child;
            }
            Ok(topology)
        }
        GraphCommand::SetEdgeProperty {
            edge,
            property,
            value,
        } => {
            if matches!(property.as_str(), "from" | "to" | "") {
                return Err(GraphEditError::InvalidProperty(
                    "端点必须使用重连操作".into(),
                ));
            }
            let (path, index) =
                edge_location(&state.identity, edge).ok_or(GraphEditError::MissingTarget)?;
            let raw = graph_at_mut(&mut state.config, &path)
                .and_then(|raw| raw.get_mut("edges"))
                .and_then(|edges| edges.get_mut(index))
                .ok_or(GraphEditError::MissingTarget)?;
            set_property(raw, property.clone(), value)?;
            validate_edge_property(raw, &property)?;
            Ok(false)
        }
        GraphCommand::RenameNode { node, id } => {
            validate_id(&id)?;
            let (path, index) =
                node_location(&state.identity, node).ok_or(GraphEditError::MissingTarget)?;
            let raw = editable_layer(state, &path)?;
            let old = node_at(raw, index).ok_or(GraphEditError::MissingTarget)?["id"]
                .as_str()
                .ok_or(GraphEditError::MissingTarget)?
                .to_owned();
            if old == id {
                return Ok(false);
            }
            if array(raw, "nodes")
                .iter()
                .any(|node| node.get("id").and_then(Value::as_str) == Some(&id))
            {
                return Err(structure_error(format!("节点 ID {id:?} 在本层已存在")));
            }
            let priorities = preserve_condition_order(raw, &old, &id)?;
            let raw =
                graph_at_mut(&mut state.config, &path).ok_or(GraphEditError::MissingTarget)?;
            node_at_mut(raw, index).ok_or(GraphEditError::MissingTarget)?["id"] =
                Value::String(id.clone());
            if raw.get("entry_point").and_then(Value::as_str) == Some(&old) {
                raw["entry_point"] = Value::String(id.clone());
            }
            let node_field = node_array_key(raw);
            if let Some(nodes) = raw.get_mut(node_field).and_then(Value::as_array_mut) {
                for node in nodes {
                    if node.get("type").and_then(Value::as_str) == Some("condition") {
                        for field in ["if_true", "if_false"] {
                            if node.get(field).and_then(Value::as_str) == Some(&old) {
                                node[field] = Value::String(id.clone());
                            }
                        }
                    }
                }
            }
            for (edge_index, priority) in priorities {
                raw["edges"][edge_index]["priority"] = Value::from(priority);
            }
            if let Some(edges) = raw.get_mut("edges").and_then(Value::as_array_mut) {
                for edge in edges {
                    for endpoint in ["from", "to"] {
                        if edge.get(endpoint).and_then(Value::as_str) == Some(&old) {
                            edge[endpoint] = Value::String(id.clone());
                        }
                    }
                }
            }
            Ok(true)
        }
        GraphCommand::AddNode { graph, value } => {
            let path =
                graph_location(&state.identity, graph).ok_or(GraphEditError::MissingTarget)?;
            editable_layer(state, &path)?;
            validate_node(&value)?;
            if has_subgraph(&value) {
                validate_tree(&value)?;
            }
            let identity = build_node_identity(&value, next);
            let raw =
                graph_at_mut(&mut state.config, &path).ok_or(GraphEditError::MissingTarget)?;
            array_mut(raw, "nodes")?.push(value);
            validate_layer(raw)?;
            identity_at_mut(&mut state.identity, &path)
                .ok_or(GraphEditError::MissingTarget)?
                .nodes
                .push(identity);
            Ok(true)
        }
        GraphCommand::AddEdge { graph, value } => {
            let path =
                graph_location(&state.identity, graph).ok_or(GraphEditError::MissingTarget)?;
            editable_layer(state, &path)?;
            let raw =
                graph_at_mut(&mut state.config, &path).ok_or(GraphEditError::MissingTarget)?;
            array_mut(raw, "edges")?.push(value);
            validate_layer(raw)?;
            identity_at_mut(&mut state.identity, &path)
                .ok_or(GraphEditError::MissingTarget)?
                .edges
                .push(EdgeKey(allocate(next)));
            Ok(true)
        }
        GraphCommand::ReconnectEdge { edge, from, to } => {
            let (path, index) =
                edge_location(&state.identity, edge).ok_or(GraphEditError::MissingTarget)?;
            editable_layer(state, &path)?;
            let raw =
                graph_at_mut(&mut state.config, &path).ok_or(GraphEditError::MissingTarget)?;
            raw["edges"][index]["from"] = Value::String(from);
            raw["edges"][index]["to"] = Value::String(to);
            validate_layer(raw)?;
            Ok(true)
        }
        GraphCommand::DeleteNodes { nodes } => {
            // 先解析全部目标，再按深层、较大下标优先移除，避免数组位移改变另一目标。
            let mut locations = BTreeMap::new();
            for key in nodes {
                let (path, index) =
                    node_location(&state.identity, key).ok_or(GraphEditError::MissingTarget)?;
                editable_layer(state, &path)?;
                locations
                    .entry(path)
                    .or_insert_with(BTreeSet::new)
                    .insert(index);
            }
            for (path, indices) in locations.into_iter().rev() {
                let raw =
                    graph_at_mut(&mut state.config, &path).ok_or(GraphEditError::MissingTarget)?;
                let identity = identity_at_mut(&mut state.identity, &path)
                    .ok_or(GraphEditError::MissingTarget)?;
                let deleted: BTreeSet<_> = indices
                    .iter()
                    .filter_map(|index| {
                        node_at(raw, *index)?.get("id")?.as_str().map(str::to_owned)
                    })
                    .collect();
                if let Some(edges) = raw.get_mut("edges").and_then(Value::as_array_mut) {
                    for index in (0..edges.len()).rev() {
                        if ["from", "to"].into_iter().any(|endpoint| {
                            edges[index]
                                .get(endpoint)
                                .and_then(Value::as_str)
                                .is_some_and(|id| deleted.contains(id))
                        }) {
                            edges.remove(index);
                            identity.edges.remove(index);
                        }
                    }
                }
                if raw
                    .get("entry_point")
                    .and_then(Value::as_str)
                    .is_some_and(|id| deleted.contains(id))
                {
                    raw.as_object_mut()
                        .ok_or(GraphEditError::MissingTarget)?
                        .remove("entry_point");
                }
                let nodes = array_mut(raw, "nodes")?;
                for node in nodes.iter_mut() {
                    if node.get("type").and_then(Value::as_str) == Some("condition") {
                        for field in ["if_true", "if_false"] {
                            if node
                                .get(field)
                                .and_then(Value::as_str)
                                .is_some_and(|id| deleted.contains(id))
                            {
                                node.as_object_mut()
                                    .ok_or(GraphEditError::MissingTarget)?
                                    .remove(field);
                            }
                        }
                    }
                }
                for index in indices.into_iter().rev() {
                    nodes.remove(index);
                    identity.nodes.remove(index);
                }
            }
            Ok(true)
        }
        GraphCommand::DeleteEdges { edges } => {
            let mut locations = BTreeMap::new();
            for key in edges {
                let (path, index) =
                    edge_location(&state.identity, key).ok_or(GraphEditError::MissingTarget)?;
                editable_layer(state, &path)?;
                locations
                    .entry(path)
                    .or_insert_with(BTreeSet::new)
                    .insert(index);
            }
            for (path, indices) in locations {
                let raw =
                    graph_at_mut(&mut state.config, &path).ok_or(GraphEditError::MissingTarget)?;
                let identity = identity_at_mut(&mut state.identity, &path)
                    .ok_or(GraphEditError::MissingTarget)?;
                let edges = array_mut(raw, "edges")?;
                for index in indices.into_iter().rev() {
                    edges.remove(index);
                    identity.edges.remove(index);
                }
            }
            Ok(true)
        }
        GraphCommand::SetSubgraph { node, value } => {
            let (path, index) =
                node_location(&state.identity, node).ok_or(GraphEditError::MissingTarget)?;
            editable_layer(state, &path)?;
            if let Some(value) = &value {
                let fields = value
                    .as_object()
                    .ok_or_else(|| structure_error("子图必须是包含节点数组和 edges 的对象"))?;
                if fields.is_empty()
                    || fields
                        .keys()
                        .any(|key| !matches!(key.as_str(), "nodes" | "branches" | "edges"))
                {
                    return Err(structure_error(
                        "子图对象只能包含 nodes/branches/edges，且至少提供一个数组",
                    ));
                }
                if fields.contains_key("nodes") && fields.contains_key("branches") {
                    return Err(structure_error(
                        "请只提交 nodes 或 branches 中的一种，避免子图来源歧义",
                    ));
                }
            }
            let raw = graph_at_mut(&mut state.config, &path)
                .and_then(|raw| node_at_mut(raw, index))
                .ok_or(GraphEditError::MissingTarget)?;
            let parallel = raw.get("type").and_then(Value::as_str) == Some("parallel");
            let previous_entry = raw
                .get("entry_point")
                .and_then(Value::as_str)
                .filter(|entry| unique_node_index(raw, entry).is_some())
                .map(str::to_owned);
            let source = match value.as_ref().and_then(|value| value.get("branches")) {
                Some(_) if !parallel => {
                    return Err(structure_error("只有 parallel 节点支持 branches"));
                }
                Some(_) => "branches",
                None => node_array_key(raw),
            };
            let map = raw.as_object_mut().ok_or(GraphEditError::MissingTarget)?;
            match &value {
                Some(value) => {
                    match value.get("branches").or_else(|| value.get("nodes")) {
                        Some(nodes) => {
                            map.insert(source.to_owned(), nodes.clone());
                        }
                        None if source == "branches" => {
                            map.insert(source.to_owned(), Value::Array(Vec::new()));
                        }
                        None => {
                            map.remove(source);
                        }
                    }
                    match value.get("edges") {
                        Some(edges) => {
                            map.insert("edges".to_owned(), edges.clone());
                        }
                        None => {
                            map.remove("edges");
                        }
                    }
                }
                None => {
                    map.remove("nodes");
                    map.remove("edges");
                    if parallel {
                        map.remove("branches");
                    }
                }
            }
            // 只清理原本指向子节点的引用；执行器不读取容器 entry_point，
            // 已有其他类型或未指向子节点的同名扩展仍保持原值。
            if previous_entry
                .as_deref()
                .is_some_and(|entry| unique_node_index(raw, entry).is_none())
            {
                raw.as_object_mut()
                    .ok_or(GraphEditError::MissingTarget)?
                    .remove("entry_point");
            }
            if value.is_some() {
                validate_tree(raw)?;
            }
            let child = has_subgraph(raw).then(|| Box::new(build_identity(raw, next)));
            identity_at_mut(&mut state.identity, &path)
                .and_then(|identity| identity.nodes.get_mut(index))
                .ok_or(GraphEditError::MissingTarget)?
                .child = child;
            Ok(true)
        }
    }
}
