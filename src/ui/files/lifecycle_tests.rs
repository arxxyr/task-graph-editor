//! 文件菜单与文件行增量复用的无窗口生命周期回归。

use super::*;
use bevy::scene::ScenePlugin;

fn isolated_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
        .init_asset::<Font>()
        .insert_resource(Session::new(
            crate::model::LoginConfig::default(),
            Vec::new(),
        ))
        .init_resource::<super::super::Editor>()
        .init_resource::<FileBrowser>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<UiScale>()
        .add_message::<AppAction>()
        .configure_sets(
            Update,
            (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
        )
        .add_plugins((FileListPlugin, widgets::WidgetsPlugin));
    app
}

#[test]
fn 点击关闭菜单先销毁旧项再刷新且重开新项交互正常() {
    let mut app = isolated_app();
    let root = app
        .world_mut()
        .spawn((ContextMenuRoot, Node::default()))
        .id();
    let old_item = app
        .world_mut()
        .spawn((
            ChildOf(root),
            MenuAction(AppAction::BackupFile("旧任务.json".into())),
            Interaction::Pressed,
        ))
        .id();
    app.world_mut()
        .resource_mut::<ContextMenu>()
        .open_at(Vec2::ZERO, Some("旧任务.json".into()));
    app.world_mut().resource_mut::<RenderedList>().menu = Some(1);

    // 与真实点击相同：Input 发出动作并关闭菜单，Rebuild 删除刚被按下的实体。
    app.update();
    assert!(!app.world().resource::<ContextMenu>().open);
    assert!(app.world().get_entity(old_item).is_err());
    assert_eq!(app.world().resource::<Messages<AppAction>>().len(), 1);
    assert_eq!(
        app.world().get::<Node>(root).unwrap().display,
        Display::None
    );

    app.world_mut()
        .resource_mut::<ContextMenu>()
        .open_at(Vec2::ZERO, Some("新任务.json".into()));
    for _ in 0..3 {
        app.update();
    }
    let new_item = app
        .world_mut()
        .query::<(Entity, &MenuAction)>()
        .iter(app.world())
        .find(|(_, item)| matches!(&item.0, AppAction::DeleteFile(name) if name == "新任务.json"))
        .unwrap()
        .0;
    assert_ne!(new_item, old_item);
    *app.world_mut().get_mut::<Interaction>(new_item).unwrap() = Interaction::Hovered;
    app.update();
    assert_eq!(
        app.world().get::<ThemeBackgroundColor>(new_item).unwrap().0,
        tokens::MENUITEM_BG_HOVER
    );
}

#[test]
fn 列表与选中同帧变更只更新新行且单独选择保留行实体() {
    let mut app = isolated_app();
    let root = app.world_mut().spawn(FileRowsSlot).id();
    let old_row = app
        .world_mut()
        .spawn((ChildOf(root), FileRow("任务.json".into())))
        .id();
    {
        let mut browser = app.world_mut().resource_mut::<FileBrowser>();
        browser.set_files(vec!["任务.json".into(), "其他.json".into()]);
        browser.selected = Some("任务.json".into());
    }
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().get_entity(old_row).is_err());
    let rows: Vec<_> = app
        .world_mut()
        .query::<(Entity, &FileRow, Has<Selected>)>()
        .iter(app.world())
        .map(|(entity, row, selected)| (entity, row.0.clone(), selected))
        .collect();
    assert_eq!(rows.len(), 2);
    for (_, name, selected) in &rows {
        assert_eq!(*selected, name == "任务.json");
    }

    app.world_mut().resource_mut::<FileBrowser>().selected = Some("其他.json".into());
    app.update();
    for (entity, name, _) in rows {
        assert!(app.world().get::<FileRow>(entity).is_some());
        assert_eq!(
            app.world().get::<Selected>(entity).is_some(),
            name == "其他.json"
        );
    }
}

fn set_listing(app: &mut App, directory: &str, files: &[&str], selected: Option<&str>) {
    let mut browser = app.world_mut().resource_mut::<FileBrowser>();
    browser.remote_dir = Some(directory.into());
    browser.set_files(files.iter().map(|name| (*name).into()).collect());
    browser.selected = selected.map(str::to_owned);
}

