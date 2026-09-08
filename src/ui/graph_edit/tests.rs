//! 实际 BSN 表单和 InputFlush 排程回归；不创建窗口、不读登录配置、不连接 SSH。

use super::*;
use crate::model::{LoginConfig, parse_task_graph, serialize_task_graph};
use crate::ui::editor::InputValidation;
use crate::ui::worker_bridge::WorkerBridgePlugin;
use crate::ui::{FileBrowser, RemoteDocument, Session, StatusLine};
use crate::worker::BusyState;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::scene::ScenePlugin;
use bevy::text::{FontCx, LayoutCx, TextEdit};
use bevy::ui::Checked;
use serde_json::json;

#[test]
fn 实际根入口按钮设置和移除显式入口且撤销精确恢复() {
    let mut app = form_app("wait");
    let button = operation_button(&mut app, |operation| {
        matches!(
            operation,
            GraphOperation::Control(GraphControl::SetEntryPoint(Some(_)))
        )
    });
    activate(&mut app, button);
    settle(&mut app);
    assert_eq!(serialized(&app)["config"]["entry_point"], "wait");
    assert!(
        app.world_mut()
            .query::<&Text>()
            .iter(app.world())
            .any(|text| text.0 == "顶层入口：wait")
    );
    let button = operation_button(&mut app, |operation| {
        matches!(
            operation,
            GraphOperation::Control(GraphControl::SetEntryPoint(None))
        )
    });
    activate(&mut app, button);
    settle(&mut app);
    assert!(serialized(&app)["config"].get("entry_point").is_none());
    send_operation(&mut app, GraphOperation::Undo);
    assert_eq!(serialized(&app)["config"]["entry_point"], "wait");
}

#[test]
fn 实际清空子图按钮保留容器和可执行空路径且撤销恢复子节点() {
    let mut app = form_app("group");
    let original = node(&app, "group");
    let button = operation_button(&mut app, |operation| {
        matches!(
            operation,
            GraphOperation::Control(GraphControl::ClearSubgraph(_))
        )
    });
    activate(&mut app, button);
    settle(&mut app);
    let cleared = node(&app, "group");
    assert_eq!(cleared["type"], "sequence");
    assert_eq!(cleared["nodes"], json!([]));
    assert_eq!(cleared["edges"], json!([{"from":"_entry","to":"_exit"}]));
    app.world()
        .resource::<Editor>()
        .data
        .as_ref()
        .unwrap()
        .graph_edit
        .validate_for_save()
        .unwrap();
    send_operation(&mut app, GraphOperation::Undo);
    assert_eq!(node(&app, "group"), original);
}

#[test]
fn 退出后重新进入流程编辑不会恢复旧多选删除范围() {
    let mut app = form_app("message");
    let message = node_key(&app, "message");
    let wait = node_key(&app, "wait");
    app.world_mut()
        .resource_mut::<GraphEditing>()
        .selected_nodes
        .extend([message, wait]);
    let button = operation_button(&mut app, |operation| {
        matches!(operation, GraphOperation::Control(GraphControl::Toggle))
    });
    activate(&mut app, button);
    settle(&mut app);
    assert!(
        app.world()
            .resource::<GraphEditing>()
            .selected_nodes
            .is_empty()
    );
    let button = operation_button(&mut app, |operation| {
        matches!(operation, GraphOperation::Control(GraphControl::Toggle))
    });
    activate(&mut app, button);
    settle(&mut app);
    assert_eq!(
        app.world().resource::<GraphEditing>().selected_nodes,
        BTreeSet::from([message])
    );
    send_control(&mut app, GraphControl::DeleteSelection);
    assert_eq!(node(&app, "wait")["id"], "wait");
}

#[test]
fn 并行子图草稿只编辑活跃分支并原样保留被遮蔽节点和边缺省() {
    for explicit_edges in [false, true] {
        let mut app = form_app("group");
        let mut raw = serialized(&app);
        let mut parallel = json!({"id":"group","type":"parallel","branches":[{"id":"child","type":"log","inputs":{"message":"原始分支"}}],"nodes":[{"opaque":"保留被执行器忽略的扩展"}]});
        if explicit_edges {
            parallel["edges"] = json!([{"from":"child","to":"_exit","vendor":{"unchanged":true}}]);
        }
        raw["config"]["nodes"][5] = parallel.clone();
        let data = parse_task_graph(&raw.to_string()).unwrap();
        let document = app.world().resource::<Editor>().document.clone().unwrap();
        app.world_mut()
            .resource_mut::<Editor>()
            .load_remote(data, document);
        settle(&mut app);
        let key = node_key(&app, "group");
        send_control(&mut app, GraphControl::EditSubgraph(key));
        settle(&mut app);
        let draft = app
            .world()
            .resource::<GraphEditing>()
            .draft
            .as_ref()
            .unwrap();
        assert_eq!(draft.original.get("edges"), parallel.get("edges"));
        assert!(draft.original.get("nodes").is_none());
        let input = field_entity(&mut app, &["branches"]);
        queue_text(
            &mut app,
            input,
            &json!([{"id":"child","type":"log","inputs":{"message":"修改后的分支"}}]).to_string(),
        );
        apply_button(&mut app);
        settle(&mut app);
        let changed = node(&app, "group");
        assert_eq!(changed["branches"][0]["inputs"]["message"], "修改后的分支");
        assert_eq!(changed["nodes"], parallel["nodes"]);
        assert_eq!(changed.get("edges"), parallel.get("edges"));
    }
}

fn long_message() -> String {
    format!(
        "{}\n原始全文末尾",
        "完整参数，不能使用展示摘要；".repeat(90)
    )
}

fn fixture() -> Editor {
    let data = parse_task_graph(
        &json!({
            "map_id":"fixture_map", "task_id":"fixture_task", "vendor":{"keep":true},
            "config": {
                "context":{"speed":1.5, "source":"  [1, 2]  "},
                "nodes":[
                    {"id":"message", "type":"log", "inputs":{"message":long_message(), "nested":{"numbers":[1,2], "literal":"{message}"}}, "vendor":{"unchanged":42}},
                    {"id":"wait", "type":"wait_condition", "inputs":{"condition":"true", "poll_interval_ms":100}},
                    {"id":"mock", "type":"mock_action", "inputs":{"action_name":"mock", "success":true}},
                    {"id":"choice", "type":"condition", "inputs":{"condition":"false"}},
                    {"id":"legacy", "type":"log", "params":{"message":"旧参数", "nested":{"value":2}}},
                    {"id":"group", "type":"sequence", "nodes":[{"id":"message", "type":"log", "inputs":{"message":"子节点原文"}}], "edges":[{"from":"_entry", "to":"message"},{"from":"message", "to":"_exit"}]}
                ],
                "edges":[
                    {"from":"_entry","to":"message"},
                    {"from":"message","to":"wait","label":"true","vendor":{"retain":7}},
                    {"from":"wait","to":"_exit"}
                ]
            }
        })
        .to_string(),
    ).unwrap();
    let mut editor = Editor::default();
    editor.load_remote(
        data,
        RemoteDocument {
            connection_generation: 7,
            remote_dir: "/fixture/graphs".into(),
            filename: "fixture_task.json".into(),
        },
    );
    editor
}

