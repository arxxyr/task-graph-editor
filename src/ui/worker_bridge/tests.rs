//! 桥接层使用纯状态响应验证来源绑定，无需真实窗口、登录配置或 SSH 服务端。

use super::*;
use crate::ui::binding::{ValueBinding, ValueSlot};
use crate::ui::{connect, editor as editor_panel, widgets};
use bevy::clipboard::{ClipboardError, ClipboardRead};
use bevy::input_focus::InputFocus;
use bevy::platform::sync::Mutex;
use bevy::scene::ScenePlugin;
use bevy::text::{EditableText, FontCx, LayoutCx, TextEdit};

fn session() -> Session {
    let mut session = Session::new(model::LoginConfig::default(), Vec::new());
    session.is_connected = true;
    session.connection_generation = 7;
    session
}

fn editor() -> Editor {
    let mut editor = Editor::default();
    editor.load_remote(
        model::parse_task_graph(
            r#"{"map_id":"m","task_id":"task","config":{"context":{"speed":1.5}}}"#,
        )
        .unwrap(),
        RemoteDocument {
            connection_generation: 7,
            remote_dir: "/A".into(),
            filename: "task.json".into(),
        },
    );
    editor
}

fn listing(directory: &str, files: &[&str]) -> Result<DirListing, String> {
    Ok(DirListing {
        resolved_dir: directory.into(),
        json_files: files.iter().map(|name| (*name).into()).collect(),
        total_entries: files.len(),
        subdirs: Vec::new(),
    })
}

/// 无窗口运行实际 Input → Flush → 动作 → Rebuild 排程，避免用调用顺序替代时序验证。
fn submission_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
        .init_asset::<Font>()
        .init_resource::<InputFocus>()
        .init_resource::<FontCx>()
        .init_resource::<LayoutCx>()
        .init_resource::<bevy::clipboard::Clipboard>()
        .insert_resource(WinitSettings::desktop_app())
        .insert_resource(session())
        .insert_resource(editor())
        .init_resource::<StatusLine>()
        .init_resource::<FileBrowser>()
        .configure_sets(
            Update,
            (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
        )
        .add_plugins((
            WorkerBridgePlugin,
            editor_panel::EditorPanelPlugin,
            connect::ConnectPanelPlugin,
            crate::ui::password::PasswordInputPlugin,
        ))
        .add_systems(PostUpdate, bevy::text::apply_text_edits);
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

fn child_text(app: &App, root: Entity) -> Entity {
    app.world()
        .get::<Children>(root)
        .unwrap()
        .iter()
        .find(|child| app.world().get::<EditableText>(*child).is_some())
        .unwrap()
}

fn queue_text(app: &mut App, entity: Entity, value: &str) {
    let mut text = app.world_mut().get_mut::<EditableText>(entity).unwrap();
    text.queue_edit(TextEdit::SelectAll);
    text.queue_edit(TextEdit::Insert(value.to_string().into()));
}

fn float_widget(app: &mut App) -> Entity {
    app.world_mut()
        .spawn_scene(widgets::number_field(
            1.5,
            None,
            ValueBinding::new(vec![0], ValueSlot::Scalar),
        ))
        .unwrap()
        .id()
}

type PendingClipboard = Arc<Mutex<Option<Result<String, ClipboardError>>>>;

/// 桌面后端只产生 Ready，因此直接构造公开的 Pending 状态，模拟 Paste 已发起异步读取。
/// 恢复时走 Bevy 真实的 poll_and_apply_paste 路径，全程不读写系统剪贴板。
fn start_pending_paste(app: &mut App, text: Entity) -> PendingClipboard {
    app.world_mut()
        .get_mut::<EditableText>(text)
        .unwrap()
        .queue_edit(TextEdit::SelectAll);
    app.update();
    let shared = Arc::new(Mutex::new(None));
    app.world_mut()
        .get_mut::<EditableText>(text)
        .unwrap()
        .pending_paste = Some(ClipboardRead::Pending(Arc::clone(&shared)));
    shared
}

#[test]
fn 异步粘贴完成后保留一次保存意图并恢复空闲刷新策略() {
    let mut app = submission_app();
    let number = float_widget(&mut app);
    app.update();
    let text = child_text(&app, number);
    let clipboard = start_pending_paste(&mut app, text);
    let original = app.world().resource::<WinitSettings>().clone();
    app.world_mut().write_message(AppAction::SaveToRemote);
    for _ in 0..2 {
        app.update();
        assert!(matches!(
            app.world().resource::<Session>().busy,
            BusyState::Idle
        ));
        assert_eq!(app.world().resource::<PendingActions>().actions.len(), 1);
        assert_eq!(
            app.world().resource::<StatusLine>().text,
            CLIPBOARD_WAIT_MESSAGE
        );
        let settings = app.world().resource::<WinitSettings>();
        assert_eq!(
            settings.focused_mode,
            UpdateMode::reactive(CLIPBOARD_POLL_INTERVAL)
        );
        assert_eq!(
            settings.unfocused_mode,
            UpdateMode::reactive_low_power(CLIPBOARD_POLL_INTERVAL)
        );
    }
    *clipboard.lock().unwrap() = Some(Ok("7.125".into()));
    app.update();
    assert!(app.world().resource::<PendingActions>().actions.is_empty());
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Working(_)
    ));
    assert!(matches!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .context_fields[0]
            .value,
        ContextValue::Float(7.125)
    ));
    let settings = app.world().resource::<WinitSettings>();
    assert_eq!(settings.focused_mode, original.focused_mode);
    assert_eq!(settings.unfocused_mode, original.unfocused_mode);
    app.update();
    assert!(app.world().resource::<PendingActions>().actions.is_empty());
}

