//! 流程浏览的异常输入、导航及独立连线选择回归；不创建原生窗口或读取用户配置。

use super::tests::{graph_app, settle_scenes, test_editor};
use super::*;
use serde_json::json;

fn editor_with_config(config: serde_json::Value) -> Editor {
    let mut editor = Editor::default();
    editor.load(Some(
        crate::model::parse_task_graph(
            &json!({"map_id":"m", "task_id":"t", "config":config}).to_string(),
        )
        .unwrap(),
    ));
    editor
}

fn texts(app: &mut App) -> Vec<String> {
    app.world_mut()
        .query::<&Text>()
        .iter(app.world())
        .map(|text| text.0.clone())
        .collect()
}

fn add_graph(app: &mut App) {
    app.world_mut().spawn((GraphSlot, Node::default()));
    settle_scenes(app);
}

#[test]
fn 重复或保留身份显示原因而不构建歧义卡片() {
    for ids in [["a", "a", "b"], ["_entry", "a", "b"], [" ", "a", "b"]] {
        let config = json!({
            "context":{"speed":1},
            "nodes":ids.map(|id| json!({"id":id,"type":"log"})),
            "edges":[{"from":"a","to":"b"}]
        });
        let mut app = graph_app(editor_with_config(config.clone()));
        add_graph(&mut app);
        let messages = texts(&mut app);
        assert!(messages.iter().any(|text| text.contains("本层无法绘制")));
        assert!(messages.iter().any(|text| text.contains("$.config.nodes[")));
        assert!(
            messages
                .iter()
                .any(|text| text.contains("可绘制 0 节点 / 0 边"))
        );
        assert_eq!(
            app.world_mut()
                .query::<&GraphNodeMarker>()
                .iter(app.world())
                .count(),
            0
        );
        let data = app.world().resource::<Editor>().data.as_ref().unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&crate::model::serialize_task_graph(data).unwrap()).unwrap();
        assert_eq!(saved["config"], config);
    }
}

#[test]
fn 悬空边和非法条目显示原始与可绘制条数() {
    let mut app = graph_app(editor_with_config(json!({
        "nodes":[{"id":"a","type":"log"}, {"id":"b","type":"log"}],
        "edges":[{"from":"a","to":12}, {"from":"a","to":"ghost"}, {"from":"a","to":"b"}]
    })));
    add_graph(&mut app);
    let messages = texts(&mut app);
    assert!(
        messages
            .iter()
            .any(|text| text.contains("原始 2 节点 / 3 边 · 可绘制 2 节点 / 1 边"))
    );
    assert!(
        messages
            .iter()
            .any(|text| text.contains("$.config.edges[1].to") && text.contains("ghost"))
    );
    let markers: Vec<_> = app
        .world_mut()
        .query::<&GraphEdgeMarker>()
        .iter(app.world())
        .map(|marker| marker.index)
        .collect();
    assert!(!markers.is_empty());
    assert!(
        markers.iter().all(|index| *index == 1),
        "源边索引不能按过滤后的可见序号重新编号"
    );
}

#[test]
fn 错误子图有入口徽标且进入后展示具体路径() {
    let mut app = graph_app(editor_with_config(json!({
        "nodes":[{"id":"bad","type":"sequence","nodes":null,"edges":[]}]
    })));
    add_graph(&mut app);
    assert!(texts(&mut app).iter().any(|text| text == "问题 1 ▸"));
    app.world_mut().resource_mut::<GraphNav>().enter("bad");
    settle_scenes(&mut app);
    assert!(
        texts(&mut app)
            .iter()
            .any(|text| text.contains("$.config.nodes[0].nodes") && text.contains("数组"))
    );
}

#[test]
fn 参数结构变化保留子图画布选择和双轴滚动() {
    let mut app = graph_app(test_editor());
    add_graph(&mut app);
    app.world_mut().resource_mut::<GraphNav>().enter("step");
    settle_scenes(&mut app);
    app.world_mut().resource_mut::<GraphNav>().selected = Some("child".into());
    settle_scenes(&mut app);
    let canvas = app
        .world_mut()
        .query_filtered::<Entity, With<CanvasNeedsCenter>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .entity_mut(canvas)
        .remove::<CanvasNeedsCenter>();
    let scroll = Vec2::new(71.0, 413.0);
    app.world_mut().get_mut::<ScrollPosition>(canvas).unwrap().0 = scroll;
    let nav_version = app.world().resource::<GraphNav>().version;
    let document_version = app.world().resource::<Editor>().document_version;
    app.world_mut()
        .resource_mut::<Editor>()
        .mark_structure_changed();
    settle_scenes(&mut app);
    let nav = app.world().resource::<GraphNav>();
    assert_eq!(nav.path, ["step"]);
    assert_eq!(nav.selected.as_deref(), Some("child"));
    assert_eq!(nav.version, nav_version);
    assert_eq!(
        app.world().resource::<Editor>().document_version,
        document_version
    );
    assert_eq!(app.world().get::<ScrollPosition>(canvas).unwrap().0, scroll);
}