/// 与应用相同的输入、刷新、动作、重建顺序；远程会话仅有内存身份，没有 worker。
fn form_app(selected: &str) -> App {
    form_app_with_guard(selected, false)
}

fn form_app_with_guard(selected: &str, with_guard: bool) -> App {
    let mut session = Session::new(LoginConfig::default(), Vec::new());
    session.is_connected = true;
    session.connection_generation = 7;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
        .init_asset::<Font>()
        .init_resource::<InputFocus>()
        .init_resource::<FontCx>()
        .init_resource::<LayoutCx>()
        .init_resource::<bevy::clipboard::Clipboard>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<InputValidation>()
        .init_resource::<StatusLine>()
        .init_resource::<FileBrowser>()
        .insert_resource(session)
        .insert_resource(fixture())
        .insert_resource(ViewMode::Graph)
        .insert_resource(GraphNav {
            selected: Some(selected.into()),
            ..default()
        })
        .insert_resource(GraphEditing {
            enabled: true,
            ..default()
        })
        .configure_sets(
            Update,
            (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
        )
        .add_plugins((widgets::WidgetsPlugin, WorkerBridgePlugin, GraphEditPlugin))
        .add_observer(crate::ui::connect::on_action_button)
        .add_systems(PostUpdate, bevy::text::apply_text_edits);
    if with_guard {
        app.add_plugins(crate::ui::document_guard::DocumentGuardPlugin);
    }
    let font = Font::from_bytes(
        include_bytes!("../../../assets/fonts/SarasaTermSCNerd-Regular.ttf").to_vec(),
    );
    let mut fonts = app.world_mut().resource_mut::<FontCx>();
    let registered = fonts.collection.register_fonts(font.data, None);
    let family = fonts
        .collection
        .family_name(registered[0].0)
        .unwrap()
        .to_string();
    fonts.set_sans_serif_family(&family).unwrap();
    app.world_mut()
        .spawn((GraphEditToolbarSlot, Node::default()));
    app.world_mut().spawn((GraphEditPanelSlot, Node::default()));
    settle(&mut app);
    app
}

fn settle(app: &mut App) {
    for _ in 0..5 {
        app.update();
    }
}

fn double_click_app(enabled: bool) -> App {
    let mut app = form_app("group");
    // 本组验证事件归并、导航与草稿守卫，双击窗口不依赖慢速 CI 的实际运行耗时。
    app.insert_resource(bevy::picking::PickingSettings {
        multi_click_interval: std::time::Duration::MAX,
        ..default()
    });
    let slots: Vec<_> = app
        .world_mut()
        .query_filtered::<Entity, Or<(With<GraphEditPanelSlot>, With<GraphEditToolbarSlot>)>>()
        .iter(app.world())
        .collect();
    for slot in slots {
        app.world_mut().despawn(slot);
    }
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        let data = editor.data.as_mut().unwrap();
        let group = data
            .graph_edit
            .node_key(data.graph_edit.root_key(), "group")
            .unwrap();
        data.apply_graph_command(data.graph_edit.revision(), GraphCommand::SetSubgraph { node: group, value: Some(json!({
            "nodes":[{"id":"inner", "type":"sequence", "nodes":[{"id":"leaf","type":"log","inputs":{"message":"叶子"}}], "edges":[{"from":"_entry","to":"leaf"},{"from":"leaf","to":"_exit"}]}],
            "edges":[{"from":"_entry","to":"inner"},{"from":"inner","to":"_exit"}]
        })) }).unwrap();
    }
    app.add_plugins(crate::ui::graph_view::GraphViewPlugin);
    app.world_mut()
        .spawn((Window::default(), bevy::window::PrimaryWindow));
    app.world_mut()
        .spawn((crate::ui::shell::GraphSlot, Node::default()));
    app.world_mut().resource_mut::<GraphEditing>().enabled = enabled;
    settle(&mut app);
    app.world_mut().resource_mut::<GraphNav>().selected = Some("group".into());
    settle(&mut app);
    app
}

fn graph_card_targets(app: &mut App, id: &str) -> (Entity, Entity) {
    let labels: Vec<_> = app
        .world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .filter(|(_, text)| text.0 == id)
        .map(|(entity, _)| entity)
        .collect();
    for label in labels {
        let mut current = label;
        loop {
            if app.world().get::<Node>(current).is_some_and(|node| {
                node.position_type == PositionType::Absolute
                    && node.width == px(crate::ui::graph_layout::NODE_W)
                    && node.height == px(crate::ui::graph_layout::NODE_H)
            }) {
                return (label, current);
            }
            let Some(parent) = app.world().get::<ChildOf>(current) else {
                break;
            };
            current = parent.parent();
        }
    }
    panic!("实际画布缺少卡片：{id}");
}

fn graph_click(app: &mut App, entity: Entity, dragged: bool) {
    use bevy::picking::{
        backend::HitData,
        pointer::{Location, PointerButton, PointerId},
    };
    let location = Location {
        target: bevy::camera::NormalizedRenderTarget::None {
            width: 1200,
            height: 800,
        },
        position: Vec2::new(80.0, 60.0),
    };
    let hit = HitData::new(Entity::PLACEHOLDER, 0.0, None, None);
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        location.clone(),
        Press {
            button: PointerButton::Primary,
            hit: hit.clone(),
            count: 1,
        },
        entity,
    ));
    if dragged {
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location.clone(),
            Drag {
                button: PointerButton::Primary,
                distance: Vec2::new(30.0, 0.0),
                delta: Vec2::new(30.0, 0.0),
            },
            entity,
        ));
    }
    // 两个子目标各自的原生count都是1，产品须按同一卡片归并；拖出再回到原点也不能成为点击。
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        location,
        Click {
            button: PointerButton::Primary,
            hit,
            duration: std::time::Duration::from_millis(20),
            count: 1,
        },
        entity,
    ));
}

