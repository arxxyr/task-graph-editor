use super::*;
use bevy::input::ButtonState;
use bevy::input::InputPlugin;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input_focus::{FocusCause, InputDispatchPlugin, InputFocus};
use bevy::picking::backend::HitData;
use bevy::picking::pointer::{Location, PointerId};
use bevy::scene::ScenePlugin;
use bevy::ui_widgets::{ButtonPlugin, MenuFocusState, MenuItem, MenuPlugin};
use bevy::window::PrimaryWindow;

/// 使用实际 BSN、菜单和输入派发，不加载偏好文件或连接会话。
fn menu_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        ScenePlugin,
        InputPlugin,
        InputDispatchPlugin,
        ButtonPlugin,
        MenuPlugin,
    ))
    .init_asset::<Font>()
    .init_asset::<Image>()
    .init_resource::<InputFocus>()
    .insert_resource(ThemeSelection(ThemeId::DuskSand))
    .add_plugins(ThemePickerPlugin);
    app.world_mut().spawn((Window::default(), PrimaryWindow));
    // 消耗初始资源变化后再排队生成，覆盖晚生成的入口文字和选中标记。
    app.update();
    app.world_mut()
        .commands()
        .spawn_empty()
        .queue_spawn_related_scenes::<Children>(bsn_list![theme_picker()]);
    app.world_mut().flush();
    settle(&mut app);
    app
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn entity_with<T: Component>(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<T>>()
        .single(app.world())
        .unwrap()
}

fn items(app: &mut App) -> Vec<Entity> {
    let popup = entity_with::<ThemePickerPopup>(app);
    app.world()
        .get::<Children>(popup)
        .unwrap()
        .iter()
        .filter(|&entity| app.world().get::<MenuItem>(entity).is_some())
        .collect()
}

fn activate_button(app: &mut App) {
    let entity = entity_with::<FeathersMenuButton>(app);
    app.world_mut().trigger(Activate { entity });
    app.world_mut().flush();
    settle(app);
}

fn assert_open(app: &mut App, expected_focus: Entity) {
    let popup = entity_with::<ThemePickerPopup>(app);
    assert_eq!(
        app.world().get::<Visibility>(popup),
        Some(&Visibility::Visible)
    );
    assert_eq!(
        app.world().get::<MenuFocusState>(popup),
        Some(&MenuFocusState::Open)
    );
    assert_eq!(
        app.world().resource::<InputFocus>().get(),
        Some(expected_focus)
    );
}

fn assert_closed(app: &mut App, expected_focus: Entity) {
    let popup = entity_with::<ThemePickerPopup>(app);
    assert_eq!(
        app.world().get::<Visibility>(popup),
        Some(&Visibility::Hidden)
    );
    assert_eq!(
        app.world().resource::<InputFocus>().get(),
        Some(expected_focus)
    );
}

fn assert_captions(app: &mut App, selected: ThemeId) {
    assert_eq!(app.world().resource::<ThemeSelection>().0, selected);
    let mut query = app.world_mut().query::<(&Text, &ThemeCaption)>();
    let mut checked = 0;
    for (text, caption) in query.iter(app.world()) {
        match *caption {
            ThemeCaption::Current => {
                assert_eq!(text.0, format!("选择主题：{}", selected.label()));
            }
            ThemeCaption::Check(id) => {
                let expected = match id == selected {
                    true => "✓",
                    false => "",
                };
                assert_eq!(text.0, expected);
                checked += usize::from(!text.0.is_empty());
            }
        }
    }
    assert_eq!(checked, 1);
}

fn key(app: &mut App, key_code: KeyCode, logical_key: Key) {
    let window = entity_with::<PrimaryWindow>(app);
    app.world_mut().write_message(KeyboardInput {
        key_code,
        logical_key,
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window,
    });
    settle(app);
}

