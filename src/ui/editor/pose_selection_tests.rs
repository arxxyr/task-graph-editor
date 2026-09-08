//! 位姿数组的真实 BSN 交互回归：只操作内存，不启动窗口、SSH 或用户配置存储。

use super::*;
use crate::ui::FileBrowser;
use crate::ui::binding::PoseTarget;
use crate::ui::worker_bridge::WorkerBridgePlugin;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::pointer::{Location, PointerButton, PointerId};
use bevy::scene::ScenePlugin;
use bevy::text::{FontCx, LayoutCx};
use bevy::ui::InteractionDisabled;
use bevy::winit::WinitSettings;

fn fixture() -> TaskGraphData {
    let mut first = RobotPose::default();
    first.chassis_pose.position.x = 0.92853;
    let mut second = first.clone();
    second.chassis_pose.position.x = 1.059707;
    let poses = vec![
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
    ];
    crate::model::parse_task_graph(
        &serde_json::json!({
            "map_id": "隔离地图",
            "task_id": "隔离任务",
            "config": {"context": {
                "pick_poses": poses,
                "home_pose": serde_json::to_string(&first).unwrap(),
                "station_profiles": {"station_1": {"pick_poses": poses}},
            }},
        })
        .to_string(),
    )
    .unwrap()
}

fn field_path(data: &TaskGraphData, keys: &[&str]) -> Vec<usize> {
    let mut fields = data.context_fields.as_slice();
    let mut path = Vec::new();
    for (depth, key) in keys.iter().enumerate() {
        let index = fields.iter().position(|field| field.key == *key).unwrap();
        path.push(index);
        if depth + 1 < keys.len() {
            let ContextValue::NestedGroup(children) = &fields[index].value else {
                panic!("路径中间项必须是嵌套分组：{key}");
            };
            fields = children;
        }
    }
    path
}

/// 注册正式插件与 Input → Flush → 动作 → Rebuild 排程，点击必须经过 AppAction 才能选中。
fn selection_app() -> App {
    let mut session = Session::new(crate::model::LoginConfig::default(), Vec::new());
    session.is_connected = true;
    let mut editor = Editor::default();
    editor.load(Some(fixture()));
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
        .init_asset::<Font>()
        .init_resource::<InputFocus>()
        .init_resource::<FontCx>()
        .init_resource::<LayoutCx>()
        .init_resource::<bevy::clipboard::Clipboard>()
        .insert_resource(WinitSettings::desktop_app())
        .insert_resource(session)
        .insert_resource(editor)
        .init_resource::<StatusLine>()
        .init_resource::<FileBrowser>()
        .configure_sets(
            Update,
            (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
        )
        .add_plugins((
            WorkerBridgePlugin,
            EditorPanelPlugin,
            widgets::WidgetsPlugin,
        ))
        .add_systems(PostUpdate, bevy::text::apply_text_edits);

    // 只创建 ECS 窗口组件满足 PointerTraversal，不装配会创建原生窗口的 WinitPlugin。
    app.world_mut()
        .spawn((Window::default(), bevy::window::PrimaryWindow));
    app.world_mut().spawn((
        EditorSlot,
        Node::default(),
        ScrollPosition(Vec2::new(0.0, 173.0)),
    ));
    app.world_mut().spawn((ActionBarSlot, Node::default()));

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
    app
}

fn settle_scenes(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

fn target(app: &App, keys: &[&str], index: usize) -> PoseTarget {
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    PoseTarget::array_element(field_path(data, keys), index)
}

fn title_entity(app: &mut App, target: &PoseTarget) -> Entity {
    app.world_mut()
        .query::<(Entity, &PoseCardTitle)>()
        .iter(app.world())
        .find(|(_, title)| title.target == *target)
        .unwrap()
        .0
}

fn number_entity(app: &mut App, target: &PoseTarget) -> Entity {
    let binding = ValueBinding::new(
        target.field_path.clone(),
        ValueSlot::PoseArray(
            target.array_index.unwrap(),
            PosePart::Chassis,
            PoseComp::PosX,
        ),
    );
    app.world_mut()
        .query_filtered::<(Entity, &ValueBinding), With<FeathersNumberInput>>()
        .iter(app.world())
        .find(|(_, candidate)| **candidate == binding)
        .unwrap()
        .0
}

fn editable_child(app: &App, field: Entity) -> Entity {
    app.world()
        .get::<Children>(field)
        .unwrap()
        .iter()
        .find(|child| app.world().get::<EditableText>(*child).is_some())
        .unwrap()
}

fn click(app: &mut App, entity: Entity, button: PointerButton) {
    let before = app.world().resource::<Messages<AppAction>>().len();
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: bevy::camera::NormalizedRenderTarget::None {
                width: 1,
                height: 1,
            },
            position: Vec2::ZERO,
        },
        Click {
            button,
            hit: bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            duration: std::time::Duration::ZERO,
            count: 1,
        },
        entity,
    ));
    app.world_mut().flush();
    let added = app.world().resource::<Messages<AppAction>>().len() - before;
    assert_eq!(
        added,
        usize::from(button == PointerButton::Primary),
        "子节点点击只能产生一次选中消息，非主键不能产生消息"
    );
}