#[test]
fn 双击文字和卡片空白在浏览编辑模式均经动作队列进入深层子图() {
    for enabled in [false, true] {
        let mut app = double_click_app(enabled);
        let (label, card) = graph_card_targets(&mut app, "group");
        graph_click(&mut app, label, false);
        app.update();
        assert!(
            app.world().resource::<GraphNav>().path.is_empty(),
            "单击只选择"
        );
        graph_click(&mut app, card, false);
        assert!(
            app.world().resource::<GraphNav>().path.is_empty(),
            "双击必须先排队等待InputFlush"
        );
        settle(&mut app);
        assert_eq!(app.world().resource::<GraphNav>().path, ["group"]);
        let (label, card) = graph_card_targets(&mut app, "inner");
        graph_click(&mut app, label, false);
        graph_click(&mut app, card, false);
        settle(&mut app);
        assert_eq!(app.world().resource::<GraphNav>().path, ["group", "inner"]);
        for id in ["leaf", "入口", "出口"] {
            let (label, card) = graph_card_targets(&mut app, id);
            graph_click(&mut app, label, false);
            graph_click(&mut app, card, false);
            settle(&mut app);
            assert_eq!(app.world().resource::<GraphNav>().path, ["group", "inner"]);
        }
        assert_eq!(app.world().resource::<GraphEditing>().enabled, enabled);
    }
}

#[test]
fn 双击不会把拖出再返回修饰键或旧画布事件当作下钻() {
    let mut app = double_click_app(true);
    let (label, card) = graph_card_targets(&mut app, "group");
    graph_click(&mut app, label, false);
    graph_click(&mut app, card, true);
    graph_click(&mut app, label, false);
    app.update();
    assert!(
        app.world().resource::<GraphNav>().path.is_empty(),
        "拖动必须中断连续点击"
    );
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);
    graph_click(&mut app, label, false);
    graph_click(&mut app, card, false);
    app.update();
    assert!(app.world().resource::<GraphNav>().path.is_empty());
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    app.world_mut().resource_mut::<GraphNav>().version += 1;
    graph_click(&mut app, label, false);
    graph_click(&mut app, card, false);
    app.update();
    assert!(app.world().resource::<GraphNav>().path.is_empty());
}

#[test]
fn 双击下钻先刷新同帧草稿并拒绝丢弃未提交属性() {
    let mut app = double_click_app(true);
    let input = field_entity(&mut app, &["type"]);
    queue_text(&mut app, input, "parallel");
    let (label, card) = graph_card_targets(&mut app, "group");
    graph_click(&mut app, label, false);
    graph_click(&mut app, card, false);
    app.update();
    assert!(app.world().resource::<GraphNav>().path.is_empty());
    assert!(app.world().resource::<GraphEditing>().has_draft_changes());
    assert_eq!(
        app.world()
            .get::<EditableText>(input)
            .unwrap()
            .value()
            .to_string(),
        "parallel"
    );
    assert_eq!(node(&app, "group")["type"], "sequence");
    assert!(app.world().resource::<StatusLine>().text.contains("草稿"));
}

#[test]
fn 双击切层等待只读详情和编辑表单跨帧落地且旧插槽不再续排() {
    for enabled in [false, true] {
        let mut app = double_click_app(enabled);
        app.world_mut().resource_mut::<GraphNav>().selected = Some("wait".into());
        settle(&mut app);
        let graph_slot = app
            .world_mut()
            .query_filtered::<Entity, With<crate::ui::shell::GraphSlot>>()
            .single(app.world())
            .unwrap();
        let old_panel = app
            .world_mut()
            .query_filtered::<Entity, With<GraphEditPanelSlot>>()
            .single(app.world())
            .unwrap();
        let (label, card) = graph_card_targets(&mut app, "group");
        app.world_mut().resource_mut::<GraphNav>().selected = Some("group".into());
        graph_click(&mut app, label, false);
        // 留住真实 BSN 在飞批次；第二次点击必须在这批子实体尚未落地时切层。
        app.world_mut().run_schedule(Update);
        let pending: Vec<_> = app
            .world_mut()
            .query_filtered::<Entity, With<widgets::SlotPending>>()
            .iter(app.world())
            .collect();
        assert!(!pending.is_empty(), "选中必须产生真实的在飞详情或表单");
        if enabled {
            assert!(pending.contains(&old_panel));
        }
        graph_click(&mut app, card, false);
        app.world_mut().run_schedule(Update);
        assert_eq!(app.world().resource::<GraphNav>().path, ["group"]);
        assert!(app.world().get_entity(old_panel).is_ok());
        // 再运行一次重建阶段不能在旧插槽上排入新层属性，避免父树一直等不到空闲。
        app.world_mut().run_schedule(Update);
        settle(&mut app);
        settle(&mut app);
        assert!(app.world().get_entity(old_panel).is_err());
        assert_eq!(app.world().resource::<GraphEditing>().enabled, enabled);
        let nodes: Vec<_> = app
            .world_mut()
            .query_filtered::<Entity, With<Node>>()
            .iter(app.world())
            .collect();
        for entity in nodes {
            let mut current = entity;
            while let Some(parent) = app.world().get::<ChildOf>(current) {
                current = parent.parent();
            }
            assert_eq!(current, graph_slot, "旧详情或表单不能成为顶层 UI：{entity}");
        }
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<GraphEditPanelSlot>>()
                .iter(app.world())
                .count(),
            1
        );
    }
}

#[test]
fn 父画布重建完成后属性面板只更新新插槽并保留有效编辑目标() {
    use crate::ui::graph_view::GraphViewPlugin;
    use crate::ui::shell::GraphSlot;
    let mut app = form_app("wait");
    // 用实际画布产生的子插槽替换表单夹具的独立插槽，覆盖两个插件同时重建的交错。
    let slots: Vec<_> = app
        .world_mut()
        .query_filtered::<Entity, Or<(With<GraphEditPanelSlot>, With<GraphEditToolbarSlot>)>>()
        .iter(app.world())
        .collect();
    for slot in slots {
        app.world_mut().despawn(slot);
    }
    app.add_plugins(GraphViewPlugin);
    app.world_mut()
        .spawn((Window::default(), bevy::window::PrimaryWindow));
    let graph = app.world_mut().spawn((GraphSlot, Node::default())).id();
    settle(&mut app);

    for reload in [true, false, true, false] {
        app.world_mut().resource_mut::<GraphNav>().selected = Some("wait".into());
        settle(&mut app);
        let old_panel = app
            .world_mut()
            .query_filtered::<Entity, With<GraphEditPanelSlot>>()
            .single(app.world())
            .unwrap();
        let old_toolbar = app
            .world_mut()
            .query_filtered::<Entity, With<GraphEditToolbarSlot>>()
            .single(app.world())
            .unwrap();
        match reload {
            true => {
                let replacement = fixture();
                app.world_mut()
                    .resource_mut::<Editor>()
                    .load_remote(replacement.data.unwrap(), replacement.document.unwrap());
            }
            false => {
                let mut editor = app.world_mut().resource_mut::<Editor>();
                let data = editor.data.as_mut().unwrap();
                data.apply_graph_command(
                    data.graph_edit.revision(),
                    GraphCommand::AddEdge {
                        graph: data.graph_edit.root_key(),
                        value: json!({"from":"message", "to":"wait"}),
                    },
                )
                .unwrap();
            }
        }
        settle(&mut app);
        assert!(app.world().get_entity(old_panel).is_err());
        assert!(app.world().get_entity(old_toolbar).is_err());
        assert!(
            app.world().get_entity(graph).is_ok(),
            "只替换画布子树，保留外层插槽"
        );
        app.world_mut().resource_mut::<GraphNav>().selected = Some("wait".into());
        settle(&mut app);
        let input = field_entity(&mut app, &["inputs", "poll_interval_ms"]);
        assert_eq!(
            app.world()
                .get::<EditableText>(input)
                .unwrap()
                .value()
                .to_string(),
            "100"
        );
        assert!(app.world().resource::<GraphEditing>().enabled);
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<GraphEditPanelSlot>>()
                .iter(app.world())
                .count(),
            1
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<GraphEditToolbarSlot>>()
                .iter(app.world())
                .count(),
            1
        );
    }
}