#[test]
fn 异步非法粘贴完成后拒绝创建位姿且保留原文本() {
    let mut app = submission_app();
    app.world_mut()
        .resource_mut::<Editor>()
        .data
        .as_mut()
        .unwrap()
        .context_fields
        .push(model::ContextField {
            key: "waypoint".into(),
            value: ContextValue::Null,
        });
    let number = float_widget(&mut app);
    app.update();
    let version = app.world().resource::<Editor>().structure_version;
    let text = child_text(&app, number);
    let clipboard = start_pending_paste(&mut app, text);
    app.world_mut()
        .write_message(AppAction::CreatePose(vec![1]));
    app.update();
    assert_eq!(app.world().resource::<PendingActions>().actions.len(), 1);
    *clipboard.lock().unwrap() = Some(Ok("1e999".into()));
    app.update();
    assert!(app.world().resource::<PendingActions>().actions.is_empty());
    assert_eq!(app.world().resource::<Editor>().structure_version, version);
    assert!(
        app.world()
            .resource::<StatusLine>()
            .text
            .contains("输入错误")
    );
    assert_eq!(
        app.world()
            .get::<EditableText>(text)
            .unwrap()
            .value()
            .to_string(),
        "1e999"
    );
}

#[test]
fn 连接动作等待异步密码粘贴而不会提前读取旧认证参数() {
    let mut app = submission_app();
    app.world_mut().resource_mut::<Session>().is_connected = false;
    let password = app
        .world_mut()
        .spawn_scene(widgets::password_field("", widgets::NoMarker))
        .unwrap()
        .id();
    app.update();
    let text = child_text(&app, password);
    let clipboard = start_pending_paste(&mut app, text);
    app.world_mut().write_message(AppAction::Connect);
    app.update();
    assert_eq!(
        app.world().resource::<StatusLine>().text,
        CLIPBOARD_WAIT_MESSAGE
    );
    assert_eq!(app.world().resource::<PendingActions>().actions.len(), 1);
    *clipboard.lock().unwrap() = Some(Ok("异步密码".into()));
    app.update();
    assert_eq!(app.world().resource::<Session>().login.password, "异步密码");
    assert!(app.world().resource::<PendingActions>().actions.is_empty());
    // 无窗口夹具在此明确报告缺少事件循环，证明连接动作已恢复且没有真实网络操作。
    assert!(
        app.world()
            .resource::<StatusLine>()
            .text
            .contains("窗口事件循环尚未就绪")
    );
}