fn assert_fetch_buttons(app: &mut App, enabled: bool) {
    let buttons: Vec<_> = app
        .world_mut()
        .query::<(&ActionButton, Has<InteractionDisabled>)>()
        .iter(app.world())
        .filter(|(button, _)| {
            matches!(
                button.0,
                AppAction::FetchChassisPose
                    | AppAction::FetchHeadJoints
                    | AppAction::FetchWaistJoints
            )
        })
        .map(|(_, disabled)| disabled)
        .collect();
    assert_eq!(buttons.len(), 3, "必须实际生成三个 ROS2 取数按钮");
    assert!(buttons.into_iter().all(|disabled| disabled != enabled));
}

fn assert_unique_selection(app: &mut App, selected: &PoseTarget) {
    assert_eq!(
        app.world().resource::<Editor>().selected_pose.as_ref(),
        Some(selected)
    );
    let cards: Vec<_> = app
        .world_mut()
        .query::<(&PoseCard, &ThemeBackgroundColor, &ThemeBorderColor)>()
        .iter(app.world())
        .map(|(card, bg, border)| (card.target.clone(), bg.0.clone(), border.0.clone()))
        .collect();
    assert_eq!(cards.len(), 5, "顶层与嵌套数组各两个元素，加一个独立位姿");
    assert_eq!(
        cards
            .iter()
            .filter(|(target, _, _)| target == selected)
            .count(),
        1
    );
    for (target, bg, border) in cards {
        let expected = pose_card_tokens(target == *selected);
        assert_eq!((bg, border), expected, "错误的卡片高亮：{target:?}");
    }
    for (title, color) in app
        .world_mut()
        .query::<(&PoseCardTitle, &ThemeTextColor)>()
        .iter(app.world())
    {
        assert_eq!(color.0, pose_title_token(title.target == *selected));
    }
}

#[test]
fn 顶层和嵌套位姿数组标题选中唯一元素并启用三个取数按钮() {
    let mut app = selection_app();
    settle_scenes(&mut app);
    assert_fetch_buttons(&mut app, false);
    for keys in [
        vec!["pick_poses"],
        vec!["station_profiles", "station_1", "pick_poses"],
    ] {
        for index in 0..2 {
            let target = target(&app, &keys, index);
            let title = title_entity(&mut app, &target);
            let previous = app.world().resource::<Editor>().selected_pose.clone();
            click(&mut app, title, PointerButton::Primary);
            assert_eq!(app.world().resource::<Editor>().selected_pose, previous);
            app.update();
            assert_unique_selection(&mut app, &target);
            assert_fetch_buttons(&mut app, true);
        }
    }

    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    let ordinary = PoseTarget::field(field_path(data, &["home_pose"]));
    let title = title_entity(&mut app, &ordinary);
    click(&mut app, title, PointerButton::Primary);
    app.update();
    assert_unique_selection(&mut app, &ordinary);
    assert_fetch_buttons(&mut app, true);

    let other = target(&app, &["pick_poses"], 0);
    let other_title = title_entity(&mut app, &other);
    click(&mut app, other_title, PointerButton::Secondary);
    app.update();
    assert_unique_selection(&mut app, &ordinary);
}