fn field_index(app: &App, path: &[&str]) -> usize {
    app.world()
        .resource::<GraphEditing>()
        .draft
        .as_ref()
        .unwrap()
        .fields
        .iter()
        .position(|field| {
            !field.removed
                && field
                    .path
                    .iter()
                    .map(String::as_str)
                    .eq(path.iter().copied())
        })
        .unwrap_or_else(|| panic!("表单缺少字段 {path:?}"))
}

fn field_entity(app: &mut App, path: &[&str]) -> Entity {
    let index = field_index(app, path);
    let generation = app.world().resource::<GraphEditing>().panel_version;
    app.world_mut()
        .query::<(Entity, &DraftBinding)>()
        .iter(app.world())
        .find(|(_, binding)| binding.generation == generation && binding.index == Some(index))
        .map(|(entity, _)| entity)
        .unwrap()
}

fn queue_text(app: &mut App, entity: Entity, value: &str) {
    let mut text = app.world_mut().get_mut::<EditableText>(entity).unwrap();
    text.queue_edit(TextEdit::SelectAll);
    text.queue_edit(TextEdit::Insert(value.to_owned().into()));
}

fn operation_button(app: &mut App, wanted: impl Fn(&GraphOperation) -> bool) -> Entity {
    app.world_mut()
        .query::<(Entity, &ControlButton)>()
        .iter(app.world())
        .find(|(_, button)| button.operation.as_ref().is_some_and(&wanted))
        .map(|(entity, _)| entity)
        .unwrap()
}

fn activate(app: &mut App, button: Entity) {
    app.world_mut().trigger(Activate { entity: button });
    app.world_mut().flush();
}

fn action_button(app: &mut App, wanted: impl Fn(&AppAction) -> bool) -> Entity {
    app.world_mut()
        .query::<(Entity, &crate::ui::connect::ActionButton)>()
        .iter(app.world())
        .find(|(_, button)| wanted(&button.0))
        .map(|(entity, _)| entity)
        .unwrap()
}

fn apply_button(app: &mut App) {
    let button = operation_button(app, |operation| {
        matches!(operation, GraphOperation::Control(GraphControl::Apply))
    });
    activate(app, button);
}

fn send_control(app: &mut App, control: GraphControl) {
    send_operation(app, GraphOperation::Control(control));
}

fn send_operation(app: &mut App, operation: GraphOperation) {
    let editor = app.world().resource::<Editor>();
    let intent = GraphIntent {
        document_version: editor.document_version,
        revision: editor.data.as_ref().unwrap().graph_edit.revision(),
        operation,
    };
    app.world_mut().write_message(AppAction::GraphEdit(intent));
    app.update();
}

fn node_key(app: &App, id: &str) -> NodeKey {
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    let graph = data.graph_edit.graph_key_at(&[]).unwrap();
    data.graph_edit.node_key(graph, id).unwrap()
}

fn node(app: &App, id: &str) -> Value {
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    data.graph_edit
        .node_json(node_key(app, id))
        .unwrap()
        .clone()
}

fn serialized(app: &App) -> Value {
    serde_json::from_str(
        &serialize_task_graph(app.world().resource::<Editor>().data.as_ref().unwrap()).unwrap(),
    )
    .unwrap()
}

#[test]
fn 真实表单初始化长文本全文且同帧输入应用不丢焦点滚动或实体() {
    let mut app = form_app("message");
    let input = field_entity(&mut app, &["inputs", "message"]);
    assert_eq!(
        app.world()
            .get::<EditableText>(input)
            .unwrap()
            .value()
            .to_string(),
        long_message()
    );
    let mut content = input;
    while app
        .world()
        .get::<bevy::ui_widgets::ScrollArea>(content)
        .is_none()
    {
        content = app.world().get::<ChildOf>(content).unwrap().parent();
    }
    app.world_mut()
        .entity_mut(content)
        .insert(ScrollPosition(Vec2::new(0.0, 147.0)));
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(input, FocusCause::Navigated);
    let generation = app.world().resource::<GraphEditing>().panel_version;
    let updated = format!("{}\n最后一次同帧输入", long_message());
    queue_text(&mut app, input, &updated);
    apply_button(&mut app);
    app.update();
    assert_eq!(node(&app, "message")["inputs"]["message"], updated);
    assert_eq!(node(&app, "message")["vendor"], json!({"unchanged":42}));
    assert_eq!(app.world().resource::<InputFocus>().get(), Some(input));
    assert_eq!(
        app.world().resource::<GraphEditing>().panel_version,
        generation
    );
    assert_eq!(field_entity(&mut app, &["inputs", "message"]), input);
    assert_eq!(
        app.world().get::<ScrollPosition>(content).unwrap().0,
        Vec2::new(0.0, 147.0)
    );
    assert_eq!(
        app.world().resource::<GraphNav>().selected.as_deref(),
        Some("message")
    );
    let output = serialized(&app);
    assert_eq!(output["config"]["context"]["source"], "  [1, 2]  ");
    let reloaded = parse_task_graph(&output.to_string()).unwrap();
    assert_eq!(
        reloaded.graph_edit.current_config()["nodes"][0]["inputs"]["message"],
        updated
    );
}

