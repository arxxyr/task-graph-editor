use super::*;
use bevy::input::InputPlugin;
use bevy::input_focus::{InputDispatchPlugin, InputFocus};
use bevy::scene::ScenePlugin;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        ScenePlugin,
        InputPlugin,
        InputDispatchPlugin,
        bevy::ui_widgets::ButtonPlugin,
        bevy::ui_widgets::MenuPlugin,
    ))
    .init_asset::<Font>()
    .init_asset::<Image>()
    .init_resource::<InputFocus>()
    .init_resource::<super::super::Editor>()
    .insert_resource(Session::new(
        crate::model::LoginConfig::default(),
        Vec::new(),
    ))
    .insert_resource(ViewMode::Logs)
    .add_message::<AppAction>()
    .configure_sets(
        Update,
        (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
    )
    .add_plugins((widgets::WidgetsPlugin, LogAnalysisPlugin));
    app
}

fn local(name: &str) -> Choice {
    Choice {
        label: name.into(),
        path: PathBuf::from(name),
        directory: false,
    }
}

#[test]
fn 新页面跨帧生成且筛选保留已有选择() {
    let mut app = app();
    app.world_mut().resource_mut::<Logs>().replace(
        vec![local("第一份.log"), local("第二份.log")],
        None,
        false,
    );
    app.world_mut().spawn_scene(pane()).unwrap();
    for _ in 0..5 {
        app.update();
    }
    assert!(
        app.world_mut()
            .query::<&LogPane>()
            .iter(app.world())
            .next()
            .is_some()
    );
    let version = app.world().resource::<Logs>().version;
    app.world_mut()
        .trigger(LogAction::Toggle { version, index: 1 });
    app.update();
    {
        let mut logs = app.world_mut().resource_mut::<Logs>();
        logs.filter = "第一".into();
        logs.refresh_filter();
        logs.list_revision += 1;
    }
    app.world_mut().trigger(LogAction::SelectAll);
    for _ in 0..4 {
        app.update();
    }
    let logs = app.world().resource::<Logs>();
    assert_eq!(logs.visible_indices(), &[0]);
    assert_eq!(logs.selected, BTreeSet::from([0, 1]));
    assert_eq!(
        app.world_mut()
            .query::<&ChoiceMarker>()
            .iter(app.world())
            .filter(|m| m.version == version)
            .count(),
        1
    );
}

#[test]
fn 切换列表后旧按钮不能选择新文件() {
    let mut app = app();
    let version = {
        let mut logs = app.world_mut().resource_mut::<Logs>();
        logs.replace(vec![local("old.log")], None, false);
        let version = logs.version;
        logs.replace(vec![local("new.log")], None, false);
        version
    };
    app.world_mut()
        .trigger(LogAction::Toggle { version, index: 0 });
    assert!(app.world().resource::<Logs>().selected.is_empty());
}

#[test]
fn 换连接后丢弃远程列表回执() {
    let mut app = app();
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.world_mut().resource_mut::<Logs>().remote = Some(RemotePending {
        receiver: Mutex::new(receiver),
        generation: 1,
        cancel: cancel.clone(),
        output: None,
    });
    sender
        .send(Ok(LogReply::Listed(LogListing {
            directory: "/旧主机".into(),
            entries: vec![],
        })))
        .unwrap();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.connection_generation = 2;
        session.is_connected = true;
    }
    app.update();
    let logs = app.world().resource::<Logs>();
    assert!(logs.remote.is_none());
    assert!(logs.source.is_none());
    assert!(cancel.load(Ordering::Acquire));
    assert_ne!(logs.directory, "/旧主机");
}

#[test]
fn 统计勾选不重建页面且忙碌期间保持请求口径() {
    let mut app = app();
    app.world_mut().spawn_scene(pane()).unwrap();
    for _ in 0..5 {
        app.update();
    }
    let entity = app
        .world_mut()
        .query::<(Entity, &StatisticsFlag)>()
        .iter(app.world())
        .find(|(_, f)| matches!(f, StatisticsFlag::Paused))
        .unwrap()
        .0;
    assert!(app.world().get::<bevy::ui::Checked>(entity).is_none());
    app.world_mut().trigger(ValueChange {
        source: entity,
        value: true,
        is_final: true,
    });
    app.update();
    assert!(app.world().resource::<Logs>().statistics.include_paused);
    assert!(app.world().get::<bevy::ui::Checked>(entity).is_some());
    let (sender, receiver) = mpsc::channel();
    app.world_mut().resource_mut::<Logs>().dialog = Some(Mutex::new(receiver));
    app.update();
    assert!(
        app.world()
            .get::<bevy::ui::InteractionDisabled>(entity)
            .is_some()
    );
    app.world_mut().trigger(ValueChange {
        source: entity,
        value: false,
        is_final: true,
    });
    app.update();
    assert!(app.world().resource::<Logs>().statistics.include_paused);
    sender.send(DialogReply::Cancelled).unwrap();
    app.update();
    app.update();
    assert!(
        app.world()
            .get::<bevy::ui::InteractionDisabled>(entity)
            .is_none()
    );
    app.world_mut().trigger(ValueChange {
        source: entity,
        value: false,
        is_final: true,
    });
    app.update();
    assert!(!app.world().resource::<Logs>().statistics.include_paused);
}
#[test]
fn 日志标签独立隐藏编辑区而切回保留实体() {
    let mut app = app();
    let actions = app
        .world_mut()
        .spawn((Node::default(), super::super::shell::ActionBarSlot))
        .id();
    let files = app
        .world_mut()
        .spawn((Node::default(), super::super::shell::FileListSlot))
        .id();
    app.update();
    for entity in [actions, files] {
        assert_eq!(
            app.world().get::<Node>(entity).unwrap().display,
            Display::None
        );
    }
    app.world_mut().insert_resource(ViewMode::Params);
    app.update();
    for entity in [actions, files] {
        assert_eq!(
            app.world().get::<Node>(entity).unwrap().display,
            Display::Flex
        );
    }
}
#[test]
fn 口径切换保留文件选择并拒绝非法日期启动() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<Logs>()
        .replace(vec![local("one.log")], None, true);
    app.world_mut().trigger(LogAction::Mode(true));
    app.world_mut().resource_mut::<Logs>().date = "2026-02-30".into();
    app.world_mut().trigger(LogAction::Analyze);
    let logs = app.world().resource::<Logs>();
    assert!(logs.workpiece);
    assert_eq!(logs.selected.len(), 1);
    assert!(logs.task.is_none());
    assert!(logs.status.contains("YYYY-MM-DD"));
}

