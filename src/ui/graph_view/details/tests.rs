use super::*;
use bevy::input::InputPlugin;
use bevy::input_focus::{InputDispatchPlugin, InputFocus};
use bevy::scene::ScenePlugin;
use serde_json::json;

/// 真实 BSN 和 Feathers 输入分发，不创建剪贴板后端、原生窗口或用户配置。
fn details_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        ScenePlugin,
        InputPlugin,
        InputDispatchPlugin,
        bevy::ui_widgets::ButtonPlugin,
    ))
    .init_asset::<Font>()
    .init_resource::<InputFocus>();
    register(&mut app);
    app.world_mut()
        .spawn((Window::default(), bevy::window::PrimaryWindow));
    app
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn descendant_of(world: &World, mut entity: Entity, ancestor: Entity) -> bool {
    loop {
        if entity == ancestor {
            return true;
        }
        match world.get::<ChildOf>(entity) {
            Some(parent) => entity = parent.parent(),
            None => return false,
        }
    }
}

fn find_in<T: Component>(app: &mut App, ancestor: Entity) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<T>>()
        .iter(app.world())
        .find(|entity| descendant_of(app.world(), *entity, ancestor))
        .unwrap()
}

fn section_value(app: &mut App, section: Entity) -> Entity {
    app.world_mut()
        .query::<(Entity, &SectionText)>()
        .iter(app.world())
        .find(|(entity, role)| {
            matches!(role, SectionText::Value) && descendant_of(app.world(), *entity, section)
        })
        .unwrap()
        .0
}

fn long_unicode() -> String {
    "机器人 🤖 café e\u{301} 完整参数 / 路径 αβγ\n第二行 \"带引号\" 和反斜线 \\。\n".repeat(100)
}

fn node_with_inputs(inputs: Value) -> TaskNode {
    TaskNode {
        id: "步骤_1".into(),
        node_type: "log".into(),
        inputs,
        checkpoint: false,
        children: None,
    }
}

fn click_caption(app: &mut App, button: Entity) {
    use bevy::picking::pointer::{Location, PointerButton, PointerId};
    let entity = app.world().get::<Children>(button).unwrap()[0];
    let location = Location {
        target: bevy::camera::NormalizedRenderTarget::None {
            width: 1,
            height: 1,
        },
        position: Vec2::ZERO,
    };
    let hit = bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None);
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
    app.world_mut().flush();
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        location.clone(),
        Click {
            button: PointerButton::Primary,
            hit: hit.clone(),
            count: 1,
            duration: std::time::Duration::ZERO,
        },
        entity,
    ));
    app.world_mut().flush();
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        location,
        Release {
            button: PointerButton::Primary,
            hit,
        },
        entity,
    ));
    app.world_mut().flush();
}

#[test]
fn 真实详情鼠标展开收起保留全文实体和滚动容器() {
    let original = long_unicode();
    let node = node_with_inputs(json!({"message": original}));
    let raw = json!({"id": node.id, "type": node.node_type, "inputs": node.inputs});
    let mut app = details_app();
    let panel = app
        .world_mut()
        .spawn_scene(detail_panel(&node, Some(&raw)))
        .unwrap()
        .id();
    settle(&mut app);
    let section = app
        .world_mut()
        .query::<(Entity, &TextSection)>()
        .iter(app.world())
        .find(|(_, section)| section.full.as_ref() == original)
        .unwrap()
        .0;
    let value = section_value(&mut app, section);
    let toggle = find_in::<ToggleText>(&mut app, section);
    let before: std::collections::HashSet<_> = app
        .world_mut()
        .query::<Entity>()
        .iter(app.world())
        .collect();
    let scroll = Vec2::new(0.0, 240.0);
    app.world_mut()
        .entity_mut(panel)
        .insert(ScrollPosition(scroll));
    assert_eq!(
        app.world().get::<Text>(value).unwrap().0,
        summarize(&original)
    );
    assert!(
        app.world()
            .get::<bevy::ui_widgets::Button>(toggle)
            .is_some()
    );

    click_caption(&mut app, toggle);
    app.update();
    assert_eq!(app.world().get::<Text>(value).unwrap().0, original);
    assert_eq!(app.world().get::<ScrollPosition>(panel).unwrap().0, scroll);
    assert_eq!(find_in::<ToggleText>(&mut app, section), toggle);
    click_caption(&mut app, toggle);
    app.update();
    assert_eq!(
        app.world().get::<Text>(value).unwrap().0,
        summarize(&original)
    );
    assert_eq!(app.world().get::<ScrollPosition>(panel).unwrap().0, scroll);
    assert_eq!(section_value(&mut app, section), value);
    assert_eq!(
        app.world_mut()
            .query::<Entity>()
            .iter(app.world())
            .collect::<std::collections::HashSet<_>>(),
        before,
        "全文切换不重建任何实体"
    );
}