#[test]
fn 详情场景仍在跨帧队列时切层或重载不会产生无父节点() {
    for reload in [false, true] {
        let mut app = graph_app(test_editor());
        add_graph(&mut app);
        let graph_slot = app
            .world_mut()
            .query_filtered::<Entity, With<GraphSlot>>()
            .single(app.world())
            .unwrap();
        let detail_slot = app
            .world_mut()
            .query_filtered::<Entity, With<GraphDetailSlot>>()
            .single(app.world())
            .unwrap();
        app.world_mut().resource_mut::<GraphNav>().selected = Some("step".into());
        // 真实生产系统排入 BSN，但暂不执行 SpawnScene，确定性模拟资源尚未落地的间隔。
        app.world_mut().run_schedule(Update);
        assert!(
            app.world()
                .get::<widgets::SlotPending>(detail_slot)
                .is_some()
        );
        assert!(app.world().get::<Children>(detail_slot).is_none());
        match reload {
            true => {
                let data = app.world().resource::<Editor>().data.clone();
                app.world_mut().resource_mut::<Editor>().load(data);
            }
            false => app.world_mut().resource_mut::<GraphNav>().enter("step"),
        }
        app.world_mut().run_schedule(Update);
        settle_scenes(&mut app);
        settle_scenes(&mut app);
        assert!(app.world().get_entity(detail_slot).is_err());
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
            assert_eq!(
                current, graph_slot,
                "旧详情不能脱离画布成为顶层 UI：{entity}"
            );
        }
        assert_eq!(
            app.world().resource::<GraphNav>().path,
            if reload {
                Vec::<String>::new()
            } else {
                vec!["step".into()]
            }
        );
    }
}

#[test]
fn 重载相同文档与清空都会清除导航和连线选择() {
    let mut app = graph_app(test_editor());
    add_graph(&mut app);
    let data = app.world().resource::<Editor>().data.clone();
    for next in [data, None] {
        {
            let mut nav = app.world_mut().resource_mut::<GraphNav>();
            nav.enter("step");
            nav.selected = Some("child".into());
            nav.selected_edge = Some(0);
        }
        let version = app.world().resource::<Editor>().document_version;
        app.world_mut().resource_mut::<Editor>().load(next);
        settle_scenes(&mut app);
        let nav = app.world().resource::<GraphNav>();
        assert!(nav.path.is_empty());
        assert!(nav.selected.is_none());
        assert!(nav.selected_edge.is_none());
        assert_eq!(
            app.world().resource::<Editor>().document_version,
            version + 1
        );
    }
}

#[test]
fn 失效子图路径只退回最近有效祖先() {
    let mut app = graph_app(test_editor());
    add_graph(&mut app);
    app.world_mut().resource_mut::<GraphNav>().enter("step");
    app.world_mut().resource_mut::<GraphNav>().enter("missing");
    app.world_mut()
        .resource_mut::<Editor>()
        .mark_structure_changed();
    settle_scenes(&mut app);
    assert_eq!(app.world().resource::<GraphNav>().path, ["step"]);
}

#[test]
fn 选边高亮整条路径并保留画布且同端点边身份独立() {
    let mut app = graph_app(editor_with_config(json!({
        "nodes":[{"id":"a","type":"log"},{"id":"b","type":"log"}],
        "edges":[{"from":"a","to":"b","tag":"first"},{"from":"a","to":"b","tag":"second"}]
    })));
    add_graph(&mut app);
    let canvas = app
        .world_mut()
        .query_filtered::<Entity, With<CanvasNeedsCenter>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .entity_mut(canvas)
        .remove::<CanvasNeedsCenter>();
    let scroll = Vec2::new(50.0, 80.0);
    app.world_mut().get_mut::<ScrollPosition>(canvas).unwrap().0 = scroll;
    let version = app.world().resource::<GraphNav>().version;
    for index in [0, 1] {
        let part = app
            .world_mut()
            .query::<(Entity, &GraphEdgeMarker)>()
            .iter(app.world())
            .find(|(_, edge)| edge.index == index)
            .unwrap()
            .0;
        *app.world_mut().get_mut::<Interaction>(part).unwrap() = Interaction::Pressed;
        app.update();
        *app.world_mut().get_mut::<Interaction>(part).unwrap() = Interaction::None;
        settle_scenes(&mut app);
        assert_eq!(
            app.world().resource::<GraphNav>().selected_edge,
            Some(index)
        );
        assert_eq!(app.world().resource::<GraphNav>().version, version);
        assert_eq!(app.world().get::<ScrollPosition>(canvas).unwrap().0, scroll);
        let mut count = 0;
        for (visual, background, text) in app
            .world_mut()
            .query::<(
                &GraphEdgeVisual,
                Option<&ThemeBackgroundColor>,
                Option<&ThemeTextColor>,
            )>()
            .iter(app.world())
        {
            let token = text
                .map(|color| &color.0)
                .or_else(|| background.map(|color| &color.0))
                .unwrap();
            match visual.index == index {
                true => {
                    assert_eq!(*token, theme::GRAPH_SELECTED_BORDER);
                    count += 1;
                }
                false => assert_eq!(*token, theme::GRAPH_BORDER),
            }
        }
        assert!(count >= 2, "线段与箭头都必须刷新");
        let expected = match index {
            0 => "first",
            _ => "second",
        };
        assert!(texts(&mut app).iter().any(|text| text.contains(expected)));
    }
}