fn advance_scenes(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

fn ordered_rows(app: &App, slot: Entity) -> Vec<(Entity, String)> {
    app.world()
        .get::<Children>(slot)
        .into_iter()
        .flat_map(|children| children.iter())
        .filter_map(|entity| {
            app.world()
                .get::<FileRow>(entity)
                .map(|row| (entity, row.0.clone()))
        })
        .collect()
}

#[test]
fn 同来源刷新增删与排序保留文件行子控件和滚动位置() {
    let mut app = isolated_app();
    set_listing(
        &mut app,
        "/tasks",
        &["甲.json", "乙.json", "丙.json"],
        Some("乙.json"),
    );
    let outer = app
        .world_mut()
        .spawn((FileListSlot, ScrollPosition(Vec2::new(0.0, 37.0))))
        .id();
    advance_scenes(&mut app);
    let slot = app
        .world_mut()
        .query_filtered::<Entity, With<FileRowsSlot>>()
        .single(app.world())
        .unwrap();
    let before = ordered_rows(&app, slot);
    assert_eq!(before.len(), 3);
    let retained_children = app.world().get::<Children>(before[1].0).unwrap().to_vec();
    let blank = *app.world().get::<Children>(slot).unwrap().last().unwrap();

    set_listing(
        &mut app,
        "/tasks",
        &["丙.json", "乙.json", "丁.json"],
        Some("丁.json"),
    );
    advance_scenes(&mut app);
    let after = ordered_rows(&app, slot);
    assert_eq!(
        after
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["丙.json", "乙.json", "丁.json"]
    );
    assert_eq!(after[0].0, before[2].0);
    assert_eq!(after[1].0, before[1].0);
    assert!(app.world().get_entity(before[0].0).is_err());
    assert_eq!(
        app.world().get::<Children>(after[1].0).unwrap().to_vec(),
        retained_children
    );
    assert_eq!(
        app.world().get::<ScrollPosition>(outer).unwrap().0,
        Vec2::new(0.0, 37.0)
    );
    assert_eq!(
        *app.world().get::<Children>(slot).unwrap().last().unwrap(),
        blank
    );
    for (entity, name) in &after {
        assert!(app.world().get::<FeathersListRow>(*entity).is_some());
        assert_eq!(
            app.world().get::<Selected>(*entity).is_some(),
            name == "丁.json"
        );
    }

    // 刷新相同列表及修改连接表单都不能改变实际来源或行身份。
    app.world_mut().resource_mut::<Session>().login.remote_dir = "/表单中尚未浏览的目录".into();
    set_listing(
        &mut app,
        "/tasks",
        &["丙.json", "乙.json", "丁.json"],
        Some("乙.json"),
    );
    advance_scenes(&mut app);
    assert_eq!(ordered_rows(&app, slot), after);
    let title = app
        .world_mut()
        .query_filtered::<&Text, With<FileListTitle>>()
        .single(app.world())
        .unwrap();
    assert_eq!(title.0, "文件列表 (3)");
    let directory = app
        .world_mut()
        .query_filtered::<&Text, With<FileListDirectory>>()
        .single(app.world())
        .unwrap();
    assert_eq!(directory.0, "当前目录：/tasks");
}

#[test]
fn 同名文件换目录或连接代次必须更换行实体且空提示常驻() {
    let mut app = isolated_app();
    let slot = app.world_mut().spawn(FileRowsSlot).id();
    set_listing(&mut app, "/甲", &["任务.json"], None);
    advance_scenes(&mut app);
    let original = ordered_rows(&app, slot)[0].0;
    let empty = app
        .world_mut()
        .query_filtered::<Entity, With<FileListEmpty>>()
        .single(app.world())
        .unwrap();
    assert_eq!(
        app.world().get::<Node>(empty).unwrap().display,
        Display::None
    );

    // 来源本身也是同步依据，即使列表版本不变也不能复用旧来源。
    app.world_mut().resource_mut::<FileBrowser>().remote_dir = Some("/乙".into());
    advance_scenes(&mut app);
    let moved = ordered_rows(&app, slot)[0].0;
    assert_ne!(moved, original);
    assert!(app.world().get_entity(original).is_err());
    app.world_mut()
        .resource_mut::<Session>()
        .connection_generation += 1;
    advance_scenes(&mut app);
    let reconnected = ordered_rows(&app, slot)[0].0;
    assert_ne!(reconnected, moved);
    assert!(app.world().get_entity(moved).is_err());

    set_listing(&mut app, "/乙", &[], None);
    advance_scenes(&mut app);
    assert!(ordered_rows(&app, slot).is_empty());
    assert!(app.world().get_entity(reconnected).is_err());
    assert_eq!(
        app.world().get::<Node>(empty).unwrap().display,
        Display::Flex
    );
    assert_eq!(app.world().get::<Children>(slot).unwrap().len(), 2);
    set_listing(&mut app, "/乙", &["新任务.json"], None);
    advance_scenes(&mut app);
    assert_eq!(
        app.world().get::<Node>(empty).unwrap().display,
        Display::None
    );
    assert_eq!(ordered_rows(&app, slot).len(), 1);
}

#[test]
fn 新增行场景连续刷新和删除不会留下旧来源或重复子项() {
    let mut app = isolated_app();
    let slot = app.world_mut().spawn(FileRowsSlot).id();
    set_listing(&mut app, "/甲", &["保留.json", "删除.json"], None);
    app.update();
    let first = ordered_rows(&app, slot);
    assert_eq!(first.len(), 2);
    // 根实体已经登记，但 Feathers 场景尚未落地，后续更新必须仍识别并管理这些根。
    assert!(
        first
            .iter()
            .all(|(entity, _)| app.world().get::<FeathersListRow>(*entity).is_none())
    );
    set_listing(
        &mut app,
        "/甲",
        &["保留.json", "新建.json"],
        Some("保留.json"),
    );
    app.update();
    let second = ordered_rows(&app, slot);
    assert_eq!(second[0].0, first[0].0);
    assert!(app.world().get_entity(first[1].0).is_err());

    app.world_mut()
        .resource_mut::<Session>()
        .connection_generation += 1;
    set_listing(&mut app, "/乙", &["保留.json"], Some("保留.json"));
    app.update();
    let final_row = ordered_rows(&app, slot)[0].0;
    assert_ne!(final_row, first[0].0);
    for (entity, _) in second {
        assert!(app.world().get_entity(entity).is_err());
    }
    advance_scenes(&mut app);
    assert_eq!(ordered_rows(&app, slot), [(final_row, "保留.json".into())]);
    assert_eq!(app.world().get::<Children>(slot).unwrap().len(), 3);
    assert!(app.world().get::<FeathersListRow>(final_row).is_some());
    assert!(app.world().get::<Selected>(final_row).is_some());
    assert_eq!(
        app.world_mut()
            .query::<&FileRow>()
            .iter(app.world())
            .count(),
        1
    );
}