#[test]
fn 精简操作保留菜单动作且分页随数据显隐() {
    let mut app = app();
    app.world_mut().spawn_scene(pane()).unwrap();
    for _ in 0..5 {
        app.update();
    }
    let find = |app: &mut App, predicate: fn(&LogAction) -> bool| {
        app.world_mut()
            .query::<(Entity, &LogAction)>()
            .iter(app.world())
            .find(|(_, action)| predicate(action))
            .unwrap()
            .0
    };
    let import = find(&mut app, |a| matches!(a, LogAction::LocalDirectory));
    let popup = app.world().get::<ChildOf>(import).unwrap().parent();
    let root = app.world().get::<ChildOf>(popup).unwrap().parent();
    let menu_button = app
        .world()
        .get::<Children>(root)
        .unwrap()
        .iter()
        .find(|e| {
            app.world()
                .get::<bevy::feathers::controls::FeathersMenuButton>(*e)
                .is_some()
        })
        .unwrap();
    app.world_mut().trigger(Activate {
        entity: menu_button,
    });
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        app.world().get::<Visibility>(popup),
        Some(&Visibility::Visible)
    );
    assert!(
        app.world()
            .get::<bevy::ui_widgets::MenuItem>(import)
            .is_some()
    );
    let mut cursor = app.world().resource::<Messages<AppAction>>().get_cursor();
    app.world_mut().trigger(Activate { entity: import });
    assert!(
        cursor
            .read(app.world().resource::<Messages<AppAction>>())
            .any(|a| matches!(a, AppAction::Logs(LogAction::LocalDirectory)))
    );
    let next = find(&mut app, |a| matches!(a, LogAction::Next));
    let directory = find(&mut app, |a| matches!(a, LogAction::ReportDirectory));
    assert!(
        app.world()
            .get::<bevy::ui::InteractionDisabled>(directory)
            .is_some()
    );
    let mut ancestor = directory;
    while let Some(parent) = app.world().get::<ChildOf>(ancestor) {
        ancestor = parent.parent();
        assert!(
            app.world().get::<ScrollArea>(ancestor).is_none(),
            "报告入口不能随内容滚动"
        );
    }
    let cancel = find(&mut app, |a| matches!(a, LogAction::Cancel));
    assert_eq!(
        app.world().get::<Node>(next).unwrap().display,
        Display::None
    );
    assert_eq!(
        app.world().get::<Node>(cancel).unwrap().display,
        Display::None
    );
    app.world_mut().resource_mut::<Logs>().replace(
        (0..=PAGE_SIZE)
            .map(|i| local(&format!("{i}.log")))
            .collect(),
        None,
        false,
    );
    app.update();
    assert_eq!(
        app.world().get::<Node>(next).unwrap().display,
        Display::Flex
    );
    app.world_mut().trigger(LogAction::Next);
    app.update();
    assert!(
        app.world()
            .get::<bevy::ui::InteractionDisabled>(next)
            .is_some()
    );
}

#[test]
fn 逐轮报告分页不依赖文件数量且保留设置手动展开() {
    let mut app = app();
    let rounds = (0..35)
        .map(|i| crate::log_analysis::RoundOverview {
            id: i.to_string(),
            label: "常规循环".into(),
            start_us: i * 1_000_000,
            end_us: Some((i + 1) * 1_000_000),
            raw_seconds: Some(1.0),
            deducted_seconds: 0.0,
            duration_seconds: Some(1.0),
            included: i > 0,
            notes: String::new(),
            detail: None,
        })
        .collect();
    app.world_mut().resource_mut::<Logs>().report = Some(AnalysisReport {
        statistics: StatisticsOptions::default(),
        output: "/tmp/report-ui-test".into(),
        reports: vec![crate::log_analysis::FileReport {
            input: "input.log".into(),
            summary: String::new(),
            rounds,
            duration_label: "有效耗时".into(),
            files: vec![],
            error: None,
        }],
        merged: None,
        warnings: vec![],
    });
    app.world_mut().spawn_scene(pane()).unwrap();
    for _ in 0..8 {
        app.update();
    }
    let setup = app
        .world_mut()
        .query_filtered::<Entity, With<view::ReportSetup>>()
        .single(app.world())
        .unwrap();
    assert!(!app.world().get::<widgets::Collapsible>(setup).unwrap().open);
    app.world_mut()
        .get_mut::<widgets::Collapsible>(setup)
        .unwrap()
        .open = true;
    app.world_mut().trigger(LogAction::ReportPage(true));
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(app.world().resource::<Logs>().report_page, 1);
    assert!(app.world().get::<widgets::Collapsible>(setup).unwrap().open);
    app.world_mut().trigger(LogAction::ReportPage(true));
    assert_eq!(app.world().resource::<Logs>().report_page, 1);
}
