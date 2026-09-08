use super::*;

fn editor() -> Editor {
    Editor {
        data: Some(crate::model::parse_task_graph(r#"{"map_id":"m","task_id":"t","config":{"context":{},"nodes":[{"id":"a","type":"sequence","nodes":[{"id":"child","type":"log"}],"edges":[]},{"id":"b","type":"log"}],"edges":[{"from":"_entry","to":"a"},{"from":"a","to":"b"},{"from":"b","to":"_exit"}]}}"#).unwrap()),
        ..default()
    }
}

fn app() -> App {
    let mut app = App::new();
    app.insert_resource(editor())
        .init_resource::<GraphNav>()
        .insert_resource(ViewMode::Graph);
    register(&mut app);
    app.world_mut().resource_mut::<GraphEditing>().enabled = true;
    app.add_systems(Update, (handle_input, prepare).chain());
    app.world_mut().spawn((Window::default(),));
    app.world_mut().spawn((
        CanvasViewport(0),
        ComputedNode {
            size: Vec2::new(2000.0, 1400.0),
            inverse_scale_factor: 1.0,
            ..default()
        },
        UiGlobalTransform::from(bevy::math::Affine2::from_translation(Vec2::new(
            1000.0, 700.0,
        ))),
        ScrollPosition::default(),
    ));
    app.update();
    app
}

fn pointer(app: &mut App, point: Vec2) {
    app.world_mut()
        .query::<&mut Window>()
        .single_mut(app.world_mut())
        .unwrap()
        .set_physical_cursor_position(Some(point.as_dvec2()));
}

fn tick(app: &mut App) {
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .clear();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
}

fn node(app: &App, id: &str) -> Vec2 {
    app.world()
        .resource::<CanvasState>()
        .layout
        .nodes
        .iter()
        .find(|node| node.id == id)
        .unwrap()
        .pos
}

#[test]
fn 一二倍dpi及不同全局ui缩放下图坐标命中一致() {
    for dpi in [1.0, 2.0] {
        for ui_scale in [0.5, 1.0, 1.5, 3.0] {
            for zoom in [MIN_ZOOM, 1.0, MAX_ZOOM] {
                let transform = Coordinates {
                    origin_physical: Vec2::new(400.0, 200.0),
                    inverse_scale: 1.0 / (dpi * ui_scale),
                    scroll: Vec2::new(200.0, 80.0),
                    zoom,
                };
                let point = Vec2::new(290.0, 412.0);
                assert!(transform.graph(transform.physical(point)).distance(point) < 0.001);
                let mut view = LayerView {
                    zoom,
                    scroll: Vec2::new(200.0, 80.0),
                    ..default()
                };
                let anchor = Vec2::new(100.0, 90.0);
                let before = (anchor + view.scroll) / view.zoom;
                zoom_at(&mut view, anchor, 1.3);
                assert!(((anchor + view.scroll) / view.zoom).distance(before) < 0.001);
            }
        }
    }
}

#[test]
fn 拖动预览改变节点和连线且取消完全恢复不产生模型历史() {
    let mut app = app();
    let original = node(&app, "a");
    let original_edges = app
        .world()
        .resource::<CanvasState>()
        .layout
        .edges
        .iter()
        .map(|edge| edge.points.clone())
        .collect::<Vec<_>>();
    pointer(&mut app, original + Vec2::new(20.0, 20.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, original + Vec2::new(420.0, 20.0));
    tick(&mut app);
    assert_eq!(node(&app, "a"), original + Vec2::new(400.0, 0.0));
    let state = app.world().resource::<CanvasState>();
    assert_ne!(
        state
            .layout
            .edges
            .iter()
            .map(|edge| edge.points.clone())
            .collect::<Vec<_>>(),
        original_edges
    );
    assert!(state.view().unwrap().positions.is_empty());
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Escape);
    tick(&mut app);
    assert_eq!(node(&app, "a"), original);
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    assert_eq!(data.graph_edit.revision(), 0);
    assert!(!data.graph_edit.can_undo());
    assert!(app.world().resource::<GraphNav>().path.is_empty());
}

#[test]
fn 拖动阈值与提交只影响本层本地位置() {
    let mut app = app();
    let original = node(&app, "a");
    let json_before =
        crate::model::serialize_task_graph(app.world().resource::<Editor>().data.as_ref().unwrap())
            .unwrap();
    pointer(&mut app, original + Vec2::new(30.0, 20.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, original + Vec2::new(32.0, 20.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert_eq!(node(&app, "a"), original);
    assert!(
        app.world()
            .resource::<CanvasState>()
            .view()
            .unwrap()
            .positions
            .is_empty()
    );
    pointer(&mut app, original + Vec2::new(30.0, 20.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, original + Vec2::new(430.0, 20.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert_eq!(node(&app, "a"), original + Vec2::new(400.0, 0.0));
    assert_eq!(
        app.world()
            .resource::<CanvasState>()
            .view()
            .unwrap()
            .positions
            .len(),
        1
    );
    assert_eq!(
        crate::model::serialize_task_graph(app.world().resource::<Editor>().data.as_ref().unwrap())
            .unwrap(),
        json_before
    );
}

#[test]
fn 框选两个节点后整体拖动保留彼此间距() {
    let mut app = app();
    let a = node(&app, "a");
    let b = node(&app, "b");
    pointer(&mut app, a.min(b) - Vec2::splat(16.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, a.max(b) + Vec2::new(NODE_W + 16.0, NODE_H + 16.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert_eq!(
        app.world().resource::<GraphEditing>().selected_nodes.len(),
        2
    );
    pointer(&mut app, a + Vec2::new(25.0, 25.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, a + Vec2::new(425.0, 25.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert_eq!(node(&app, "a") - a, Vec2::new(400.0, 0.0));
    assert_eq!(node(&app, "b") - b, Vec2::new(400.0, 0.0));
}

#[test]
fn 端口连线取消与悬空释放都不生成命令() {
    let mut app = app();
    let start = node(&app, "a") + Vec2::new(NODE_W / 2.0, NODE_H);
    pointer(&mut app, start);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Connect { .. }
    ));
    pointer(&mut app, Vec2::new(900.0, 900.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert!(app.world().resource::<Messages<AppAction>>().is_empty());
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Idle
    ));
}

#[test]
fn 输入输出端口直接连线只入统一动作队列不直接修改模型() {
    let mut app = app();
    let start = node(&app, "b") + Vec2::new(NODE_W / 2.0, NODE_H);
    let end = node(&app, "a") + Vec2::new(NODE_W / 2.0, 0.0);
    pointer(&mut app, start);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, end);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert!(!app.world().resource::<Messages<AppAction>>().is_empty());
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
fn 虚拟入口出口支持正反方向拖线且不暴露非法方向端口() {
    use crate::ui::graph_edit::GraphOperation;
    for (from, to, reverse) in [
        ("_entry", "b", false),
        ("a", "_exit", false),
        ("_entry", "_exit", true),
    ] {
        let mut app = app();
        let output = node(&app, from) + Vec2::new(NODE_W / 2.0, NODE_H);
        let input = node(&app, to) + Vec2::new(NODE_W / 2.0, 0.0);
        let state = app.world().resource::<CanvasState>();
        assert_eq!(
            hit_port(
                &state.layout,
                node(&app, "_entry") + Vec2::new(NODE_W / 2.0, 0.0),
                1.0
            ),
            None
        );
        assert_eq!(
            hit_port(
                &state.layout,
                node(&app, "_exit") + Vec2::new(NODE_W / 2.0, NODE_H),
                1.0
            ),
            None
        );
        let (start, end) = if reverse {
            (input, output)
        } else {
            (output, input)
        };
        pointer(&mut app, start);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        tick(&mut app);
        assert!(matches!(
            app.world().resource::<CanvasState>().gesture,
            Gesture::Connect { .. }
        ));
        pointer(&mut app, end);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        tick(&mut app);
        let actions: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<AppAction>>()
            .drain()
            .collect();
        assert_eq!(actions.len(), 1);
        let AppAction::GraphEdit(intent) = &actions[0] else {
            panic!("虚拟端口必须走统一动作")
        };
        let GraphOperation::Command(GraphCommand::AddEdge { value, .. }) = &intent.operation else {
            panic!("虚拟端口必须生成连线命令")
        };
        assert_eq!(value, &serde_json::json!({"from":from,"to":to}));
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
}

#[test]
fn 框选单节点同步详情目标且取消下一次框选恢复原选择() {
    let mut app = app();
    let a = node(&app, "a");
    pointer(&mut app, a - Vec2::splat(16.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, a + Vec2::new(NODE_W + 16.0, NODE_H + 16.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert_eq!(
        app.world().resource::<GraphNav>().selected.as_deref(),
        Some("a")
    );
    let original = app
        .world()
        .resource::<GraphEditing>()
        .selected_nodes
        .clone();
    assert_eq!(original.len(), 1);
    pointer(&mut app, Vec2::new(12.0, 12.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    assert!(
        app.world()
            .resource::<GraphEditing>()
            .selected_nodes
            .is_empty()
    );
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Escape);
    tick(&mut app);
    assert_eq!(
        app.world().resource::<GraphEditing>().selected_nodes,
        original
    );
    assert_eq!(
        app.world().resource::<GraphNav>().selected.as_deref(),
        Some("a")
    );
}

#[test]
fn 输入后退出编辑同帧取消拖动且已有手工位置仍保留() {
    use bevy::ecs::system::RunSystemOnce;
    let mut app = app();
    let original = node(&app, "a");
    pointer(&mut app, original + Vec2::new(20.0, 20.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, original + Vec2::new(420.0, 20.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    let committed = node(&app, "a");
    let positions = app
        .world()
        .resource::<CanvasState>()
        .view()
        .unwrap()
        .positions
        .clone();
    pointer(&mut app, committed + Vec2::new(20.0, 20.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, committed + Vec2::new(420.0, 20.0));
    tick(&mut app);
    assert_eq!(node(&app, "a"), committed + Vec2::new(400.0, 0.0));
    app.world_mut().resource_mut::<GraphEditing>().enabled = false;
    app.world_mut().run_system_once(prepare).unwrap();
    assert_eq!(node(&app, "a"), committed);
    assert_eq!(
        app.world()
            .resource::<CanvasState>()
            .view()
            .unwrap()
            .positions,
        positions
    );
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Idle
    ));
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
fn 图属性刷新不重建画布且跨帧新增类别色条同步最新值() {
    let mut app = app();
    app.add_systems(Update, sync_node_text.after(prepare));
    let card = app
        .world_mut()
        .spawn((
            GraphNodeMarker("b".into()),
            GraphNodeText {
                id: "b".into(),
                kind: true,
            },
            GraphNodeCategory("b".into()),
            GraphCheckpoint("b".into()),
            Text::new("log"),
            ThemeBackgroundColor(theme::GRAPH_LOG),
            Node {
                display: Display::None,
                ..default()
            },
        ))
        .id();
    tick(&mut app);
    let version = app.world().resource::<GraphNav>().version;
    let geometry = app.world().resource::<CanvasState>().geometry_revision;
    let mut editor = app.world_mut().resource_mut::<Editor>();
    let data = editor.data.as_mut().unwrap();
    let key = data
        .graph_edit
        .node_key(data.graph_edit.root_key(), "b")
        .unwrap();
    data.apply_graph_command(
        0,
        GraphCommand::Batch(vec![
            GraphCommand::SetNodeProperty {
                node: key,
                property: "inputs".into(),
                value: Some(serde_json::json!({"duration_ms": 100})),
            },
            GraphCommand::SetNodeProperty {
                node: key,
                property: "type".into(),
                value: Some(serde_json::json!("delay")),
            },
            GraphCommand::SetNodeProperty {
                node: key,
                property: "checkpoint".into(),
                value: Some(serde_json::json!(true)),
            },
        ]),
    )
    .unwrap();
    tick(&mut app);
    assert_eq!(app.world().get::<Text>(card).unwrap().0, "delay");
    assert_eq!(
        app.world().get::<ThemeBackgroundColor>(card).unwrap().0,
        theme::GRAPH_FLOW
    );
    assert_eq!(
        app.world().get::<Node>(card).unwrap().display,
        Display::Flex
    );
    assert_eq!(app.world().resource::<GraphNav>().version, version);
    assert_eq!(
        app.world().resource::<CanvasState>().geometry_revision,
        geometry
    );
    tick(&mut app);
    // 模拟属性修改前排队的 BSN 场景此时才落地，Editor/GraphNav 均没有新变化。
    let late = app
        .world_mut()
        .spawn((
            GraphNodeCategory("b".into()),
            ThemeBackgroundColor(theme::GRAPH_LOG),
        ))
        .id();
    tick(&mut app);
    assert_eq!(
        app.world().get::<ThemeBackgroundColor>(late).unwrap().0,
        theme::GRAPH_FLOW
    );
    assert!(app.world().get::<GraphNodeMarker>(card).is_some());
}

#[test]
fn 重载清除位置选择与未完成手势旧画布不能再收输入() {
    let mut app = app();
    let original = node(&app, "a");
    pointer(&mut app, original + Vec2::new(25.0, 25.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, original + Vec2::new(425.0, 25.0));
    tick(&mut app);
    let mut editor = app.world_mut().resource_mut::<Editor>();
    editor.document_version += 1;
    tick(&mut app);
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Idle
    ));
    assert!(
        app.world()
            .resource::<CanvasState>()
            .view()
            .unwrap()
            .positions
            .is_empty()
    );
    assert!(
        app.world()
            .resource::<GraphEditing>()
            .selected_nodes
            .is_empty()
    );
}

#[test]
fn 手工位置与子图视口按稳定身份跨改名保留() {
    let mut app = app();
    let graph = app.world().resource::<CanvasState>().active.unwrap();
    let key = app
        .world()
        .resource::<Editor>()
        .data
        .as_ref()
        .unwrap()
        .graph_edit
        .node_key(graph, "a")
        .unwrap();
    app.world_mut()
        .resource_mut::<CanvasState>()
        .views
        .get_mut(&graph)
        .unwrap()
        .positions
        .insert(key, Vec2::new(640.0, 200.0));
    app.world_mut()
        .resource_mut::<CanvasState>()
        .geometry_revision += 1;
    tick(&mut app);
    app.world_mut()
        .resource_mut::<Editor>()
        .data
        .as_mut()
        .unwrap()
        .apply_graph_command(
            0,
            GraphCommand::RenameNode {
                node: key,
                id: "renamed".into(),
            },
        )
        .unwrap();
    tick(&mut app);
    assert_eq!(node(&app, "renamed"), Vec2::new(640.0, 200.0));
    assert_eq!(
        app.world()
            .resource::<CanvasState>()
            .views
            .get(&graph)
            .unwrap()
            .positions
            .get(&key),
        Some(&Vec2::new(640.0, 200.0))
    );
}

#[test]
fn 光标离开窗口后松开会取消捕获且不留下拖动位置() {
    let mut app = app();
    let original = node(&app, "a");
    pointer(&mut app, original + Vec2::new(25.0, 25.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, original + Vec2::new(425.0, 25.0));
    tick(&mut app);
    app.world_mut()
        .query::<&mut Window>()
        .single_mut(app.world_mut())
        .unwrap()
        .set_physical_cursor_position(None);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Idle
    ));
    assert_eq!(node(&app, "a"), original);
    assert!(
        app.world()
            .resource::<CanvasState>()
            .view()
            .unwrap()
            .positions
            .is_empty()
    );
}

#[test]
fn 浏览模式不会拖动或连接而空格平移仍可用() {
    let mut app = app();
    app.world_mut().resource_mut::<GraphEditing>().enabled = false;
    let original = node(&app, "a");
    pointer(&mut app, original + Vec2::new(25.0, 25.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, original + Vec2::new(425.0, 25.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    assert_eq!(node(&app, "a"), original);
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Idle
    ));
    assert!(app.world().resource::<Messages<AppAction>>().is_empty());
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::Space);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Pan { .. }
    ));
}

#[test]
fn 明确重连终点使用稳定边身份且保留原起点() {
    use crate::ui::graph_edit::GraphOperation;
    let mut app = app();
    app.world_mut().resource_mut::<GraphNav>().selected_edge = Some(0);
    let graph = app.world().resource::<CanvasState>().active.unwrap();
    let expected = app
        .world()
        .resource::<Editor>()
        .data
        .as_ref()
        .unwrap()
        .graph_edit
        .edge_key_at(graph, 0)
        .unwrap();
    let button = app
        .world_mut()
        .spawn((CanvasAction::ReconnectTo, GraphSceneVersion(0)))
        .id();
    app.world_mut().trigger(Activate { entity: button });
    let end = node(&app, "b") + Vec2::new(NODE_W / 2.0, 0.0);
    pointer(&mut app, end);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    let actions: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<AppAction>>()
        .drain()
        .collect();
    assert_eq!(actions.len(), 1);
    let AppAction::GraphEdit(intent) = &actions[0] else {
        panic!("必须走流程动作")
    };
    let GraphOperation::Command(GraphCommand::ReconnectEdge { edge, from, to }) = &intent.operation
    else {
        panic!("必须是重连事务")
    };
    assert_eq!(*edge, expected);
    assert_eq!(from, "_entry");
    assert_eq!(to, "b");
    assert_eq!(intent.document_version, 0);
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .graph
            .edges[0]
            .to,
        "a"
    );
}

#[test]
fn 过期缩放与重连按钮不能作用于新画布() {
    let mut app = app();
    let zoom = app.world().resource::<CanvasState>().zoom();
    for action in [
        CanvasAction::ZoomIn,
        CanvasAction::ReconnectFrom,
        CanvasAction::Auto,
    ] {
        let button = app.world_mut().spawn((action, GraphSceneVersion(99))).id();
        app.world_mut().trigger(Activate { entity: button });
    }
    assert_eq!(app.world().resource::<CanvasState>().zoom(), zoom);
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Idle
    ));
    assert!(app.world().resource::<Messages<AppAction>>().is_empty());
}

#[test]
fn 切层保存独立视口与位置返回时恢复() {
    let mut app = app();
    let root = app.world().resource::<CanvasState>().active.unwrap();
    {
        let mut state = app.world_mut().resource_mut::<CanvasState>();
        let view = state.view_mut().unwrap();
        view.zoom = 1.7;
        view.scroll = Vec2::new(200.0, 400.0);
    }
    app.world_mut().resource_mut::<GraphNav>().enter("a");
    tick(&mut app);
    let child = app.world().resource::<CanvasState>().active.unwrap();
    assert_ne!(root, child);
    assert_eq!(app.world().resource::<CanvasState>().zoom(), 1.0);
    app.world_mut().resource_mut::<GraphNav>().go_to(0);
    tick(&mut app);
    let state = app.world().resource::<CanvasState>();
    assert_eq!(state.active, Some(root));
    assert_eq!(state.zoom(), 1.7);
    assert_eq!(state.view().unwrap().scroll, Vec2::new(200.0, 400.0));
}

#[test]
fn 适配与自动布局恢复不改变模型或撤销历史() {
    let mut app = app();
    app.add_systems(Update, sync_geometry.after(prepare));
    let viewport = app
        .world_mut()
        .query_filtered::<Entity, With<CanvasViewport>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .get_mut::<ComputedNode>(viewport)
        .unwrap()
        .size = Vec2::new(300.0, 260.0);
    let graph = app.world().resource::<CanvasState>().active.unwrap();
    let key = app
        .world()
        .resource::<Editor>()
        .data
        .as_ref()
        .unwrap()
        .graph_edit
        .node_key(graph, "a")
        .unwrap();
    {
        let mut state = app.world_mut().resource_mut::<CanvasState>();
        state
            .view_mut()
            .unwrap()
            .positions
            .insert(key, Vec2::new(700.0, 200.0));
        state.geometry_revision += 1;
    }
    tick(&mut app);
    let button = app
        .world_mut()
        .spawn((CanvasAction::Fit, GraphSceneVersion(0)))
        .id();
    app.world_mut().trigger(Activate { entity: button });
    tick(&mut app);
    let state = app.world().resource::<CanvasState>();
    assert!((MIN_ZOOM..=MAX_ZOOM).contains(&state.zoom()));
    assert!((state.layout.size * state.zoom()).x <= 300.0);
    let button = app
        .world_mut()
        .spawn((CanvasAction::Auto, GraphSceneVersion(0)))
        .id();
    app.world_mut().trigger(Activate { entity: button });
    tick(&mut app);
    assert!(
        app.world()
            .resource::<CanvasState>()
            .view()
            .unwrap()
            .positions
            .is_empty()
    );
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
fn 业务禁用态在通用门控前后顺序及资源变化和迟到bsn下均保持唯一所有者() {
    use crate::ui::{Session, widgets::ButtonGate};
    use bevy::scene::ScenePlugin;
    use bevy::ui::InteractionDisabled;

    for business_first in [true, false] {
        let mut app = app();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
            .init_asset::<Font>()
            .insert_resource(Session::new(
                crate::model::LoginConfig::default(),
                Vec::new(),
            ));
        match business_first {
            true => {
                app.add_systems(
                    Update,
                    (sync_controls, widgets::sync_button_gates)
                        .chain()
                        .after(prepare),
                );
            }
            false => {
                app.add_systems(
                    Update,
                    (widgets::sync_button_gates, sync_controls)
                        .chain()
                        .after(prepare),
                );
            }
        }
        let button = app
            .world_mut()
            .spawn_scene(canvas_control("重连起点", CanvasAction::ReconnectFrom))
            .unwrap()
            .id();
        let always = app
            .world_mut()
            .spawn_scene(widgets::button(
                "始终可用",
                ButtonVariant::Normal,
                CrumbMarker(0),
            ))
            .unwrap()
            .id();
        app.world_mut()
            .entity_mut(always)
            .insert(InteractionDisabled);
        for _ in 0..3 {
            tick(&mut app);
        }
        assert!(
            app.world()
                .get::<bevy::ui_widgets::Button>(button)
                .is_some()
        );
        assert!(matches!(
            app.world().get::<ButtonGate>(button),
            Some(ButtonGate::Managed)
        ));
        assert!(app.world().get::<InteractionDisabled>(button).is_some());
        assert!(
            app.world().get::<InteractionDisabled>(always).is_none(),
            "Always仍须主动解除禁用"
        );

        for reconnecting in [true, false] {
            app.world_mut().resource_mut::<Editor>().value_version += 1;
            app.world_mut().resource_mut::<Session>().reconnect_status =
                reconnecting.then(|| "重连中".into());
            tick(&mut app);
            assert!(
                app.world().get::<InteractionDisabled>(button).is_some(),
                "Editor/Session变更不能撤销业务禁用"
            );
        }
        tick(&mut app);
        // 资源已稳定数帧后才生成另一批真实BSN按钮，业务门控仍须初始化其禁用态。
        let late = app
            .world_mut()
            .spawn_scene(canvas_control("重连终点", CanvasAction::ReconnectTo))
            .unwrap()
            .id();
        for _ in 0..3 {
            tick(&mut app);
        }
        assert!(app.world().get::<InteractionDisabled>(late).is_some());
        app.world_mut().resource_mut::<GraphNav>().selected_edge = Some(0);
        tick(&mut app);
        assert!(app.world().get::<InteractionDisabled>(button).is_none());
        assert!(
            app.world().get::<InteractionDisabled>(late).is_none(),
            "业务条件满足后须正常恢复可用"
        );
    }
}

#[test]
fn 原生ui拾取命中文字子节点时各dpi缩放下仍选中并拖动所属卡片() {
    use bevy::app::HierarchyPropagatePlugin;
    use bevy::camera::{ComputedCameraValues, RenderTarget, RenderTargetInfo};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::picking::backend::PointerHits;
    use bevy::picking::hover::{PreviousHoverMap, generate_hovermap};
    use bevy::picking::pointer::{Location, PointerId, PointerInput, PointerLocation};
    use bevy::ui::picking_backend::{UiPickingSettings, ui_picking};
    use bevy::ui::{ComputedUiRenderTargetInfo, ComputedUiTargetCamera, UiStack, UiTargetCamera};
    use bevy::window::WindowRef;

    for dpi in [1.0, 2.0] {
        for ui_scale in [0.5, 1.0, 1.5] {
            for zoom in [0.5, 1.0, 2.0] {
                let mut app = app();
                app.insert_resource(UiScale(ui_scale))
                    .init_resource::<UiPickingSettings>()
                    .init_resource::<UiStack>()
                    .init_resource::<HoverMap>()
                    .init_resource::<PreviousHoverMap>()
                    .add_message::<PointerHits>()
                    .add_message::<PointerInput>()
                    .add_plugins(HierarchyPropagatePlugin::<ComputedUiTargetCamera>::new(
                        PostUpdate,
                    ))
                    .add_plugins(HierarchyPropagatePlugin::<ComputedUiRenderTargetInfo>::new(
                        PostUpdate,
                    ))
                    .add_systems(Update, bevy::ui::update::propagate_ui_target_cameras);
                let window = app
                    .world_mut()
                    .query_filtered::<Entity, With<Window>>()
                    .single(app.world())
                    .unwrap();
                // 与下面相机的物理渲染目标一致；否则高缩放拖动会越过默认 1280×720 窗口，
                // Window::physical_cursor_position 正确返回 None，测到的是离窗而非坐标转换。
                app.world_mut()
                    .get_mut::<Window>(window)
                    .unwrap()
                    .resolution = bevy::window::WindowResolution::new(12000, 12000)
                    .with_scale_factor_override(dpi);
                let target = RenderTarget::Window(WindowRef::Entity(window));
                let camera = app
                    .world_mut()
                    .spawn((
                        Camera {
                            computed: ComputedCameraValues {
                                target_info: Some(RenderTargetInfo {
                                    physical_size: UVec2::splat(12000),
                                    scale_factor: dpi,
                                }),
                                ..default()
                            },
                            ..default()
                        },
                        target.clone(),
                    ))
                    .id();
                let scale = dpi * ui_scale;
                let origin = Vec2::new(300.0, 200.0);
                let viewport = app
                    .world_mut()
                    .query_filtered::<Entity, With<CanvasViewport>>()
                    .single(app.world())
                    .unwrap();
                let view_size = Vec2::new(2000.0, 1400.0) * scale;
                app.world_mut().entity_mut(viewport).insert((
                    Node {
                        overflow: Overflow::scroll(),
                        ..default()
                    },
                    ComputedNode {
                        size: view_size,
                        inverse_scale_factor: 1.0 / scale,
                        ..default()
                    },
                    UiGlobalTransform::from(bevy::math::Affine2::from_translation(
                        origin + view_size / 2.0,
                    )),
                    UiTargetCamera(camera),
                    InheritedVisibility::VISIBLE,
                ));
                let position = node(&app, "a");
                let transform_at = |position: Vec2| {
                    UiGlobalTransform::from(bevy::math::Affine2::from_scale_angle_translation(
                        Vec2::splat(zoom),
                        0.0,
                        origin + position * zoom * scale,
                    ))
                };
                let card = app
                    .world_mut()
                    .spawn((
                        Node::default(),
                        GraphNodeMarker("a".into()),
                        GraphSceneVersion(0),
                        ComputedNode {
                            size: Vec2::new(NODE_W, NODE_H) * scale,
                            inverse_scale_factor: 1.0 / scale,
                            ..default()
                        },
                        transform_at(position + Vec2::new(NODE_W, NODE_H) / 2.0),
                        InheritedVisibility::VISIBLE,
                        ChildOf(viewport),
                    ))
                    .id();
                let text_position = position + Vec2::new(40.0, 24.0);
                let label = app
                    .world_mut()
                    .spawn((
                        Text::new("节点正文"),
                        ComputedNode {
                            size: Vec2::new(60.0, 18.0) * scale,
                            inverse_scale_factor: 1.0 / scale,
                            ..default()
                        },
                        transform_at(text_position),
                        InheritedVisibility::VISIBLE,
                        ChildOf(card),
                    ))
                    .id();
                // 上游自己传播相机归属；不通过私有字段伪造 ComputedUiTargetCamera。
                app.update();
                app.update();
                app.world_mut()
                    .resource_mut::<CanvasState>()
                    .view_mut()
                    .unwrap()
                    .zoom = zoom;
                let physical = origin + text_position * zoom * scale;
                app.world_mut()
                    .get_mut::<Window>(window)
                    .unwrap()
                    .set_physical_cursor_position(Some(physical.as_dvec2()));
                app.world_mut().spawn((
                    PointerId::Mouse,
                    PointerLocation::new(Location {
                        target: target.normalize(Some(window)).unwrap(),
                        position: physical / dpi,
                    }),
                ));
                *app.world_mut().resource_mut::<UiStack>() = UiStack {
                    partition: std::iter::once(0..3).collect(),
                    uinodes: vec![viewport, card, label],
                };
                app.world_mut().run_system_once(ui_picking).unwrap();
                let hits: Vec<_> = app
                    .world_mut()
                    .resource_mut::<Messages<PointerHits>>()
                    .drain()
                    .collect();
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].picks[0].0, label, "拾取必须落到最内层文字");
                app.world_mut()
                    .resource_mut::<Messages<PointerHits>>()
                    .write_batch(hits);
                app.world_mut().run_system_once(generate_hovermap).unwrap();
                assert!(app.world().resource::<HoverMap>()[&PointerId::Mouse].contains_key(&label));
                app.world_mut()
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .press(MouseButton::Left);
                app.world_mut().run_system_once(handle_input).unwrap();
                assert_eq!(
                    app.world().resource::<GraphNav>().selected.as_deref(),
                    Some("a")
                );
                app.world_mut()
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .clear();
                app.world_mut()
                    .get_mut::<Window>(window)
                    .unwrap()
                    .set_physical_cursor_position(Some(
                        (physical + Vec2::new(400.0, 0.0) * zoom * scale).as_dvec2(),
                    ));
                app.world_mut().run_system_once(handle_input).unwrap();
                app.world_mut().run_system_once(prepare).unwrap();
                assert!((node(&app, "a") - position - Vec2::new(400.0, 0.0)).length() < 0.01);
                assert!(app.world().resource::<GraphNav>().path.is_empty());
            }
        }
    }
}

#[test]
fn 原生拾取浮层遮挡时不穿透选择缩放与滚轮且已捕获拖动可继续() {
    use bevy::app::HierarchyPropagatePlugin;
    use bevy::camera::{ComputedCameraValues, RenderTarget, RenderTargetInfo};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::picking::backend::PointerHits;
    use bevy::picking::hover::{PreviousHoverMap, generate_hovermap};
    use bevy::picking::pointer::{Location, PointerInput, PointerLocation};
    use bevy::ui::picking_backend::{UiPickingSettings, ui_picking};
    use bevy::ui::{ComputedUiRenderTargetInfo, ComputedUiTargetCamera, UiStack, UiTargetCamera};
    use bevy::window::WindowRef;

    fn pick(app: &mut App) {
        app.world_mut()
            .resource_mut::<Messages<PointerHits>>()
            .clear();
        app.world_mut().run_system_once(ui_picking).unwrap();
        app.world_mut().run_system_once(generate_hovermap).unwrap();
    }

    let mut app = app();
    app.init_resource::<UiScale>()
        .init_resource::<UiPickingSettings>()
        .init_resource::<UiStack>()
        .init_resource::<HoverMap>()
        .init_resource::<PreviousHoverMap>()
        .add_message::<PointerHits>()
        .add_message::<PointerInput>()
        .add_plugins(HierarchyPropagatePlugin::<ComputedUiTargetCamera>::new(
            PostUpdate,
        ))
        .add_plugins(HierarchyPropagatePlugin::<ComputedUiRenderTargetInfo>::new(
            PostUpdate,
        ))
        .add_systems(Update, bevy::ui::update::propagate_ui_target_cameras);
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<Window>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .resolution = bevy::window::WindowResolution::new(2000, 1400);
    let target = RenderTarget::Window(WindowRef::Entity(window));
    let camera = app
        .world_mut()
        .spawn((
            Camera {
                computed: ComputedCameraValues {
                    target_info: Some(RenderTargetInfo {
                        physical_size: UVec2::new(2000, 1400),
                        scale_factor: 1.0,
                    }),
                    ..default()
                },
                ..default()
            },
            target.clone(),
        ))
        .id();
    let viewport = app
        .world_mut()
        .query_filtered::<Entity, With<CanvasViewport>>()
        .single(app.world())
        .unwrap();
    app.world_mut().entity_mut(viewport).insert((
        Node::default(),
        UiTargetCamera(camera),
        InheritedVisibility::VISIBLE,
    ));
    let original = node(&app, "a");
    let transform =
        |position| UiGlobalTransform::from(bevy::math::Affine2::from_translation(position));
    let card = app
        .world_mut()
        .spawn((
            Node::default(),
            GraphNodeMarker("a".into()),
            GraphSceneVersion(0),
            ComputedNode {
                size: Vec2::new(NODE_W, NODE_H),
                inverse_scale_factor: 1.0,
                ..default()
            },
            transform(original + Vec2::new(NODE_W, NODE_H) / 2.0),
            InheritedVisibility::VISIBLE,
            ChildOf(viewport),
        ))
        .id();
    let point = original + Vec2::new(40.0, 24.0);
    let label = app
        .world_mut()
        .spawn((
            Text::new("节点正文"),
            ComputedNode {
                size: Vec2::new(60.0, 18.0),
                inverse_scale_factor: 1.0,
                ..default()
            },
            transform(point),
            InheritedVisibility::VISIBLE,
            ChildOf(card),
        ))
        .id();
    let overlay = app
        .world_mut()
        .spawn((
            Node::default(),
            Text::new("浮层菜单"),
            UiTargetCamera(camera),
            ComputedNode {
                size: Vec2::new(260.0, 100.0),
                inverse_scale_factor: 1.0,
                ..default()
            },
            transform(point),
            InheritedVisibility::VISIBLE,
        ))
        .id();
    app.update();
    app.update();
    let pointer_entity = app
        .world_mut()
        .spawn((
            PointerId::Mouse,
            PointerLocation::new(Location {
                target: target.normalize(Some(window)).unwrap(),
                position: point,
            }),
        ))
        .id();
    pointer(&mut app, point);
    *app.world_mut().resource_mut::<UiStack>() = UiStack {
        partition: std::iter::once(0..4).collect(),
        uinodes: vec![viewport, card, label, overlay],
    };
    pick(&mut app);
    let hovered = &app.world().resource::<HoverMap>()[&PointerId::Mouse];
    assert!(hovered.contains_key(&overlay));
    assert!(!hovered.contains_key(&label));
    assert_eq!(
        hit_node(&app.world().resource::<CanvasState>().layout, point)
            .unwrap()
            .id,
        "a",
        "同一坐标的底层几何确实命中卡片，必须由真实拾取阻断"
    );
    for modified in [false, true] {
        if modified {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::ControlLeft);
        }
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut()
            .resource_mut::<Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: MouseScrollUnit::Line,
                x: 0.0,
                y: -3.0,
                window,
                phase: bevy::input::touch::TouchPhase::Moved,
            });
        app.world_mut().run_system_once(handle_input).unwrap();
        assert_eq!(app.world().resource::<GraphNav>().selected, None);
        let state = app.world().resource::<CanvasState>();
        assert!(matches!(state.gesture, Gesture::Idle));
        assert_eq!(state.zoom(), 1.0);
        assert_eq!(state.view().unwrap().scroll, Vec2::ZERO);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<Messages<MouseWheel>>()
            .clear();
    }
    app.world_mut()
        .entity_mut(overlay)
        .insert(InheritedVisibility::HIDDEN);
    pick(&mut app);
    assert!(app.world().resource::<HoverMap>()[&PointerId::Mouse].contains_key(&label));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    app.world_mut().run_system_once(handle_input).unwrap();
    assert_eq!(
        app.world().resource::<GraphNav>().selected.as_deref(),
        Some("a")
    );
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .clear();
    let moved = point + Vec2::new(400.0, 0.0);
    pointer(&mut app, moved);
    app.world_mut()
        .get_mut::<PointerLocation>(pointer_entity)
        .unwrap()
        .location
        .as_mut()
        .unwrap()
        .position = moved;
    app.world_mut()
        .entity_mut(overlay)
        .insert((InheritedVisibility::VISIBLE, transform(moved)));
    pick(&mut app);
    assert!(app.world().resource::<HoverMap>()[&PointerId::Mouse].contains_key(&overlay));
    app.world_mut().run_system_once(handle_input).unwrap();
    app.world_mut().run_system_once(prepare).unwrap();
    assert_eq!(node(&app, "a"), original + Vec2::new(400.0, 0.0));
}

#[test]
fn 未保存确认层取消拖动预览并拦截几何点击滚轮与激活操作() {
    let mut app = app();
    let original = node(&app, "a");
    pointer(&mut app, original + Vec2::new(25.0, 25.0));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    pointer(&mut app, original + Vec2::new(425.0, 25.0));
    tick(&mut app);
    assert_eq!(node(&app, "a"), original + Vec2::new(400.0, 0.0));

    let mut guard = DocumentGuard::default();
    guard.request(app.world().resource::<Editor>(), AppAction::CloseWindow);
    app.insert_resource(guard);
    // 模拟确认层在 Input 之后才打开；同帧 Rebuild 的第二道守卫必须马上恢复原布局。
    use bevy::ecs::system::RunSystemOnce;
    app.world_mut().run_system_once(prepare).unwrap();
    assert_eq!(node(&app, "a"), original);
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Idle
    ));
    assert!(
        app.world()
            .resource::<CanvasState>()
            .view()
            .unwrap()
            .positions
            .is_empty()
    );

    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    tick(&mut app);
    let zoom = app.world().resource::<CanvasState>().zoom();
    let layout_revision = app.world().resource::<CanvasState>().geometry_revision;
    for action in [
        CanvasAction::ZoomIn,
        CanvasAction::ZoomOut,
        CanvasAction::Actual,
        CanvasAction::Fit,
        CanvasAction::Auto,
        CanvasAction::ReconnectFrom,
        CanvasAction::ReconnectTo,
    ] {
        let button = app.world_mut().spawn((action, GraphSceneVersion(0))).id();
        app.world_mut().trigger(Activate { entity: button });
    }
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<Window>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ControlLeft);
    app.world_mut()
        .resource_mut::<Messages<MouseWheel>>()
        .write(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: 3.0,
            window,
            phase: bevy::input::touch::TouchPhase::Moved,
        });
    pointer(&mut app, original + Vec2::new(NODE_W / 2.0, NODE_H));
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    tick(&mut app);
    assert_eq!(app.world().resource::<CanvasState>().zoom(), zoom);
    assert_eq!(
        app.world().resource::<CanvasState>().geometry_revision,
        layout_revision
    );
    assert!(matches!(
        app.world().resource::<CanvasState>().gesture,
        Gesture::Idle
    ));
    assert!(app.world().resource::<Messages<AppAction>>().is_empty());
    assert!(!app.world().resource::<CanvasState>().fit_requested);
}