#[test]
fn 数值子节点点击切换数组元素且回填保留实体焦点和滚动位置() {
    let mut app = selection_app();
    settle_scenes(&mut app);
    let first = target(&app, &["pick_poses"], 0);
    let second = target(&app, &["pick_poses"], 1);
    let first_field = number_entity(&mut app, &first);
    let first_text = editable_child(&app, first_field);
    let second_field = number_entity(&mut app, &second);
    let second_text = editable_child(&app, second_field);
    let entities_before: HashSet<_> = app
        .world_mut()
        .query_filtered::<Entity, With<ValueBinding>>()
        .iter(app.world())
        .collect();
    let slot = app
        .world_mut()
        .query_filtered::<Entity, With<EditorSlot>>()
        .single(app.world())
        .unwrap();
    let scroll_before = app.world().get::<ScrollPosition>(slot).unwrap().0;
    let structure_before = app.world().resource::<Editor>().structure_version;

    for (target, text) in [(&first, first_text), (&second, second_text)] {
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(text, FocusCause::Pressed);
        click(&mut app, text, PointerButton::Primary);
        app.update();
        assert_unique_selection(&mut app, target);
        assert_fetch_buttons(&mut app, true);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(text));
    }

    // 实际取数时按钮获得焦点；Feathers 有意不覆盖仍在聚焦编辑的数字文本。
    let fetch_button = app
        .world_mut()
        .query::<(Entity, &ActionButton)>()
        .iter(app.world())
        .find(|(_, button)| matches!(button.0, AppAction::FetchChassisPose))
        .unwrap()
        .0;
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(fetch_button, FocusCause::Pressed);

    let refreshed = 0.9216510910864573;
    let response_target = second.clone();
    // 与 poll_worker 一样，先刷新本帧输入，再于 Update 阶段写入外部响应。
    app.add_systems(
        Update,
        (move |mut editor: ResMut<Editor>| {
            response_target
                .pose_mut(editor.data.as_mut().unwrap())
                .unwrap()
                .chassis_pose
                .position
                .x = refreshed;
            editor.mark_values_changed();
        })
        .in_set(UiSet::Update)
        .run_if(bevy::ecs::schedule::common_conditions::run_once),
    );
    app.update();
    let displayed = app
        .world()
        .get::<EditableText>(second_text)
        .unwrap()
        .value()
        .to_string()
        .parse::<f64>()
        .unwrap();
    assert_eq!(
        displayed.to_bits(),
        refreshed.to_bits(),
        "model={:?}, focus={:?}, values={:?}, invalid={:?}",
        second
            .pose(app.world().resource::<Editor>().data.as_ref().unwrap())
            .unwrap()
            .chassis_pose
            .position
            .x,
        app.world().resource::<InputFocus>().get(),
        app.world().resource::<RenderedEditor>().values,
        app.world().resource::<InputValidation>().invalid_fields,
    );
    let unchanged = app
        .world()
        .get::<EditableText>(first_text)
        .unwrap()
        .value()
        .to_string()
        .parse::<f64>()
        .unwrap();
    assert_eq!(unchanged, 0.92853);
    let entities_after: HashSet<_> = app
        .world_mut()
        .query_filtered::<Entity, With<ValueBinding>>()
        .iter(app.world())
        .collect();
    assert_eq!(entities_after, entities_before);
    assert_eq!(
        app.world().resource::<Editor>().structure_version,
        structure_before
    );
    assert_eq!(
        app.world().get::<ScrollPosition>(slot).unwrap().0,
        scroll_before
    );
    assert_eq!(
        app.world().resource::<InputFocus>().get(),
        Some(fetch_button)
    );
}

#[test]
fn 旧选中状态下排队的新数组卡片和标题按当前选中初始化颜色() {
    let mut app = selection_app();
    settle_scenes(&mut app);
    let target = target(&app, &["pick_poses"], 1);
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    let field = crate::model::field_at_path(&data.context_fields, &target.field_path).unwrap();
    // 模拟 BSN 在选择变化之前准备好、之后才落地的场景。
    let delayed = other_field(field, target.field_path.clone(), None);
    let title = title_entity(&mut app, &target);
    click(&mut app, title, PointerButton::Primary);
    settle_scenes(&mut app);
    let existing_cards: HashSet<_> = app
        .world_mut()
        .query_filtered::<Entity, With<PoseCard>>()
        .iter(app.world())
        .collect();
    let existing_titles: HashSet<_> = app
        .world_mut()
        .query_filtered::<Entity, With<PoseCardTitle>>()
        .iter(app.world())
        .collect();
    app.world_mut().spawn_scene(delayed).unwrap();
    settle_scenes(&mut app);

    let mut new_cards = 0;
    for (entity, card, bg, border) in app
        .world_mut()
        .query::<(Entity, &PoseCard, &ThemeBackgroundColor, &ThemeBorderColor)>()
        .iter(app.world())
    {
        if !existing_cards.contains(&entity) {
            new_cards += 1;
            let (expected_bg, expected_border) = pose_card_tokens(card.target == target);
            assert_eq!(bg.0, expected_bg);
            assert_eq!(border.0, expected_border);
        }
    }
    assert_eq!(new_cards, 2);
    let mut new_titles = 0;
    for (entity, title, color) in app
        .world_mut()
        .query::<(Entity, &PoseCardTitle, &ThemeTextColor)>()
        .iter(app.world())
    {
        if !existing_titles.contains(&entity) {
            new_titles += 1;
            assert_eq!(color.0, pose_title_token(title.target == target));
        }
    }
    assert_eq!(new_titles, 2);
    assert_eq!(app.world().resource::<Editor>().selected_pose, Some(target));
}