#[test]
fn 保存也经真实输入刷新提交当前长文本而非草稿旧值() {
    let mut app = form_app("message");
    let input = field_entity(&mut app, &["inputs", "message"]);
    let expected = format!("{}\n保存帧末尾", long_message());
    let ticket = app.world().resource::<Editor>().next_save_ticket();
    queue_text(&mut app, input, &expected);
    let save = action_button(&mut app, |action| matches!(action, AppAction::SaveToRemote));
    activate(&mut app, save);
    app.update();
    assert_eq!(node(&app, "message")["inputs"]["message"], expected);
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Working(_)
    ));
    assert!(
        app.world_mut()
            .resource_mut::<Editor>()
            .confirm_save(ticket)
    );
    assert!(!app.world().resource::<Editor>().has_unsaved_changes());
    assert_eq!(
        serialized(&app)["config"]["nodes"][0]["inputs"]["message"],
        expected
    );
}

#[test]
fn 数字改表达式通过真实表示切换与输入提交且只产生一次事务() {
    let mut app = form_app("wait");
    let index = field_index(&app, &["inputs", "poll_interval_ms"]);
    send_control(&mut app, GraphControl::Representation { index, json: true });
    settle(&mut app);
    let input = field_entity(&mut app, &["inputs", "poll_interval_ms"]);
    queue_text(&mut app, input, "\"{settings.poll_ms}\"");
    apply_button(&mut app);
    app.update();
    assert_eq!(
        node(&app, "wait")["inputs"]["poll_interval_ms"],
        "{settings.poll_ms}"
    );
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    assert_eq!(data.graph_edit.revision(), 1);
    send_operation(&mut app, GraphOperation::Undo);
    assert_eq!(node(&app, "wait")["inputs"]["poll_interval_ms"], 100);
    send_operation(&mut app, GraphOperation::Redo);
    assert_eq!(
        node(&app, "wait")["inputs"]["poll_interval_ms"],
        "{settings.poll_ms}"
    );
}

#[test]
fn 非法数值和非法_json保留可见草稿且阻止应用保存() {
    let mut app = form_app("wait");
    let input = field_entity(&mut app, &["inputs", "poll_interval_ms"]);
    let ticket = app.world().resource::<Editor>().next_save_ticket();
    for text in ["1e999", "NaN", "{broken"] {
        queue_text(&mut app, input, text);
        app.world_mut().write_message(AppAction::SaveToRemote);
        app.update();
        assert_eq!(node(&app, "wait")["inputs"]["poll_interval_ms"], 100);
        assert_eq!(
            app.world()
                .get::<EditableText>(input)
                .unwrap()
                .value()
                .to_string(),
            text
        );
        assert!(app.world().resource::<GraphEditing>().has_draft_changes());
        assert!(app.world().resource::<GraphEditing>().error.is_some());
        assert!(matches!(
            app.world().resource::<Session>().busy,
            BusyState::Idle
        ));
        assert_eq!(app.world().resource::<Editor>().next_save_ticket(), ticket);
    }
    queue_text(&mut app, input, "25");
    apply_button(&mut app);
    app.update();
    assert_eq!(node(&app, "wait")["inputs"]["poll_interval_ms"], 25);
    assert!(!app.world().resource::<GraphEditing>().has_draft_changes());
}

#[test]
fn 真实布尔控件双向变化同步勾选且与文本修改同一事务() {
    let mut app = form_app("mock");
    let checked = field_entity(&mut app, &["inputs", "success"]);
    assert!(app.world().get::<Checked>(checked).is_some());
    let name = field_entity(&mut app, &["inputs", "action_name"]);
    queue_text(&mut app, name, "已编辑的模拟动作");
    app.world_mut().trigger(ValueChange {
        source: checked,
        value: false,
        is_final: true,
    });
    app.world_mut().flush();
    assert!(app.world().get::<Checked>(checked).is_none());
    apply_button(&mut app);
    app.update();
    assert_eq!(
        node(&app, "mock")["inputs"],
        json!({"action_name":"已编辑的模拟动作", "success":false})
    );
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        1
    );
    app.world_mut().trigger(ValueChange {
        source: checked,
        value: true,
        is_final: true,
    });
    app.world_mut().flush();
    assert!(app.world().get::<Checked>(checked).is_some());
    apply_button(&mut app);
    app.update();
    assert_eq!(node(&app, "mock")["inputs"]["success"], true);
}

#[test]
fn 参数回退和嵌套_json编辑保留未改字段且不引入遮蔽inputs() {
    let mut app = form_app("legacy");
    let message = field_entity(&mut app, &["params", "message"]);
    let nested = field_entity(&mut app, &["params", "nested"]);
    queue_text(&mut app, message, "新参数全文");
    queue_text(
        &mut app,
        nested,
        r#"{"value":3,"matrix":[[1,2],[3,4]],"literal":"{legacy}"}"#,
    );
    apply_button(&mut app);
    app.update();
    let value = node(&app, "legacy");
    assert!(value.get("inputs").is_none());
    assert_eq!(value["params"]["message"], "新参数全文");
    assert_eq!(value["params"]["nested"]["matrix"], json!([[1, 2], [3, 4]]));
    assert_eq!(value["params"]["nested"]["literal"], "{legacy}");
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        1
    );
    send_operation(&mut app, GraphOperation::Undo);
    assert_eq!(
        node(&app, "legacy")["params"],
        json!({"message":"旧参数", "nested":{"value":2}})
    );
}

#[test]
fn 子图草稿跨重建保持且切换真实选择后才退出() {
    let mut app = form_app("group");
    let group = node_key(&app, "group");
    send_control(&mut app, GraphControl::EditSubgraph(group));
    settle(&mut app);
    assert_eq!(
        app.world()
            .resource::<GraphEditing>()
            .draft
            .as_ref()
            .unwrap()
            .target,
        DraftTarget::Subgraph(group)
    );
    let nodes = field_entity(&mut app, &["nodes"]);
    queue_text(
        &mut app,
        nodes,
        r#"[{"id":"message","type":"log","inputs":{"message":"子图编辑生效"}}]"#,
    );
    apply_button(&mut app);
    app.update();
    assert_eq!(
        node(&app, "group")["nodes"][0]["inputs"]["message"],
        "子图编辑生效"
    );
    assert_eq!(node(&app, "message")["inputs"]["message"], long_message());
    settle(&mut app);
    assert_eq!(
        app.world()
            .resource::<GraphEditing>()
            .draft
            .as_ref()
            .unwrap()
            .target,
        DraftTarget::Subgraph(group)
    );
    app.world_mut().resource_mut::<GraphNav>().selected = Some("legacy".into());
    settle(&mut app);
    assert_eq!(
        app.world()
            .resource::<GraphEditing>()
            .draft
            .as_ref()
            .unwrap()
            .target,
        DraftTarget::Node(node_key(&app, "legacy"))
    );
}