#[test]
fn 断开立即取消等待粘贴的保存意图() {
    let mut app = submission_app();
    let number = float_widget(&mut app);
    app.update();
    let text = child_text(&app, number);
    let clipboard = start_pending_paste(&mut app, text);
    app.world_mut().write_message(AppAction::SaveToRemote);
    app.update();
    app.world_mut().write_message(AppAction::Disconnect);
    app.update();
    assert!(app.world().resource::<PendingActions>().actions.is_empty());
    assert!(!app.world().resource::<Session>().is_connected);
    *clipboard.lock().unwrap() = Some(Ok("2.5".into()));
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Idle
    ));
    assert!(app.world().resource::<Editor>().data.is_none());
}

#[test]
fn 同帧数值和元数据编辑先于保存动作写回() {
    let mut app = submission_app();
    let number = float_widget(&mut app);
    let metadata = app
        .world_mut()
        .spawn_scene(widgets::text_field("task", editor_panel::MetaField::TaskId))
        .unwrap()
        .id();
    app.update();
    let number_text = child_text(&app, number);
    let task_text = child_text(&app, metadata);
    queue_text(&mut app, number_text, "2.75");
    queue_text(&mut app, task_text, "renamed");
    app.world_mut().write_message(AppAction::SaveToRemote);
    app.update();

    let session = app.world().resource::<Session>();
    assert!(matches!(session.busy, BusyState::Working(_)));
    let request = save_request(session, app.world().resource::<Editor>()).unwrap();
    let WorkerRequest::SaveFile {
        content,
        new_filename,
        ..
    } = request
    else {
        panic!("必须保存");
    };
    assert_eq!(new_filename.as_deref(), Some("renamed.json"));
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(json["config"]["context"]["speed"], 2.75);
    assert_eq!(json["task_id"], "renamed");
}

#[test]
fn 同帧非法输入拦截保存且修正后一次点击即可保存() {
    let mut app = submission_app();
    let number = float_widget(&mut app);
    app.update();
    let text = child_text(&app, number);
    queue_text(&mut app, text, "1e999");
    app.world_mut().write_message(AppAction::SaveToRemote);
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Idle
    ));
    assert!(
        app.world()
            .resource::<StatusLine>()
            .text
            .contains("输入错误")
    );

    queue_text(&mut app, text, "3.5");
    app.world_mut().write_message(AppAction::SaveToRemote);
    app.update();
    assert!(matches!(
        app.world().resource::<Session>().busy,
        BusyState::Working(_)
    ));
    let data = app.world().resource::<Editor>().data.as_ref().unwrap();
    assert!(matches!(
        data.context_fields[0].value,
        ContextValue::Float(3.5)
    ));
}

#[test]
fn 同帧非法输入不能被创建位姿重建丢弃() {
    let mut app = submission_app();
    app.world_mut()
        .resource_mut::<Editor>()
        .data
        .as_mut()
        .unwrap()
        .context_fields
        .push(model::ContextField {
            key: "waypoint".into(),
            value: ContextValue::Null,
        });
    let version = app.world().resource::<Editor>().structure_version;
    let number = float_widget(&mut app);
    app.update();
    let text = child_text(&app, number);
    queue_text(&mut app, text, "1e999");
    app.world_mut()
        .write_message(AppAction::CreatePose(vec![1]));
    app.update();
    let editor = app.world().resource::<Editor>();
    assert_eq!(editor.structure_version, version);
    assert!(matches!(
        editor.data.as_ref().unwrap().context_fields[1].value,
        ContextValue::Null
    ));

    queue_text(&mut app, text, "2.5");
    app.world_mut()
        .write_message(AppAction::CreatePose(vec![1]));
    app.update();
    let editor = app.world().resource::<Editor>();
    assert_eq!(editor.structure_version, version + 1);
    assert!(matches!(
        editor.data.as_ref().unwrap().context_fields[1].value,
        ContextValue::Pose(_)
    ));
}