fn pointer<E: std::fmt::Debug + Clone + Reflect>(entity: Entity, event: E) -> Pointer<E> {
    Pointer::new(
        PointerId::Mouse,
        Location {
            target: bevy::camera::NormalizedRenderTarget::None {
                width: 1,
                height: 1,
            },
            position: Vec2::ZERO,
        },
        event,
        entity,
    )
}

fn click_label(app: &mut App, label: Entity) {
    let hit = HitData::new(Entity::PLACEHOLDER, 0.0, None, None);
    app.world_mut().trigger(pointer(
        label,
        Press {
            button: PointerButton::Primary,
            hit: hit.clone(),
            count: 1,
        },
    ));
    app.world_mut().flush();
    app.world_mut().trigger(pointer(
        label,
        Click {
            button: PointerButton::Primary,
            hit: hit.clone(),
            duration: std::time::Duration::ZERO,
            count: 1,
        },
    ));
    app.world_mut().flush();
    app.world_mut().trigger(pointer(
        label,
        Release {
            button: PointerButton::Primary,
            hit,
        },
    ));
    app.world_mut().flush();
    settle(app);
}

#[test]
fn 跨帧生成后首次打开可聚焦且重复开关保留四个实体() {
    let mut app = menu_app();
    assert_captions(&mut app, ThemeId::DuskSand);
    let original = items(&mut app);
    assert_eq!(original.len(), ThemeId::ALL.len());
    let label = app
        .world_mut()
        .query::<(Entity, &ThemeCaption)>()
        .iter(app.world())
        .find(|(_, caption)| matches!(caption, ThemeCaption::Current))
        .unwrap()
        .0;
    for _ in 0..3 {
        click_label(&mut app, label);
        assert_open(&mut app, original[0]);
        click_label(&mut app, label);
        let popup = entity_with::<ThemePickerPopup>(&mut app);
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
        assert_eq!(items(&mut app), original);
    }
}

#[test]
fn 点击子标签和键盘选择同步主题且不重建菜单() {
    let mut app = menu_app();
    let original = items(&mut app);
    let label = app
        .world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .find(|(_, text)| text.0 == ThemeId::WarmPaper.label())
        .unwrap()
        .0;
    activate_button(&mut app);
    click_label(&mut app, label);
    let button = entity_with::<FeathersMenuButton>(&mut app);
    assert_closed(&mut app, button);
    assert_captions(&mut app, ThemeId::WarmPaper);

    activate_button(&mut app);
    key(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
    key(&mut app, KeyCode::Enter, Key::Enter);
    assert_closed(&mut app, button);
    assert_captions(&mut app, ThemeId::DuskSand);

    key(&mut app, KeyCode::ArrowUp, Key::ArrowUp);
    let last = *original.last().unwrap();
    assert_open(&mut app, last);
    key(&mut app, KeyCode::Space, Key::Space);
    assert_closed(&mut app, button);
    assert_captions(&mut app, ThemeId::MistIndigo);
    let changed_at = app.world().resource_ref::<ThemeSelection>().last_changed();
    key(&mut app, KeyCode::ArrowUp, Key::ArrowUp);
    key(&mut app, KeyCode::Space, Key::Space);
    assert_closed(&mut app, button);
    assert_eq!(
        app.world().resource_ref::<ThemeSelection>().last_changed(),
        changed_at,
        "重复选择当前主题不应触发重应用或偏好写入"
    );
    assert_eq!(items(&mut app), original);
}

#[test]
fn 取消回到入口而外部失焦保留新焦点且不改变主题() {
    let mut app = menu_app();
    let button = entity_with::<FeathersMenuButton>(&mut app);
    activate_button(&mut app);
    key(&mut app, KeyCode::Escape, Key::Escape);
    assert_closed(&mut app, button);
    assert_captions(&mut app, ThemeId::DuskSand);

    activate_button(&mut app);
    let outside = entity_with::<PrimaryWindow>(&mut app);
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(outside, FocusCause::Navigated);
    settle(&mut app);
    assert_closed(&mut app, outside);
    assert_captions(&mut app, ThemeId::DuskSand);
}