#[test]
fn 回车和空格通过真实键盘展开收起并保留焦点() {
    use bevy::input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    };
    use bevy::input_focus::FocusCause;

    let original = long_unicode();
    let mut app = details_app();
    let section = app
        .world_mut()
        .spawn_scene(text_section("message", &original))
        .unwrap()
        .id();
    settle(&mut app);
    let toggle = find_in::<ToggleText>(&mut app, section);
    let value = section_value(&mut app, section);
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(toggle, FocusCause::Navigated);
    for (key_code, logical_key, expanded) in [
        (KeyCode::Enter, Key::Enter, true),
        (KeyCode::Space, Key::Space, false),
    ] {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
        app.update();
        assert_eq!(
            app.world().get::<TextSection>(section).unwrap().expanded,
            expanded
        );
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(toggle));
        assert_eq!(
            app.world().get::<Text>(value).unwrap().0,
            match expanded {
                true => original.clone(),
                false => summarize(&original),
            }
        );
    }
}

#[test]
fn 长unicode复制完整原文且成功失败均不改变详情内容() {
    let original = long_unicode();
    let mut app = details_app();
    let section = app
        .world_mut()
        .spawn_scene(text_section("message", &original))
        .unwrap()
        .id();
    settle(&mut app);
    let button = find_in::<CopyText>(&mut app, section);
    let caption = find_in::<CopyLabel>(&mut app, button);
    let value = section_value(&mut app, section);
    let mut copied = String::new();
    app.world_mut()
        .get_mut::<CopyText>(button)
        .unwrap()
        .copy_with(|text| {
            copied = text.to_string();
            Ok(())
        });
    app.update();
    assert_eq!(copied, original);
    assert_eq!(app.world().get::<Text>(caption).unwrap().0, "已复制");
    assert_eq!(
        app.world().get::<Text>(value).unwrap().0,
        summarize(&original)
    );
    app.world_mut()
        .get_mut::<CopyText>(button)
        .unwrap()
        .copy_with(|text| {
            assert_eq!(text, original);
            Err(ClipboardError::ClipboardOccupied)
        });
    app.update();
    assert_eq!(
        app.world().get::<Text>(caption).unwrap().0,
        "复制失败 · 重试"
    );
    assert_eq!(
        app.world().get::<Text>(value).unwrap().0,
        summarize(&original)
    );
    assert!(!app.world().contains_resource::<Clipboard>());
    assert!(!app.world().get::<TextSection>(section).unwrap().expanded);
    assert_eq!(
        app.world().get::<CopyText>(button).unwrap().full.as_ref(),
        original
    );
}

#[test]
fn 复制按钮鼠标激活在无剪贴板后端时显示失败而不崩溃() {
    let mut app = details_app();
    let section = app
        .world_mut()
        .spawn_scene(text_section("短参数", "原始内容"))
        .unwrap()
        .id();
    settle(&mut app);
    let button = find_in::<CopyText>(&mut app, section);
    let caption = find_in::<CopyLabel>(&mut app, button);
    click_caption(&mut app, button);
    app.update();
    assert_eq!(
        app.world().get::<Text>(caption).unwrap().0,
        "复制失败 · 重试"
    );
    let value = section_value(&mut app, section);
    assert_eq!(app.world().get::<Text>(value).unwrap().0, "原始内容");
}

#[test]
fn 迟到的全文和按钮文字同步既有展开及复制状态() {
    let original = long_unicode();
    let mut app = details_app();
    let full: Arc<str> = original.clone().into();
    let section = app
        .world_mut()
        .spawn(TextSection {
            full: Arc::clone(&full),
            summary: summarize(&original),
            expanded: true,
        })
        .id();
    let button = app
        .world_mut()
        .spawn((
            CopyText {
                full,
                feedback: CopyFeedback::Copied,
            },
            ChildOf(section),
        ))
        .id();
    app.update();
    let value = app
        .world_mut()
        .spawn((Text::new("旧摘要"), SectionText::Value, ChildOf(section)))
        .id();
    let toggle = app
        .world_mut()
        .spawn((
            Text::new("展开全文"),
            SectionText::ToggleLabel,
            ChildOf(section),
        ))
        .id();
    let caption = app
        .world_mut()
        .spawn((Text::new("复制全文"), CopyLabel, ChildOf(button)))
        .id();
    app.update();
    assert_eq!(app.world().get::<Text>(value).unwrap().0, original);
    assert_eq!(app.world().get::<Text>(toggle).unwrap().0, "收起全文");
    assert_eq!(app.world().get::<Text>(caption).unwrap().0, "已复制");
}