#[test]
fn 悬停连线无需选中且离开后恢复已选路径() {
    let mut app = graph_app(editor_with_config(json!({
        "nodes":[{"id":"a","type":"log"},{"id":"b","type":"log"}],
        "edges":[{"from":"a","to":"b"},{"from":"b","to":"a"}]
    })));
    add_graph(&mut app);
    let part = app
        .world_mut()
        .query::<(Entity, &GraphEdgeMarker)>()
        .iter(app.world())
        .find(|(_, edge)| edge.index == 1)
        .unwrap()
        .0;
    app.world_mut().resource_mut::<GraphNav>().selected_edge = Some(0);
    *app.world_mut().get_mut::<Interaction>(part).unwrap() = Interaction::Hovered;
    app.update();
    assert_eq!(app.world().resource::<HoveredEdge>().0, Some(1));
    assert_eq!(app.world().resource::<GraphNav>().selected_edge, Some(0));
    *app.world_mut().get_mut::<Interaction>(part).unwrap() = Interaction::None;
    app.update();
    assert_eq!(app.world().resource::<HoveredEdge>().0, None);
    for (edge, color) in app
        .world_mut()
        .query::<(&GraphEdgeVisual, &ThemeBackgroundColor)>()
        .iter(app.world())
    {
        if edge.index == 0 {
            assert_eq!(color.0, theme::GRAPH_SELECTED_BORDER);
        }
    }
}

#[test]
fn 旧画布连线事件不能选中新子图() {
    let mut app = graph_app(test_editor());
    settle_scenes(&mut app);
    let old_version = app.world().resource::<GraphNav>().version;
    app.world_mut().spawn((
        Interaction::Pressed,
        GraphEdgeMarker {
            index: 0,
            version: old_version,
        },
    ));
    app.world_mut().resource_mut::<GraphNav>().enter("step");
    app.update();
    assert!(app.world().resource::<GraphNav>().selected_edge.is_none());
    assert_eq!(app.world().resource::<HoveredEdge>().0, None);
}

#[test]
fn 原始边查找跳过坏端点但不合并平行边() {
    let editor = editor_with_config(json!({
        "nodes":[{"id":"group","type":"sequence","nodes":[{"id":"a","type":"log"}],
            "edges":[{"from":5,"to":"a"},{"from":"a","to":"a","data":1},{"from":"a","to":"a","data":2}]}]
    }));
    let data = editor.data.as_ref().unwrap();
    let path = vec!["group".into()];
    for (index, expected) in [(0, 1), (1, 2)] {
        let (source, raw) = raw_edge_at(data, &path, index).unwrap();
        assert_eq!(source, expected);
        assert_eq!(raw["data"], expected);
    }
}

#[test]
fn 旧图层卡片下钻和面包屑事件不能作用于跨层同名节点() {
    let mut app = graph_app(editor_with_config(json!({
        "nodes":[{"id":"same","type":"sequence","nodes":[
            {"id":"same","type":"sequence","nodes":[]}
        ]}]
    })));
    settle_scenes(&mut app);
    let old_version = app.world().resource::<GraphNav>().version;
    let old_scene = app.world_mut().spawn(GraphSceneVersion(old_version)).id();
    app.world_mut().spawn((
        GraphNodeMarker("same".into()),
        Interaction::Pressed,
        ChildOf(old_scene),
    ));
    app.world_mut()
        .spawn((CrumbMarker(0), Interaction::Pressed, ChildOf(old_scene)));
    let drill = app
        .world_mut()
        .spawn((DrillButton("same".into()), ChildOf(old_scene)))
        .id();
    app.world_mut().resource_mut::<GraphNav>().enter("same");
    app.world_mut().trigger(Activate { entity: drill });
    app.update();
    let nav = app.world().resource::<GraphNav>();
    assert_eq!(nav.path, ["same"]);
    assert!(nav.selected.is_none());
    assert_eq!(nav.version, old_version + 1);

    let current_scene = app
        .world_mut()
        .spawn(GraphSceneVersion(old_version + 1))
        .id();
    app.world_mut().spawn((
        GraphNodeMarker("same".into()),
        Interaction::Pressed,
        ChildOf(current_scene),
    ));
    app.update();
    assert_eq!(
        app.world().resource::<GraphNav>().selected.as_deref(),
        Some("same")
    );
    assert_eq!(app.world().resource::<GraphNav>().path, ["same"]);
}