#[test]
fn 未应用草稿不被选择刷新丢弃而旧文档按钮与控件输入不能误写() {
    let mut app = form_app("message");
    let input = field_entity(&mut app, &["inputs", "message"]);
    let stale_binding = app.world().get::<DraftBinding>(input).unwrap().clone();
    let old_apply = operation_button(&mut app, |operation| {
        matches!(operation, GraphOperation::Control(GraphControl::Apply))
    });
    let old_button = app.world().get::<ControlButton>(old_apply).unwrap().clone();
    queue_text(&mut app, input, "尚未应用");
    app.update();
    app.world_mut().resource_mut::<GraphNav>().selected = Some("legacy".into());
    app.update();
    assert_eq!(
        app.world()
            .resource::<GraphEditing>()
            .draft
            .as_ref()
            .unwrap()
            .target,
        DraftTarget::Node(node_key(&app, "message"))
    );
    let replacement = fixture();
    app.world_mut()
        .resource_mut::<Editor>()
        .load_remote(replacement.data.unwrap(), replacement.document.unwrap());
    settle(&mut app);
    let old_entity = app
        .world_mut()
        .spawn((stale_binding, EditableText::new("旧实体")))
        .id();
    let old_control = app.world_mut().spawn(old_button).id();
    queue_text(&mut app, old_entity, "不应进入新文档");
    activate(&mut app, old_control);
    app.update();
    assert_eq!(node(&app, "legacy")["params"]["message"], "旧参数");
    assert_eq!(node(&app, "message")["inputs"]["message"], long_message());
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        0
    );
    assert!(
        app.world()
            .resource::<GraphEditing>()
            .error
            .as_deref()
            .is_some_and(|error| error.contains("旧文档"))
    );
}

#[test]
fn 外部修订变化拒绝旧属性草稿且失败不新增历史() {
    let mut app = form_app("message");
    let input = field_entity(&mut app, &["inputs", "message"]);
    queue_text(&mut app, input, "旧修订草稿");
    app.update();
    let other = node_key(&app, "legacy");
    let mut editor = app.world_mut().resource_mut::<Editor>();
    let data = editor.data.as_mut().unwrap();
    data.apply_graph_command(
        0,
        GraphCommand::SetNodeProperty {
            node: other,
            property: "vendor".into(),
            value: Some(json!({"new":true})),
        },
    )
    .unwrap();
    apply_button(&mut app);
    app.update();
    assert_eq!(node(&app, "message")["inputs"]["message"], long_message());
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        1
    );
    assert!(app.world().resource::<GraphEditing>().has_draft_changes());
    assert!(app.world().resource::<GraphEditing>().error.is_some());
}

#[test]
fn 新建模板切换和克隆通过界面意图保持子树原文且可撤销重做() {
    let mut app = form_app("group");
    send_control(&mut app, GraphControl::NewNode);
    send_control(&mut app, GraphControl::Template("delay".into()));
    settle(&mut app);
    let target = app
        .world()
        .resource::<GraphEditing>()
        .draft
        .as_ref()
        .unwrap()
        .original["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let duration = field_entity(&mut app, &["inputs", "duration_ms"]);
    queue_text(&mut app, duration, "5");
    app.update();
    send_control(&mut app, GraphControl::Template("sequence".into()));
    assert_eq!(
        app.world()
            .resource::<GraphEditing>()
            .draft
            .as_ref()
            .unwrap()
            .original["type"],
        "delay"
    );
    apply_button(&mut app);
    app.update();
    assert_eq!(node(&app, &target)["inputs"]["duration_ms"], 5);
    let group = node_key(&app, "group");
    send_control(&mut app, GraphControl::CloneNode(group));
    settle(&mut app);
    let cloned_id = app
        .world()
        .resource::<GraphEditing>()
        .draft
        .as_ref()
        .unwrap()
        .original["id"]
        .as_str()
        .unwrap()
        .to_owned();
    apply_button(&mut app);
    app.update();
    assert_eq!(
        node(&app, &cloned_id)["nodes"],
        node(&app, "group")["nodes"]
    );
    assert_eq!(
        node(&app, &cloned_id)["edges"],
        node(&app, "group")["edges"]
    );
    let clone_key = node_key(&app, &cloned_id);
    send_operation(&mut app, GraphOperation::Undo);
    assert!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .node_json(clone_key)
            .is_none()
    );
    send_operation(&mut app, GraphOperation::Redo);
    assert_eq!(node_key(&app, &cloned_id), clone_key);
}

#[test]
fn 边表单重连与标签修改同一事务保留扩展且删除入口阻止保存() {
    let mut app = form_app("message");
    app.world_mut().resource_mut::<GraphNav>().selected_edge = Some(1);
    settle(&mut app);
    let to = field_entity(&mut app, &["to"]);
    let label = field_entity(&mut app, &["label"]);
    queue_text(&mut app, to, "legacy");
    queue_text(&mut app, label, "false");
    apply_button(&mut app);
    app.update();
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    assert_eq!(data.graph_edit.revision(), 1);
    assert_eq!(
        data.graph_edit.current_config()["edges"][1],
        json!({"from":"message", "to":"legacy", "label":"false", "vendor":{"retain":7}})
    );
    send_operation(&mut app, GraphOperation::Undo);
    assert_eq!(serialized(&app)["config"]["edges"][1]["to"], "wait");
    app.world_mut().resource_mut::<GraphNav>().selected_edge = Some(0);
    settle(&mut app);
    send_control(&mut app, GraphControl::DeleteSelection);
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .current_config()["edges"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    app.world_mut().write_message(AppAction::SaveToRemote);
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Idle
    ));
    assert!(app.world().resource::<StatusLine>().text.contains("入口"));
    send_operation(&mut app, GraphOperation::Undo);
    assert_eq!(
        serialized(&app)["config"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn 多字段批次中一项无效时全部拒绝且修正后一次提交() {
    let mut app = form_app("wait");
    let id = field_entity(&mut app, &["id"]);
    let poll = field_entity(&mut app, &["inputs", "poll_interval_ms"]);
    queue_text(&mut app, id, "renamed_wait");
    queue_text(&mut app, poll, "0");
    apply_button(&mut app);
    app.update();
    assert_eq!(node(&app, "wait")["inputs"]["poll_interval_ms"], 100);
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        0
    );
    queue_text(&mut app, poll, "50");
    apply_button(&mut app);
    app.update();
    assert_eq!(node(&app, "renamed_wait")["inputs"]["poll_interval_ms"], 50);
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        1
    );
    send_operation(&mut app, GraphOperation::Undo);
    assert_eq!(node(&app, "wait")["inputs"]["poll_interval_ms"], 100);
}