#[test]
fn 同帧密码和表单输入在认证参数读取前同步() {
    let mut app = submission_app();
    #[derive(Resource, Default)]
    struct AuthenticationSnapshot(String, String);
    app.init_resource::<AuthenticationSnapshot>().add_systems(
        Update,
        (|session: Res<Session>, mut snapshot: ResMut<AuthenticationSnapshot>| {
            snapshot.0.clone_from(&session.login.host);
            snapshot.1 = match auth_method(&session) {
                AuthMethod::Password(password) => password,
                AuthMethod::PublicKey { .. } => String::new(),
            };
        })
        .before(handle_actions)
        .in_set(UiSet::Update),
    );
    let host_initial = app.world().resource::<Session>().login.host.clone();
    let host = app
        .world_mut()
        .spawn_scene(widgets::text_field(host_initial, connect::LoginField::Host))
        .unwrap()
        .id();
    let password = app
        .world_mut()
        .spawn_scene(widgets::password_field("", widgets::NoMarker))
        .unwrap()
        .id();
    app.update();
    let host_text = child_text(&app, host);
    let password_text = child_text(&app, password);
    queue_text(&mut app, host_text, "robot.example");
    queue_text(&mut app, password_text, "新的密码");
    app.update();
    let snapshot = app.world().resource::<AuthenticationSnapshot>();
    assert_eq!(snapshot.0, "robot.example");
    assert_eq!(snapshot.1, "新的密码");
    let session = app.world().resource::<Session>();
    assert_eq!(session.login.host, "robot.example");
    assert!(
        matches!(auth_method(session), AuthMethod::Password(password) if password == "新的密码")
    );
    // 同帧 PostUpdate 完成遮罩，下一帧再次刷新不会重复应用真实密码。
    assert_eq!(
        app.world()
            .get::<EditableText>(password_text)
            .unwrap()
            .value()
            .to_string(),
        "••••"
    );
    app.update();
    assert_eq!(app.world().resource::<Session>().login.password, "新的密码");
}

#[test]
fn 初始密码遮罩在输入刷新之前就绪且不清空已保存密码() {
    let mut app = submission_app();
    let password = app
        .world_mut()
        .spawn_scene(widgets::password_field("已有密码", widgets::NoMarker))
        .unwrap()
        .id();
    app.update();
    let text = child_text(&app, password);
    assert_eq!(app.world().resource::<Session>().login.password, "已有密码");
    assert_eq!(
        app.world()
            .get::<EditableText>(text)
            .unwrap()
            .value()
            .to_string(),
        "••••"
    );
    app.update();
    assert_eq!(app.world().resource::<Session>().login.password, "已有密码");
}

#[test]
fn 修改表单目录仍保存到原始文档() {
    let mut session = session();
    session.login.remote_dir = "/B".into();
    session.login.host = "另一个尚未连接的主机".into();
    let request = save_request(&session, &editor()).unwrap();
    let WorkerRequest::SaveFile {
        remote_dir,
        current_filename,
        new_filename,
        ..
    } = request
    else {
        panic!("应产生保存请求");
    };
    assert_eq!(remote_dir, "/A");
    assert_eq!(current_filename, "task.json");
    assert!(new_filename.is_none());
}

#[test]
fn 新连接拒绝旧文档及越界文件名() {
    let mut session = session();
    let mut editor = editor();
    session.connection_generation += 1;
    assert!(save_request(&session, &editor).is_err());
    session.connection_generation -= 1;
    for task_id in ["../outside", "dir/file", "dir\\file", "", "\n"] {
        editor.data.as_mut().unwrap().task_id = task_id.into();
        assert!(
            save_request(&session, &editor).is_err(),
            "必须拒绝 {task_id:?}"
        );
    }
    session.is_connected = false;
    assert!(save_request(&session, &editor).is_err());
}