#[test]
fn 原始节点默认折叠且展开复制包含未知扩展字段() {
    let raw = json!({
        "id": "步骤_1", "type": "log", "inputs": {"message": "日志"},
        "未知扩展": {"数组": [1, true, null], "长内容": long_unicode()},
        "timeout": 120.5
    });
    let node = node_with_inputs(raw["inputs"].clone());
    let original = formatted_json(&raw);
    let mut app = details_app();
    app.world_mut()
        .spawn_scene(detail_panel(&node, Some(&raw)))
        .unwrap();
    settle(&mut app);
    let section = app
        .world_mut()
        .query::<(Entity, &TextSection)>()
        .iter(app.world())
        .find(|(_, state)| state.full.as_ref() == original)
        .unwrap()
        .0;
    let value = section_value(&mut app, section);
    assert!(!app.world().get::<TextSection>(section).unwrap().expanded);
    assert!(
        !app.world()
            .get::<Text>(value)
            .unwrap()
            .0
            .contains("未知扩展")
    );
    let button = find_in::<CopyText>(&mut app, section);
    app.world_mut()
        .get_mut::<CopyText>(button)
        .unwrap()
        .copy_with(|text| {
            assert_eq!(serde_json::from_str::<Value>(text).unwrap(), raw);
            assert_eq!(text, original);
            Ok(())
        });
    let toggle = find_in::<ToggleText>(&mut app, section);
    app.world_mut().trigger(Activate { entity: toggle });
    app.update();
    assert_eq!(app.world().get::<Text>(value).unwrap().0, original);
}

#[test]
fn 非对象输入与缺失空对象明确区分且保留实际内容() {
    for (input, title, expected) in [
        (json!(null), "inputs · null", "null".into()),
        (json!(true), "inputs · 布尔", "true".into()),
        (json!(1.5), "inputs · 数值", "1.5".into()),
        (
            json!([1, "数组"]),
            "inputs · 数组",
            formatted_json(&json!([1, "数组"])),
        ),
        (
            json!("原始字符串\n含换行"),
            "inputs · 字符串",
            "原始字符串\n含换行".into(),
        ),
        (json!({}), "inputs · 空对象", "{}".into()),
    ] {
        let raw = json!({"id": "步骤_1", "type": "log", "inputs": input});
        // 刻意给模型不同的值，验证详情优先读取原始节点。
        let node = node_with_inputs(json!(null));
        let mut app = details_app();
        app.world_mut()
            .spawn_scene(detail_panel(&node, Some(&raw)))
            .unwrap();
        settle(&mut app);
        let texts: Vec<_> = app
            .world_mut()
            .query::<&Text>()
            .iter(app.world())
            .map(|text| text.0.as_str())
            .collect();
        assert!(texts.contains(&title));
        assert!(texts.contains(&expected.as_str()));
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("没有输入参数") || text.contains("未定义 inputs"))
        );
    }
    let mut app = details_app();
    let raw = json!({"id": "步骤_1", "type": "log"});
    app.world_mut()
        .spawn_scene(detail_panel(&node_with_inputs(Value::Null), Some(&raw)))
        .unwrap();
    settle(&mut app);
    assert!(
        app.world_mut()
            .query::<&Text>()
            .iter(app.world())
            .any(|text| text.0 == "该节点未定义 inputs")
    );
}

#[test]
fn 空字符串可复制且json值格式化保持语义和字符串原文() {
    let mut copy = CopyText {
        full: Arc::from(""),
        feedback: CopyFeedback::Ready,
    };
    let mut called = false;
    copy.copy_with(|text| {
        called = true;
        assert!(text.is_empty());
        Ok(())
    });
    assert!(called);
    assert_eq!(copy.feedback, CopyFeedback::Copied);
    let text = "  \"字符串\"\\\n🤖  ";
    assert_eq!(input_text(&json!(text)), text);
    for value in [
        json!([1, 2.5, true, null]),
        json!({"未知": "扩展", "数值": 3}),
    ] {
        assert_eq!(
            serde_json::from_str::<Value>(&input_text(&value)).unwrap(),
            value
        );
    }
    assert_eq!(summarize(&"🤖".repeat(300)), "🤖".repeat(300));
    assert_eq!(
        summarize(&"🤖".repeat(301)),
        format!("{}…", "🤖".repeat(299))
    );
}