#[test]
fn 新增连接草稿可创建独立平行边并按选中身份删除撤销() {
    let mut app = form_app("message");
    send_control(&mut app, GraphControl::NewEdge);
    settle(&mut app);
    let from = field_entity(&mut app, &["from"]);
    let to = field_entity(&mut app, &["to"]);
    queue_text(&mut app, from, "message");
    queue_text(&mut app, to, "wait");
    apply_button(&mut app);
    app.update();
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    let graph = data.graph_edit.graph_key_at(&[]).unwrap();
    let original = data.graph_edit.edge_key_at(graph, 1).unwrap();
    let added = data.graph_edit.edge_key_at(graph, 3).unwrap();
    assert_ne!(original, added);
    assert_eq!(
        data.graph_edit.edge_json(added).unwrap(),
        &json!({"from":"message", "to":"wait"})
    );
    app.world_mut().resource_mut::<GraphNav>().selected_edge = Some(3);
    settle(&mut app);
    send_control(&mut app, GraphControl::DeleteSelection);
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    assert!(data.graph_edit.edge_json(added).is_none());
    assert_eq!(
        data.graph_edit.edge_json(original).unwrap()["label"],
        "true"
    );
    send_operation(&mut app, GraphOperation::Undo);
    assert!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .edge_json(added)
            .is_some()
    );
}

#[test]
fn 条件叶可由表单创建内联子图再移除且撤销完整恢复() {
    let mut app = form_app("choice");
    let choice = node_key(&app, "choice");
    send_control(&mut app, GraphControl::EditSubgraph(choice));
    settle(&mut app);
    let nodes = field_entity(&mut app, &["nodes"]);
    let edges = field_entity(&mut app, &["edges"]);
    queue_text(
        &mut app,
        nodes,
        r#"[{"id":"message","type":"log","inputs":{"message":"条件容器内"}}]"#,
    );
    queue_text(
        &mut app,
        edges,
        r#"[{"from":"_entry","to":"message","condition":"default"},{"from":"message","to":"_exit"}]"#,
    );
    apply_button(&mut app);
    app.update();
    let with_child = node(&app, "choice");
    assert_eq!(with_child["nodes"][0]["id"], "message");
    assert_eq!(with_child["inputs"]["condition"], "false");
    send_control(&mut app, GraphControl::RemoveSubgraph(choice));
    assert!(node(&app, "choice").get("nodes").is_none());
    assert!(node(&app, "choice").get("edges").is_none());
    assert_eq!(node(&app, "choice")["inputs"]["condition"], "false");
    send_operation(&mut app, GraphOperation::Undo);
    assert_eq!(node(&app, "choice"), with_child);
    assert_eq!(node(&app, "message")["inputs"]["message"], long_message());
}

#[test]
fn 类型改为未知业务节点保留属性并即时更新未核验说明() {
    let mut app = form_app("message");
    let input = field_entity(&mut app, &["type"]);
    let original = node(&app, "message");
    let generation = app.world().resource::<GraphEditing>().panel_version;
    queue_text(&mut app, input, "vendor_unregistered");
    apply_button(&mut app);
    app.update();
    assert_eq!(node(&app, "message")["type"], "vendor_unregistered");
    assert_eq!(node(&app, "message")["inputs"], original["inputs"]);
    assert_eq!(node(&app, "message")["vendor"], original["vendor"]);
    assert_eq!(
        app.world().resource::<GraphEditing>().panel_version,
        generation
    );
    assert_eq!(field_entity(&mut app, &["type"]), input);
    assert!(
        app.world_mut()
            .query::<&Text>()
            .iter(app.world())
            .any(|text| text.0.contains("尚未核验"))
    );
}

#[test]
fn 未应用输入切换文件先弹提示且真实取消按钮保留草稿() {
    let mut app = form_app_with_guard("message", true);
    let input = field_entity(&mut app, &["inputs", "message"]);
    queue_text(&mut app, input, "切换之前尚未应用");
    app.world_mut()
        .write_message(AppAction::LoadFile("next.json".into()));
    settle(&mut app);
    assert!(
        app.world()
            .resource::<crate::ui::document_guard::DocumentGuard>()
            .active()
    );
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Idle
    ));
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .document
            .as_ref()
            .unwrap()
            .filename,
        "fixture_task.json"
    );
    let cancel = action_button(&mut app, |action| {
        matches!(action, AppAction::CancelDiscard)
    });
    activate(&mut app, cancel);
    app.update();
    assert!(
        !app.world()
            .resource::<crate::ui::document_guard::DocumentGuard>()
            .active()
    );
    assert!(app.world().resource::<GraphEditing>().has_draft_changes());
    assert_eq!(
        app.world()
            .get::<EditableText>(input)
            .unwrap()
            .value()
            .to_string(),
        "切换之前尚未应用"
    );
    assert_eq!(node(&app, "message")["inputs"]["message"], long_message());
}

fn begin_guarded_save(app: &mut App) -> crate::worker::SaveTicket {
    app.world_mut().resource_mut::<FileBrowser>().remote_dir = Some("/fixture/graphs".into());
    let input = field_entity(app, &["inputs", "message"]);
    queue_text(app, input, "先保存这次输入");
    app.world_mut()
        .write_message(AppAction::LoadFile("next.json".into()));
    settle(app);
    let save = action_button(app, |action| matches!(action, AppAction::SaveAndContinue));
    let ticket = app.world().resource::<Editor>().next_save_ticket();
    activate(app, save);
    app.update();
    ticket
}

#[test]
fn 保存并继续必须等待相同提交成功才执行原切换动作() {
    let mut app = form_app_with_guard("message", true);
    let ticket = begin_guarded_save(&mut app);
    assert_eq!(node(&app, "message")["inputs"]["message"], "先保存这次输入");
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Working(_)
    ));
    assert!(
        app.world()
            .resource::<crate::ui::document_guard::DocumentGuard>()
            .active()
    );
    app.update();
    assert!(!matches!(
        app.world().resource::<Session>().busy,
        BusyState::Loading(_)
    ));
    assert!(
        app.world_mut()
            .resource_mut::<Editor>()
            .confirm_save(ticket)
    );
    app.world_mut().resource_mut::<Session>().busy = BusyState::Idle;
    app.update();
    app.update();
    assert!(
        matches!(&app.world().resource::<Session>().busy, BusyState::Loading(name) if name == "next.json")
    );
    assert!(
        !app.world()
            .resource::<crate::ui::document_guard::DocumentGuard>()
            .active()
    );
}