#[test]
fn 切换列表目录清除选中但不改变已加载来源() {
    let editor = editor();
    let mut browser = FileBrowser {
        remote_dir: Some("/A".into()),
        selected: Some("task.json".into()),
        ..default()
    };
    apply_file_list(
        &mut browser,
        &mut StatusLine::default(),
        listing("/B", &["task.json"]),
    );
    assert_eq!(browser.remote_dir.as_deref(), Some("/B"));
    assert!(browser.selected.is_none());
    assert_eq!(editor.document.as_ref().unwrap().remote_dir, "/A");
}

#[test]
fn 保存旧目录不会把新列表切回去且同步改名来源() {
    let mut session = session();
    let mut editor = editor();
    let mut browser = FileBrowser {
        remote_dir: Some("/B".into()),
        files: vec!["task.json".into()],
        selected: None,
        ..default()
    };
    handle_response(
        WorkerResponse::FileSaved {
            remote_dir: "/A".into(),
            old_filename: "task.json".into(),
            new_filename: Some("renamed.json".into()),
            cleanup_warning: None,
            file_list: listing("/A", &["renamed.json"]),
        },
        &mut session,
        &mut StatusLine::default(),
        &mut browser,
        &mut editor,
    );
    assert_eq!(browser.remote_dir.as_deref(), Some("/B"));
    assert_eq!(browser.files, ["task.json"]);
    assert!(browser.selected.is_none());
    assert_eq!(editor.document.as_ref().unwrap().filename, "renamed.json");
}

#[test]
fn 删除其他目录的同名文件不清空文档() {
    let mut session = session();
    let mut editor = editor();
    let mut browser = FileBrowser {
        remote_dir: Some("/B".into()),
        ..default()
    };
    handle_response(
        WorkerResponse::FileDeleted {
            remote_dir: "/B".into(),
            filename: "task.json".into(),
            file_list: listing("/B", &[]),
        },
        &mut session,
        &mut StatusLine::default(),
        &mut browser,
        &mut editor,
    );
    assert!(editor.data.is_some());
    assert_eq!(editor.document.as_ref().unwrap().remote_dir, "/A");
}

#[test]
fn 加载响应绑定实际目录和连接代次() {
    let mut session = session();
    let mut editor = Editor::default();
    handle_response(
        WorkerResponse::FileLoaded {
            remote_dir: "/actual/home/graphs".into(),
            filename: "task.json".into(),
            result: Ok(r#"{"map_id":"m","task_id":"task","config":{"context":{}}}"#.into()),
        },
        &mut session,
        &mut StatusLine::default(),
        &mut FileBrowser::default(),
        &mut editor,
    );
    assert_eq!(
        editor.document.unwrap(),
        RemoteDocument {
            connection_generation: 7,
            remote_dir: "/actual/home/graphs".into(),
            filename: "task.json".into(),
        }
    );
}

#[test]
fn 重连读取域值同时推进表单版本() {
    let mut session = session();
    let version = session.form_version;
    handle_response(
        WorkerResponse::Reconnected {
            ros_domain_id: Some("56".into()),
            file_list: listing("/A", &["task.json"]),
        },
        &mut session,
        &mut StatusLine::default(),
        &mut FileBrowser::default(),
        &mut Editor::default(),
    );
    assert_eq!(session.login.ros_domain_id, "56");
    assert_eq!(session.form_version, version + 1);
    session.update_ros_domain_id(Some("56".into()));
    assert_eq!(session.form_version, version + 1);
}

#[test]
fn 保存成功但清理失败保持来源并明确提示() {
    let mut session = session();
    let mut editor = editor();
    let mut status = StatusLine::default();
    handle_response(
        WorkerResponse::FileSaved {
            remote_dir: "/A".into(),
            old_filename: "task.json".into(),
            new_filename: Some("new.json".into()),
            cleanup_warning: Some("旧文件未删除".into()),
            file_list: listing("/A", &["task.json", "new.json"]),
        },
        &mut session,
        &mut status,
        &mut FileBrowser::default(),
        &mut editor,
    );
    assert_eq!(editor.document.as_ref().unwrap().filename, "new.json");
    assert!(status.text.contains("文件已保存"));
    assert!(status.text.contains("旧文件未删除"));
}
