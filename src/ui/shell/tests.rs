use super::*;
use crate::model::LoginConfig;
use crate::worker::BusyState;
use bevy::scene::ScenePlugin;

/// 使用实际 BSN 和同步排程，不创建 Clipboard 或读取用户配置。
fn status_app(message: &str) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
        .init_asset::<Font>()
        .insert_resource(Session::new(LoginConfig::default(), Vec::new()))
        .init_resource::<StatusLine>()
        .add_systems(
            Update,
            (sync_status_bar, sync_status_copy, sync_copy_button_style).chain(),
        );
    app.world_mut().resource_mut::<StatusLine>().set(message);
    // 先消耗资源的初始变化，再异步排入场景，确保新增控件仍能显示既有提示。
    app.update();
    app.world_mut()
        .commands()
        .spawn_empty()
        .queue_spawn_related_scenes::<Children>(bsn_list![status_bar()]);
    app.world_mut().flush();
    for _ in 0..4 {
        app.update();
    }
    app
}

fn single_entity<T: Component>(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<T>>()
        .single(app.world())
        .unwrap()
}

fn assert_copy_state(app: &mut App, message: &str, label: &str, disabled: bool) {
    let text = single_entity::<StatusText>(app);
    let button = single_entity::<StatusCopyButton>(app);
    let caption = single_entity::<StatusCopyLabel>(app);
    assert_eq!(app.world().get::<Text>(text).unwrap().0, message);
    assert_eq!(
        app.world().get::<StatusCopyButton>(button).unwrap().text,
        message
    );
    assert_eq!(app.world().get::<Text>(caption).unwrap().0, label);
    assert_eq!(
        app.world().get::<InteractionDisabled>(button).is_some(),
        disabled
    );
}

#[test]
fn 复制完整多行提示并保留原文且新提示重置反馈() {
    let original =
        "远程错误 🤖：任务图读取失败，字段 café 与位姿不匹配。\n详细原因：目标文件不存在。\n"
            .repeat(128);
    let mut app = status_app(&original);
    assert!(!app.world().contains_resource::<Clipboard>());
    assert_copy_state(&mut app, &original, "复制提示", false);

    let button = single_entity::<StatusCopyButton>(&mut app);
    let mut written = String::new();
    app.world_mut()
        .get_mut::<StatusCopyButton>(button)
        .unwrap()
        .copy_with(|text| {
            written.push_str(text);
            Ok(())
        });
    app.update();
    assert_eq!(written, original);
    assert_eq!(app.world().resource::<StatusLine>().text, original);
    assert_copy_state(&mut app, &original, "已复制", false);

    let mut attempts = 0;
    app.world_mut()
        .get_mut::<StatusCopyButton>(button)
        .unwrap()
        .copy_with(|text| {
            attempts += 1;
            assert_eq!(text, original);
            Err(ClipboardError::ClipboardOccupied)
        });
    app.update();
    assert_eq!(attempts, 1);
    assert_eq!(app.world().resource::<StatusLine>().text, original);
    assert_copy_state(&mut app, &original, "复制失败", false);

    app.world_mut()
        .resource_mut::<StatusLine>()
        .set("新的提示：保存成功");
    app.update();
    assert_copy_state(&mut app, "新的提示：保存成功", "复制提示", false);
    app.world_mut().resource_mut::<StatusLine>().set("");
    app.update();
    assert_copy_state(&mut app, "", "复制提示", true);
    app.world_mut()
        .get_mut::<StatusCopyButton>(button)
        .unwrap()
        .copy_with(|_| panic!("空提示不应调用剪贴板写入"));
}

#[test]
fn 复制源始终跟随重连优先于忙碌及普通提示的显示顺序() {
    let mut app = status_app("普通提示");
    assert_copy_state(&mut app, "普通提示", "复制提示", false);

    app.world_mut().resource_mut::<Session>().busy = BusyState::Loading("中文任务图.json".into());
    app.update();
    assert_copy_state(&mut app, "正在加载 中文任务图.json...", "复制提示", false);

    app.world_mut().resource_mut::<Session>().reconnect_status = Some("正在重连：第 2 次".into());
    app.update();
    assert_copy_state(&mut app, "正在重连：第 2 次", "复制提示", false);
    app.world_mut()
        .resource_mut::<StatusLine>()
        .set("后台的新提示");
    app.update();
    assert_copy_state(&mut app, "正在重连：第 2 次", "复制提示", false);

    app.world_mut().resource_mut::<Session>().reconnect_status = None;
    app.update();
    assert_copy_state(&mut app, "正在加载 中文任务图.json...", "复制提示", false);
    app.world_mut().resource_mut::<Session>().busy = BusyState::Idle;
    app.update();
    assert_copy_state(&mut app, "后台的新提示", "复制提示", false);
    assert!(!app.world().contains_resource::<Clipboard>());
}

#[test]
fn 复制按钮保持专用底色并正确响应悬停按下与禁用() {
    let mut app = status_app("可以复制的提示");
    let button = single_entity::<StatusCopyButton>(&mut app);
    let assert_colors = |app: &App, bg, text| {
        assert_eq!(
            app.world().get::<ThemeBackgroundColor>(button).unwrap().0,
            bg
        );
        assert_eq!(
            app.world()
                .get::<InheritableThemeTextColor>(button)
                .unwrap()
                .0,
            text
        );
        assert!(app.world().get::<ButtonVariant>(button).is_none());
    };
    assert_colors(&app, theme::COPY_BG, theme::COPY_TEXT);
    app.world_mut().entity_mut(button).insert(Hovered(true));
    app.update();
    assert_colors(&app, theme::COPY_BG_HOVER, theme::COPY_TEXT);
    app.world_mut().entity_mut(button).insert(Hovered(false));
    app.world_mut().entity_mut(button).insert(Pressed);
    app.update();
    assert_colors(&app, theme::COPY_BG_HOVER, theme::COPY_TEXT);
    app.world_mut().entity_mut(button).remove::<Pressed>();
    app.update();
    assert_colors(&app, theme::COPY_BG, theme::COPY_TEXT);
    app.world_mut().resource_mut::<StatusLine>().set("");
    app.update();
    assert_colors(
        &app,
        bevy::feathers::tokens::BUTTON_BG_DISABLED,
        bevy::feathers::tokens::BUTTON_TEXT_DISABLED,
    );
    app.world_mut().resource_mut::<StatusLine>().set("恢复提示");
    app.update();
    assert_colors(&app, theme::COPY_BG, theme::COPY_TEXT);
}