#[test]
fn 保存并继续遇到失败或期间新编辑不会执行切换() {
    for fail in [true, false] {
        let mut app = form_app_with_guard("message", true);
        let ticket = begin_guarded_save(&mut app);
        match fail {
            true => assert!(app.world_mut().resource_mut::<Editor>().fail_save(ticket)),
            false => {
                app.world_mut()
                    .resource_mut::<Editor>()
                    .data
                    .as_mut()
                    .unwrap()
                    .map_id = "保存期间的新地图".into();
                assert!(
                    app.world_mut()
                        .resource_mut::<Editor>()
                        .confirm_save(ticket)
                );
            }
        }
        app.world_mut().resource_mut::<Session>().busy = BusyState::Idle;
        settle(&mut app);
        assert!(
            app.world()
                .resource::<crate::ui::document_guard::DocumentGuard>()
                .active()
        );
        assert!(app.world().resource::<Editor>().has_unsaved_changes());
        assert!(matches!(
            app.world().resource::<Session>().busy,
            BusyState::Idle
        ));
        assert_eq!(
            app.world()
                .resource::<Editor>()
                .document
                .as_ref()
                .unwrap()
                .filename,
            "fixture_task.json"
        );
    }
}

#[test]
fn 旧放弃确认按钮不能清除新文档的未保存提示() {
    let mut app = form_app_with_guard("message", true);
    let input = field_entity(&mut app, &["inputs", "message"]);
    queue_text(&mut app, input, "旧草稿");
    app.world_mut()
        .write_message(AppAction::LoadFile("old_target.json".into()));
    settle(&mut app);
    let old_confirm = action_button(&mut app, |action| {
        matches!(action, AppAction::ConfirmDiscard(_))
    });
    let old_action = app
        .world()
        .get::<crate::ui::connect::ActionButton>(old_confirm)
        .unwrap()
        .clone();
    let replacement = fixture();
    app.world_mut()
        .resource_mut::<Editor>()
        .load_remote(replacement.data.unwrap(), replacement.document.unwrap());
    settle(&mut app);
    let input = field_entity(&mut app, &["inputs", "message"]);
    queue_text(&mut app, input, "新草稿");
    app.world_mut()
        .write_message(AppAction::LoadFile("new_target.json".into()));
    settle(&mut app);
    let stale = app.world_mut().spawn(old_action).id();
    activate(&mut app, stale);
    app.update();
    let guard = app
        .world()
        .resource::<crate::ui::document_guard::DocumentGuard>();
    assert!(guard.active());
    assert!(
        matches!(&guard.pending.as_ref().unwrap().action, AppAction::LoadFile(name) if name == "new_target.json")
    );
    assert!(app.world().resource::<GraphEditing>().has_draft_changes());
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Idle
    ));
}

#[test]
fn 跨帧旧面板按钮不能应用新面板草稿() {
    let mut app = form_app("message");
    let apply = operation_button(&mut app, |operation| {
        matches!(operation, GraphOperation::Control(GraphControl::Apply))
    });
    let old_button = app.world().get::<ControlButton>(apply).unwrap().clone();
    let old_generation = app.world().resource::<GraphEditing>().panel_version;
    app.world_mut().resource_mut::<GraphNav>().selected = Some("legacy".into());
    settle(&mut app);
    let current_apply = operation_button(&mut app, |operation| {
        matches!(operation, GraphOperation::Control(GraphControl::Apply))
    });
    let input = field_entity(&mut app, &["params", "message"]);
    queue_text(&mut app, input, "新面板待应用");
    app.update();
    let stale_panel = app.world_mut().spawn(PanelGeneration(old_generation)).id();
    let stale_button = app
        .world_mut()
        .spawn((old_button, ChildOf(stale_panel)))
        .id();
    activate(&mut app, stale_button);
    app.update();
    assert_eq!(node(&app, "legacy")["params"]["message"], "旧参数");
    assert!(app.world().resource::<GraphEditing>().has_draft_changes());
    activate(&mut app, current_apply);
    app.update();
    assert_eq!(node(&app, "legacy")["params"]["message"], "新面板待应用");
}

#[test]
fn 编辑模式真实进入子图按钮定位当前容器且不修改文档() {
    let mut app = form_app("group");
    let before = serialized(&app);
    let enter = operation_button(&mut app, |operation| {
        matches!(
            operation,
            GraphOperation::Control(GraphControl::EnterSubgraph(_))
        )
    });
    activate(&mut app, enter);
    app.update();
    assert_eq!(app.world().resource::<GraphNav>().path, ["group"]);
    assert!(app.world().resource::<GraphNav>().selected.is_none());
    assert_eq!(serialized(&app), before);
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        0
    );
}

#[test]
fn 窗口关闭消息先保护本帧草稿且明确放弃后才退出() {
    let mut app = form_app_with_guard("message", true);
    let window = app.world_mut().spawn(Window::default()).id();
    let input = field_entity(&mut app, &["inputs", "message"]);
    queue_text(&mut app, input, "关闭帧的最后输入");
    app.world_mut()
        .write_message(bevy::window::WindowCloseRequested { window });
    settle(&mut app);
    let guard = app
        .world()
        .resource::<crate::ui::document_guard::DocumentGuard>();
    assert!(matches!(
        guard.pending.as_ref().map(|pending| &pending.action),
        Some(AppAction::CloseWindow)
    ));
    assert!(app.world().resource::<Messages<AppExit>>().is_empty());
    assert!(app.world().resource::<GraphEditing>().has_draft_changes());
    let discard = action_button(&mut app, |action| {
        matches!(action, AppAction::ConfirmDiscard(_))
    });
    activate(&mut app, discard);
    app.update();
    assert!(!app.world().resource::<Messages<AppExit>>().is_empty());
    assert!(
        !app.world()
            .resource::<crate::ui::document_guard::DocumentGuard>()
            .active()
    );
}

#[test]
fn 流程撤销快捷键不抢文本焦点且离开输入框后才执行图历史() {
    let mut app = form_app("message");
    let input = field_entity(&mut app, &["inputs", "message"]);
    queue_text(&mut app, input, "已应用内容");
    apply_button(&mut app);
    app.update();
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(input, FocusCause::Navigated);
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::KeyZ);
    }
    app.update();
    assert_eq!(node(&app, "message")["inputs"]["message"], "已应用内容");
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        1
    );
    app.world_mut().resource_mut::<InputFocus>().clear();
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.reset_all();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::KeyZ);
    }
    app.update();
    assert_eq!(node(&app, "message")["inputs"]["message"], long_message());
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph_edit
            .revision(),
        2
    );
}
