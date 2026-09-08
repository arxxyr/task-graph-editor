//! 桥接层使用纯状态响应验证来源绑定，无需真实窗口、登录配置或 SSH 服务端。

use super::*;
use crate::ui::binding::{PosePart, ValueBinding, ValueSlot};
use crate::ui::{connect, editor as editor_panel, widgets};
use bevy::clipboard::{ClipboardError, ClipboardRead};
use bevy::input_focus::InputFocus;
use bevy::platform::sync::Mutex;
use bevy::scene::ScenePlugin;
use bevy::text::{EditableText, FontCx, LayoutCx, TextEdit};

fn respond(
    response: WorkerResponse,
    session: &mut Session,
    status: &mut StatusLine,
    browser: &mut FileBrowser,
    editor: &mut Editor,
) {
    handle_response(
        response,
        session,
        status,
        browser,
        editor,
        &DocumentInputSnapshot::default(),
    );
}

fn begin_read(session: &mut Session, editor: &mut Editor) -> crate::worker::DocumentReadTicket {
    let ticket = editor.begin_read(
        session.connection_generation,
        DocumentInputSnapshot::default(),
    );
    session.pending_read_ticket = Some(ticket);
    session.busy = BusyState::Loading("next.json".into());
    ticket
}

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

/// 使用混合字符串/对象位姿，并保留模型未知字段，验证回填不会改变原始格式。
fn pose_editor() -> Editor {
    let mut pose = serde_json::to_value(RobotPose::default()).unwrap();
    pose["fixture_metadata"] = serde_json::json!({"station": "A"});
    pose["chassis_pose"]["position"]["precision"] = serde_json::json!("原始扩展字段");
    let text_pose = serde_json::to_string_pretty(&pose).unwrap();
    let context = serde_json::json!({
        "pick_poses": [text_pose.clone(), pose.clone(), text_pose.clone()],
        "home_pose": text_pose.clone(),
        "profiles": {
            "home_pose": text_pose.clone(),
            "pick_poses": [text_pose, pose],
        },
        "speed": 1.5,
        "empty_poses": [],
    });
    let data = model::parse_task_graph(
        &serde_json::json!({"map_id": "m", "task_id": "task", "config": {"context": context}})
            .to_string(),
    )
    .unwrap();
    let mut editor = editor();
    editor.load_remote(data, editor.document.clone().unwrap());
    editor
}

fn context_path(data: &model::TaskGraphData, keys: &[&str]) -> Vec<usize> {
    let mut fields = data.context_fields.as_slice();
    let mut path = Vec::new();
    for key in keys {
        let index = fields.iter().position(|field| field.key == *key).unwrap();
        path.push(index);
        fields = match &fields[index].value {
            ContextValue::NestedGroup(children) => children,
            _ => &[],
        };
    }
    path
}

const TRACKED_POSE_OUTPUT: &str = "pose:
  position:
    x: 0.7197834644638161
    y: 0.18714868614716318
    z: 0.125
  orientation:
    w: 0.9992947295789162
    x: 0.001
    y: 0.002
    z: 0.037550545079943314
---";
const JOINT_OUTPUT: &str = "head_joint_1=-0.314158499 head_joint_2=0.000042716 body_joint_1=0.679999937 body_joint_2=0.299999416";

fn fetch_cases() -> [(AppAction, PosePart, &'static str); 3] {
    [
        (
            AppAction::FetchChassisPose,
            PosePart::Chassis,
            TRACKED_POSE_OUTPUT,
        ),
        (AppAction::FetchHeadJoints, PosePart::Head, JOINT_OUTPUT),
        (AppAction::FetchWaistJoints, PosePart::Waist, JOINT_OUTPUT),
    ]
}

/// 运行真实动作处理系统，确认请求捕获目标和结构版本，全程不连接 SSH。
fn request_pose(
    editor: Editor,
    target: PoseTarget,
    action: AppAction,
) -> (Session, Editor, StatusLine) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(session())
        .insert_resource(editor)
        .init_resource::<StatusLine>()
        .init_resource::<FileBrowser>()
        .init_resource::<InputValidation>()
        .add_message::<DispatchAction>()
        .add_systems(Update, handle_actions);
    app.world_mut()
        .write_message(DispatchAction(AppAction::SelectPose(target)));
    app.world_mut().write_message(DispatchAction(action));
    app.update();
    (
        app.world_mut().remove_resource::<Session>().unwrap(),
        app.world_mut().remove_resource::<Editor>().unwrap(),
        app.world_mut().remove_resource::<StatusLine>().unwrap(),
    )
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
fn 创建位姿经过真实动作排程保留流程导航而重载同份文档回根() {
    use crate::ui::graph_view::{GraphNav, GraphViewPlugin};

    let data = model::parse_task_graph(
        r#"{
            "map_id": "m", "task_id": "task",
            "config": {
                "context": {"waypoint": null},
                "nodes": [{
                    "id": "group", "type": "sequence",
                    "nodes": [{"id": "child", "type": "log", "inputs": {"message": "保留导航"}}],
                    "edges": [{"from": "_entry", "to": "child"}, {"from": "child", "to": "_exit"}]
                }],
                "edges": [{"from": "_entry", "to": "group"}, {"from": "group", "to": "_exit"}]
            }
        }"#,
    )
    .unwrap();
    let field_path = context_path(&data, &["waypoint"]);
    let mut app = submission_app();
    app.add_plugins(GraphViewPlugin);
    app.world_mut().resource_mut::<Editor>().load(Some(data));
    // 先让真实文档加载进入 Rebuild，再模拟使用者已经下钻和选中的状态。
    app.update();
    {
        let mut nav = app.world_mut().resource_mut::<GraphNav>();
        nav.path = vec!["group".into()];
        nav.selected = Some("child".into());
        nav.version += 1;
    }
    let editor = app.world().resource::<Editor>();
    let document_version = editor.document_version;
    let structure_version = editor.structure_version;
    let nav_version = app.world().resource::<GraphNav>().version;

    // 写入原始 AppAction，必须经过 InputFlush 放行及 handle_actions 才能创建位姿。
    app.world_mut()
        .write_message(AppAction::CreatePose(field_path.clone()));
    app.update();
    let editor = app.world().resource::<Editor>();
    assert_eq!(editor.document_version, document_version);
    assert_eq!(editor.structure_version, structure_version + 1);
    let field =
        model::field_at_path(&editor.data.as_ref().unwrap().context_fields, &field_path).unwrap();
    assert_eq!(field.value, ContextValue::Pose(RobotPose::default()));
    assert!(app.world().resource::<PendingActions>().actions.is_empty());
    let nav = app.world().resource::<GraphNav>();
    assert_eq!(nav.path, ["group"]);
    assert_eq!(nav.selected.as_deref(), Some("child"));
    assert_eq!(nav.version, nav_version);

    // 同内容重新加载仍是新文档代次，不能因为图没变化就沿用旧浏览状态。
    let same_data = editor.data.clone();
    app.world_mut().resource_mut::<Editor>().load(same_data);
    app.update();
    let editor = app.world().resource::<Editor>();
    assert_eq!(editor.document_version, document_version + 1);
    assert_eq!(editor.structure_version, structure_version + 2);
    let nav = app.world().resource::<GraphNav>();
    assert!(nav.path.is_empty());
    assert!(nav.selected.is_none());
    assert!(nav.selected_edge.is_none());
    assert_eq!(nav.version, nav_version + 1);
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
    let ticket = editor.next_save_ticket();
    assert!(editor.begin_save(ticket));
    respond(
        WorkerResponse::FileSaved {
            ticket,
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
    let ticket = begin_read(&mut session, &mut editor);
    respond(
        WorkerResponse::FileDeleted {
            ticket,
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
    let ticket = begin_read(&mut session, &mut editor);
    respond(
        WorkerResponse::FileLoaded {
            ticket,
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
    respond(
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
    let ticket = editor.next_save_ticket();
    assert!(editor.begin_save(ticket));
    respond(
        WorkerResponse::FileSaved {
            ticket,
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

#[test]
fn 位姿数组三个取数动作固定原目标且保留其他元素与原始格式() {
    for (action, part, output) in fetch_cases() {
        for index in 0..2 {
            let editor = pose_editor();
            let path = context_path(editor.data.as_ref().unwrap(), &["pick_poses"]);
            let target = PoseTarget::array_element(path.clone(), index);
            let (mut session, mut editor, mut status) =
                request_pose(editor, target.clone(), action.clone());
            assert!(editor.has_pose_selection());
            assert!(matches!(session.busy, BusyState::Fetching(_)));
            let (pending_target, version) = match session.pending_command.as_ref().unwrap() {
                PendingCommand::ChassisPose {
                    target,
                    structure_version,
                }
                | PendingCommand::HeadJoints {
                    target,
                    structure_version,
                }
                | PendingCommand::WaistJoints {
                    target,
                    structure_version,
                } => (target, *structure_version),
            };
            assert_eq!(pending_target, &target);
            assert_eq!(version, editor.structure_version);

            // 请求在途时改变选中和数值版本，结果仍须落在发起时的那一项。
            let other_target = PoseTarget::array_element(path, 2);
            editor.selected_pose = Some(other_target.clone());
            editor.mark_values_changed();
            let value_version = editor.value_version;
            let mut expected = editor.data.as_ref().unwrap().clone();
            let expected_pose = target.pose_mut(&mut expected).unwrap();
            match part {
                PosePart::Chassis => {
                    expected_pose.chassis_pose = model::Pose {
                        position: model::Position {
                            x: 0.7197834644638161,
                            y: 0.18714868614716318,
                            z: 0.125,
                        },
                        orientation: model::Orientation {
                            w: 0.9992947295789162,
                            x: 0.001,
                            y: 0.002,
                            z: 0.037550545079943314,
                        },
                    };
                }
                PosePart::Head => {
                    expected_pose.head_pose.position.x = -0.314158499;
                    expected_pose.head_pose.position.y = 0.000042716;
                }
                PosePart::Waist => {
                    expected_pose.waist_pose.position.x = 0.679999937;
                    expected_pose.waist_pose.position.y = 0.299999416;
                }
            }
            respond(
                WorkerResponse::CommandOutput(Ok(output.into())),
                &mut session,
                &mut status,
                &mut FileBrowser::default(),
                &mut editor,
            );
            let data = editor.data.as_ref().unwrap();
            assert_eq!(data.context_fields, expected.context_fields);
            assert_eq!(editor.selected_pose.as_ref(), Some(&other_target));
            assert_eq!(editor.structure_version, version);
            assert_eq!(editor.value_version, value_version + 1);
            assert!(matches!(session.busy, BusyState::Idle));
            assert!(session.pending_command.is_none());
            assert!(status.text.ends_with(&format!("pick_poses[{index}]")));

            let saved: serde_json::Value =
                serde_json::from_str(&model::serialize_task_graph(data).unwrap()).unwrap();
            let raw_poses = &data.raw_json["config"]["context"]["pick_poses"];
            let saved_poses = &saved["config"]["context"]["pick_poses"];
            for other in 0..3 {
                if other != index {
                    assert_eq!(saved_poses[other], raw_poses[other]);
                }
            }
            assert_eq!(saved_poses[index].is_string(), raw_poses[index].is_string());
            let decoded = match saved_poses[index].as_str() {
                Some(text) => serde_json::from_str::<serde_json::Value>(text).unwrap(),
                None => saved_poses[index].clone(),
            };
            assert_eq!(
                decoded["fixture_metadata"],
                serde_json::json!({"station": "A"})
            );
            assert_eq!(
                decoded["chassis_pose"]["position"]["precision"],
                "原始扩展字段"
            );
            let reparsed = model::parse_task_graph(&saved.to_string()).unwrap();
            assert_eq!(reparsed.context_fields, expected.context_fields);
        }
    }
}

#[test]
fn 独立与嵌套位姿及嵌套数组均可通过取数动作回填() {
    for (keys, array_index, label) in [
        (vec!["home_pose"], None, "home_pose"),
        (vec!["profiles", "home_pose"], None, "profiles.home_pose"),
        (
            vec!["profiles", "pick_poses"],
            Some(1),
            "profiles.pick_poses[1]",
        ),
    ] {
        let editor = pose_editor();
        let target = PoseTarget {
            field_path: context_path(editor.data.as_ref().unwrap(), &keys),
            array_index,
        };
        let (mut session, mut editor, mut status) =
            request_pose(editor, target.clone(), AppAction::FetchChassisPose);
        assert!(editor.has_pose_selection());
        respond(
            WorkerResponse::CommandOutput(Ok(TRACKED_POSE_OUTPUT.into())),
            &mut session,
            &mut status,
            &mut FileBrowser::default(),
            &mut editor,
        );
        assert_eq!(
            target
                .pose(editor.data.as_ref().unwrap())
                .unwrap()
                .chassis_pose
                .position
                .x,
            0.7197834644638161
        );
        assert!(status.text.ends_with(label));
    }
}

#[test]
fn 无效位姿目标不能发起任何取数命令() {
    let editor = pose_editor();
    let data = editor.data.as_ref().unwrap();
    let invalid_targets = [
        PoseTarget::default(),
        PoseTarget::field(context_path(data, &["pick_poses"])),
        PoseTarget::array_element(context_path(data, &["pick_poses"]), 3),
        PoseTarget::array_element(context_path(data, &["home_pose"]), 0),
        PoseTarget::array_element(context_path(data, &["empty_poses"]), 0),
        PoseTarget::field(context_path(data, &["speed"])),
    ];
    for target in invalid_targets {
        for (action, _, _) in fetch_cases() {
            let (session, editor, status) = request_pose(pose_editor(), target.clone(), action);
            assert!(!editor.has_pose_selection());
            assert!(session.pending_command.is_none());
            assert!(matches!(session.busy, BusyState::Idle));
            assert_eq!(status.text, "选中的字段不是位姿类型");
        }
    }
}

#[test]
fn 位姿回填拒绝重载同名同形文档及字段结构变化后的旧响应() {
    for reload in [true, false] {
        for (action, _, output) in fetch_cases() {
            let editor = pose_editor();
            let target = PoseTarget::array_element(
                context_path(editor.data.as_ref().unwrap(), &["pick_poses"]),
                0,
            );
            let (mut session, mut editor, mut status) = request_pose(editor, target, action);
            let expected = editor.data.as_ref().unwrap().context_fields.clone();
            let value_version = editor.value_version;
            match reload {
                true => editor.load_remote(
                    editor.data.clone().unwrap(),
                    editor.document.clone().unwrap(),
                ),
                false => editor.mark_structure_changed(),
            }
            respond(
                WorkerResponse::CommandOutput(Ok(output.into())),
                &mut session,
                &mut status,
                &mut FileBrowser::default(),
                &mut editor,
            );
            assert_eq!(editor.data.as_ref().unwrap().context_fields, expected);
            assert_eq!(editor.value_version, value_version);
            assert!(session.pending_command.is_none());
            assert!(status.text.contains("过期"));
        }
    }
}

#[test]
fn 位姿回填拒绝已经删除或改变类型的目标() {
    for remove_element in [true, false] {
        let editor = pose_editor();
        let target = PoseTarget::array_element(
            context_path(editor.data.as_ref().unwrap(), &["pick_poses"]),
            2,
        );
        let (mut session, mut editor, mut status) =
            request_pose(editor, target.clone(), AppAction::FetchChassisPose);
        let data = editor.data.as_mut().unwrap();
        let field = model::field_at_path_mut(&mut data.context_fields, &target.field_path).unwrap();
        match (&mut field.value, remove_element) {
            (ContextValue::PoseArray(poses), true) => {
                poses.pop();
            }
            (value, false) => *value = ContextValue::Text("类型已变化".into()),
            _ => unreachable!("测试夹具应为位姿数组"),
        }
        let expected = data.context_fields.clone();
        let value_version = editor.value_version;
        respond(
            WorkerResponse::CommandOutput(Ok(TRACKED_POSE_OUTPUT.into())),
            &mut session,
            &mut status,
            &mut FileBrowser::default(),
            &mut editor,
        );
        assert_eq!(editor.data.as_ref().unwrap().context_fields, expected);
        assert_eq!(editor.value_version, value_version);
        assert_eq!(status.text, "目标位姿字段已不存在");
    }
}

#[test]
fn 位姿获取失败或无法解析时不改变任何数据() {
    for (action, _, _) in fetch_cases() {
        for result in [Err("读取超时".into()), Ok("无法解析的输出".into())] {
            let editor = pose_editor();
            let target = PoseTarget::array_element(
                context_path(editor.data.as_ref().unwrap(), &["pick_poses"]),
                0,
            );
            let (mut session, mut editor, mut status) =
                request_pose(editor, target, action.clone());
            let expected = editor.data.as_ref().unwrap().context_fields.clone();
            let value_version = editor.value_version;
            respond(
                WorkerResponse::CommandOutput(result),
                &mut session,
                &mut status,
                &mut FileBrowser::default(),
                &mut editor,
            );
            assert_eq!(editor.data.as_ref().unwrap().context_fields, expected);
            assert_eq!(editor.value_version, value_version);
            assert!(session.pending_command.is_none());
            assert!(status.text.contains("失败"));
        }
    }
}

#[test]
#[ignore = "需要 TASK_GRAPH_REAL_FILE 和 TGE_TRACKED_POSE_FILE 指定只读文件副本"]
fn 真实文件位姿数组选择后使用真实底盘输出逐项回填往返() {
    let file = std::env::var("TASK_GRAPH_REAL_FILE").expect("需要 TASK_GRAPH_REAL_FILE");
    let output_file = std::env::var("TGE_TRACKED_POSE_FILE").expect("需要 TGE_TRACKED_POSE_FILE");
    let field_name = std::env::var("TGE_POSE_ARRAY_FIELD").unwrap_or_else(|_| "pick_poses".into());
    let content = std::fs::read_to_string(file).expect("读取任务图副本失败");
    let output = std::fs::read_to_string(output_file).expect("读取 ROS2 输出副本失败");
    let chassis = model::parse_tracked_pose(&output).expect("必须提供有效 tracked_pose 输出");
    let original = model::parse_task_graph(&content).expect("任务图副本必须能解析");
    let path = context_path(&original, &[&field_name]);
    let field = model::field_at_path(&original.context_fields, &path).unwrap();
    let ContextValue::PoseArray(poses) = &field.value else {
        panic!("{field_name} 应被识别为位姿数组");
    };
    assert!(!poses.is_empty());
    let raw_poses = original.raw_json["config"]["context"][&field_name]
        .as_array()
        .unwrap();
    for index in 0..poses.len() {
        let mut editor = editor();
        editor.load_remote(original.clone(), editor.document.clone().unwrap());
        let target = PoseTarget::array_element(path.clone(), index);
        let (mut session, mut editor, mut status) =
            request_pose(editor, target.clone(), AppAction::FetchChassisPose);
        assert!(editor.has_pose_selection());
        assert!(session.pending_command.is_some());
        // 改选另一项，响应仍须准确写回发起请求的下标。
        editor.selected_pose = Some(PoseTarget::array_element(
            path.clone(),
            (index + 1) % poses.len(),
        ));
        respond(
            WorkerResponse::CommandOutput(Ok(output.clone())),
            &mut session,
            &mut status,
            &mut FileBrowser::default(),
            &mut editor,
        );
        let data = editor.data.as_ref().unwrap();
        let mut expected = original.clone();
        target.pose_mut(&mut expected).unwrap().chassis_pose = chassis.clone();
        assert_eq!(
            data.context_fields, expected.context_fields,
            "第 {index} 项应只更新底盘部位"
        );
        let saved = model::serialize_task_graph(data).unwrap();
        let saved_json: serde_json::Value = serde_json::from_str(&saved).unwrap();
        let saved_poses = saved_json["config"]["context"][&field_name]
            .as_array()
            .unwrap();
        assert_eq!(saved_poses.len(), raw_poses.len());
        for (other, (saved_pose, raw_pose)) in saved_poses.iter().zip(raw_poses).enumerate() {
            assert_eq!(
                saved_pose.is_string(),
                raw_pose.is_string(),
                "第 {other} 项格式必须保留"
            );
            if other != index {
                assert_eq!(saved_pose, raw_pose, "第 {other} 项原始内容不得变化");
            }
        }
        let reparsed = model::parse_task_graph(&saved).unwrap();
        assert_eq!(reparsed.context_fields, expected.context_fields);
        let actual = &target.pose(&reparsed).unwrap().chassis_pose;
        for comp in crate::ui::binding::PoseComp::POSITION
            .into_iter()
            .chain(crate::ui::binding::PoseComp::ORIENTATION)
        {
            assert_eq!(
                comp.get(actual).to_bits(),
                comp.get(&chassis).to_bits(),
                "第 {index} 项的 {comp:?} 精度不一致"
            );
        }
        assert!(status.text.ends_with(&format!("{field_name}[{index}]")));
    }
    println!(
        "{field_name}：{} 个元素逐项选择与回填通过，全部 {} 个底盘 f64 分量位级一致，其他项原始内容及各项存储格式保持不变",
        poses.len(),
        poses.len() * 7
    );
}

fn loaded(
    ticket: crate::worker::DocumentReadTicket,
    result: Result<String, String>,
) -> WorkerResponse {
    WorkerResponse::FileLoaded {
        ticket,
        remote_dir: "/A".into(),
        filename: "next.json".into(),
        result,
    }
}

fn loaded_content() -> String {
    r#"{"map_id":"next_map","task_id":"next","config":{"context":{"speed":2}}}"#.into()
}

fn graph_control(
    editing: &mut super::super::graph_edit::GraphEditing,
    editor: &mut Editor,
    control: super::super::graph_edit::GraphControl,
) {
    use super::super::graph_edit::{GraphIntent, GraphOperation};
    let intent = GraphIntent {
        document_version: editor.document_version,
        revision: editor.data.as_ref().unwrap().graph_edit.revision(),
        operation: GraphOperation::Control(control),
    };
    editing
        .handle(
            &intent,
            editor,
            &mut super::super::graph_view::GraphNav::default(),
        )
        .unwrap();
}

#[test]
fn 加载成功读取失败和解析失败均不覆盖等待期间的新修改或流程历史() {
    use crate::model::graph_edit::GraphCommand;
    for outcome in [
        Ok(loaded_content()),
        Ok("不是合法JSON".into()),
        Err("读取失败".into()),
    ] {
        for changed in ["graph", "context", "task_id", "map_id"] {
            let mut session = session();
            let mut editor = editor();
            let ticket = begin_read(&mut session, &mut editor);
            let version = editor.document_version;
            let original_source = editor.document.clone();
            match changed {
                "graph" => {
                    let data = editor.data.as_mut().unwrap();
                    data.apply_graph_command(
                        0,
                        GraphCommand::AddNode {
                            graph: data.graph_edit.root_key(),
                            value: serde_json::json!({"id":"new","type":"custom"}),
                        },
                    )
                    .unwrap();
                }
                "context" => {
                    editor.data.as_mut().unwrap().context_fields[0].value = ContextValue::Float(9.5)
                }
                "task_id" => editor.data.as_mut().unwrap().task_id = "edited".into(),
                "map_id" => editor.data.as_mut().unwrap().map_id = "edited".into(),
                _ => unreachable!(),
            }
            let before = super::super::document_state::DocumentSnapshot::capture(
                editor.data.as_ref().unwrap(),
            );
            let mut status = StatusLine::default();
            let mut browser = FileBrowser {
                remote_dir: Some("/A".into()),
                selected: Some("next.json".into()),
                ..default()
            };
            respond(
                loaded(ticket, outcome.clone()),
                &mut session,
                &mut status,
                &mut browser,
                &mut editor,
            );
            assert!(
                before
                    == super::super::document_state::DocumentSnapshot::capture(
                        editor.data.as_ref().unwrap()
                    )
            );
            assert_eq!(editor.document_version, version);
            assert_eq!(editor.document, original_source);
            assert_eq!(browser.selected.as_deref(), Some("task.json"));
            assert!(status.text.contains("已保留当前修改"));
            assert!(matches!(session.busy, BusyState::Idle));
            assert!(session.pending_read_ticket.is_none());
            if changed == "graph" {
                assert!(editor.data.as_ref().unwrap().graph_edit.can_undo());
            }
        }
    }
}

#[test]
fn 加载失败保留原文档而发起前已确认放弃的脏内容不阻止成功切换() {
    for outcome in [
        Ok(loaded_content()),
        Ok("解析错误".into()),
        Err("读取错误".into()),
    ] {
        let success = outcome.as_ref().is_ok_and(|text| text == &loaded_content());
        let mut session = session();
        let mut editor = editor();
        editor.data.as_mut().unwrap().task_id = "发起前的旧修改".into();
        let version = editor.document_version;
        let ticket = begin_read(&mut session, &mut editor);
        let mut status = StatusLine::default();
        respond(
            loaded(ticket, outcome),
            &mut session,
            &mut status,
            &mut FileBrowser::default(),
            &mut editor,
        );
        match success {
            true => {
                assert_eq!(editor.data.as_ref().unwrap().task_id, "next");
                assert_eq!(editor.document_version, version + 1);
                assert_eq!(editor.document.as_ref().unwrap().filename, "next.json");
            }
            false => {
                assert_eq!(editor.data.as_ref().unwrap().task_id, "发起前的旧修改");
                assert_eq!(editor.document_version, version);
                assert!(status.text.contains("当前文档已保留"));
            }
        }
        assert!(matches!(session.busy, BusyState::Idle));
    }
}

#[test]
fn 过期加载回执不清新请求忙碌状态而重载使原回执失效时正常结束忙碌() {
    let mut session = session();
    let mut editor = editor();
    let old = begin_read(&mut session, &mut editor);
    let new = begin_read(&mut session, &mut editor);
    let mut status = StatusLine::default();
    respond(
        loaded(old, Ok(loaded_content())),
        &mut session,
        &mut status,
        &mut FileBrowser::default(),
        &mut editor,
    );
    assert_eq!(session.pending_read_ticket, Some(new));
    assert!(matches!(session.busy, BusyState::Loading(_)));
    assert_eq!(editor.document.as_ref().unwrap().filename, "task.json");
    let source = editor.document.clone().unwrap();
    editor.load_remote(model::parse_task_graph(&loaded_content()).unwrap(), source);
    respond(
        loaded(new, Ok(loaded_content())),
        &mut session,
        &mut status,
        &mut FileBrowser::default(),
        &mut editor,
    );
    assert!(session.pending_read_ticket.is_none());
    assert!(matches!(session.busy, BusyState::Idle));
    assert!(status.text.contains("回执已过期"));
    assert_eq!(editor.document.as_ref().unwrap().filename, "task.json");
}

#[test]
fn 加载期间新草稿或已确认放弃草稿的后续变化仍保留() {
    use super::super::graph_edit::{GraphControl, GraphEditing};
    for existing_draft in [false, true] {
        let mut session = session();
        let mut editor = editor();
        let mut editing = GraphEditing::default();
        graph_control(&mut editing, &mut editor, GraphControl::Toggle);
        if existing_draft {
            graph_control(&mut editing, &mut editor, GraphControl::NewNode);
        }
        let validation = InputValidation::default();
        let input = DocumentInputSnapshot::capture(Some(&editing), &validation);
        let ticket = editor.begin_read(session.connection_generation, input);
        session.pending_read_ticket = Some(ticket);
        session.busy = BusyState::Loading("next.json".into());
        match existing_draft {
            true => graph_control(&mut editing, &mut editor, GraphControl::RemoveProperty(2)),
            false => graph_control(&mut editing, &mut editor, GraphControl::NewNode),
        }
        let input = DocumentInputSnapshot::capture(Some(&editing), &validation);
        let mut status = StatusLine::default();
        handle_response(
            loaded(ticket, Ok(loaded_content())),
            &mut session,
            &mut status,
            &mut FileBrowser::default(),
            &mut editor,
            &input,
        );
        assert!(editing.has_draft_changes());
        assert_eq!(editor.document.as_ref().unwrap().filename, "task.json");
        assert!(status.text.contains("已保留当前修改"));
    }
}

#[test]
fn 加载期间同一个非法数字框继续输入也被回执守卫识别() {
    let mut app = submission_app();
    let number = float_widget(&mut app);
    app.update();
    let field = child_text(&app, number);
    queue_text(&mut app, field, "1e");
    app.update();
    assert!(
        app.world()
            .resource::<InputValidation>()
            .has_errors(app.world().resource::<Editor>())
    );
    let input = DocumentInputSnapshot::capture(None, app.world().resource::<InputValidation>());
    let generation = app.world().resource::<Session>().connection_generation;
    let ticket = app
        .world_mut()
        .resource_mut::<Editor>()
        .begin_read(generation, input);
    app.world_mut()
        .resource_mut::<Session>()
        .pending_read_ticket = Some(ticket);
    app.world_mut().resource_mut::<Session>().busy = BusyState::Loading("next.json".into());
    queue_text(&mut app, field, "1e-");
    app.update();
    let input = DocumentInputSnapshot::capture(None, app.world().resource::<InputValidation>());
    let mut session = app.world_mut().remove_resource::<Session>().unwrap();
    let mut editor = app.world_mut().remove_resource::<Editor>().unwrap();
    let mut status = StatusLine::default();
    handle_response(
        loaded(ticket, Ok(loaded_content())),
        &mut session,
        &mut status,
        &mut FileBrowser::default(),
        &mut editor,
        &input,
    );
    assert_eq!(
        app.world()
            .get::<EditableText>(field)
            .unwrap()
            .value()
            .to_string(),
        "1e-"
    );
    assert_eq!(editor.document.as_ref().unwrap().filename, "task.json");
    assert!(status.text.contains("已保留当前修改"));
    assert!(matches!(session.busy, BusyState::Idle));
}

#[test]
fn 删除当前文件时等待期间的新修改保留且解除已删除来源但未变化可正常清空() {
    for changed in [false, true] {
        let mut session = session();
        let mut editor = editor();
        let ticket = begin_read(&mut session, &mut editor);
        let version = editor.document_version;
        if changed {
            editor.data.as_mut().unwrap().map_id = "等待期间的新修改".into();
        }
        let mut status = StatusLine::default();
        respond(
            WorkerResponse::FileDeleted {
                ticket,
                remote_dir: "/A".into(),
                filename: "task.json".into(),
                file_list: listing("/A", &[]),
            },
            &mut session,
            &mut status,
            &mut FileBrowser::default(),
            &mut editor,
        );
        match changed {
            true => {
                assert_eq!(editor.data.as_ref().unwrap().map_id, "等待期间的新修改");
                assert_eq!(editor.document_version, version);
                assert!(editor.document.is_none());
                assert!(editor.has_unsaved_changes());
                assert!(save_request(&session, &editor).is_err());
                assert!(status.text.contains("新修改已保留在内存"));
            }
            false => {
                assert!(editor.data.is_none());
                assert_eq!(editor.document_version, version + 1);
            }
        }
        assert!(matches!(session.busy, BusyState::Idle));
        assert!(session.pending_read_ticket.is_none());
    }
}

#[test]
fn 删除失败保留修改且旧删除回执不清新加载请求() {
    let mut session = session();
    let mut editor = editor();
    let old = begin_read(&mut session, &mut editor);
    let current = begin_read(&mut session, &mut editor);
    let mut status = StatusLine::default();
    respond(
        WorkerResponse::DeleteFailed {
            ticket: old,
            error: "旧失败".into(),
        },
        &mut session,
        &mut status,
        &mut FileBrowser::default(),
        &mut editor,
    );
    assert_eq!(session.pending_read_ticket, Some(current));
    assert!(matches!(session.busy, BusyState::Loading(_)));
    editor.data.as_mut().unwrap().map_id = "新地图".into();
    respond(
        WorkerResponse::DeleteFailed {
            ticket: current,
            error: "未删除".into(),
        },
        &mut session,
        &mut status,
        &mut FileBrowser::default(),
        &mut editor,
    );
    assert_eq!(editor.data.as_ref().unwrap().map_id, "新地图");
    assert!(editor.document.is_some());
    assert!(session.pending_read_ticket.is_none());
    assert!(matches!(session.busy, BusyState::Idle));
}

#[test]
fn 加载回执不能丢弃等待期间尚未完成的异步粘贴() {
    let mut app = submission_app();
    let number = float_widget(&mut app);
    app.update();
    let field = child_text(&app, number);
    let input = DocumentInputSnapshot::capture(None, app.world().resource::<InputValidation>());
    let generation = app.world().resource::<Session>().connection_generation;
    let ticket = app
        .world_mut()
        .resource_mut::<Editor>()
        .begin_read(generation, input);
    app.world_mut()
        .resource_mut::<Session>()
        .pending_read_ticket = Some(ticket);
    app.world_mut().resource_mut::<Session>().busy = BusyState::Loading("next.json".into());
    let clipboard = start_pending_paste(&mut app, field);
    let input = DocumentInputSnapshot::capture(None, app.world().resource::<InputValidation>())
        .with_pending_input(
            app.world()
                .get::<EditableText>(field)
                .unwrap()
                .pending_paste
                .is_some(),
        );
    let mut session = app.world_mut().remove_resource::<Session>().unwrap();
    let mut editor = app.world_mut().remove_resource::<Editor>().unwrap();
    let mut status = StatusLine::default();
    handle_response(
        loaded(ticket, Ok(loaded_content())),
        &mut session,
        &mut status,
        &mut FileBrowser::default(),
        &mut editor,
        &input,
    );
    assert!(status.text.contains("已保留当前修改"));
    assert_eq!(editor.document.as_ref().unwrap().filename, "task.json");
    app.insert_resource(session).insert_resource(editor);
    *clipboard.lock().unwrap() = Some(Ok("3.5".into()));
    app.update();
    assert_eq!(
        app.world()
            .get::<EditableText>(field)
            .unwrap()
            .value()
            .to_string(),
        "3.5"
    );
    assert_eq!(
        app.world()
            .resource::<Editor>()
            .data
            .as_ref()
            .unwrap()
            .context_fields[0]
            .value,
        ContextValue::Float(3.5)
    );
}

#[derive(Resource)]
struct InjectedReadResponse(Option<WorkerResponse>);

/// 在真实 InputFlush 之后、动作处理之前注入回执，覆盖单帧队列与后台完成同时到达。
fn inject_read_response(
    mut response: ResMut<InjectedReadResponse>,
    mut session: ResMut<Session>,
    mut status: ResMut<StatusLine>,
    mut browser: ResMut<FileBrowser>,
    mut editor: ResMut<Editor>,
    inputs: ResponseInputs,
) {
    if let Some(response) = response.0.take() {
        handle_response(
            response,
            &mut session,
            &mut status,
            &mut browser,
            &mut editor,
            &inputs.snapshot(),
        );
    }
}

#[test]
fn 同帧加载回执不能将已排队保存或流程操作转移到新文档() {
    use super::super::graph_edit::{GraphEditing, GraphIntent, GraphOperation};
    use crate::model::graph_edit::GraphCommand;
    for save in [true, false] {
        let mut app = submission_app();
        let mut editing = GraphEditing::default();
        graph_control(
            &mut editing,
            &mut app.world_mut().resource_mut::<Editor>(),
            super::super::graph_edit::GraphControl::Toggle,
        );
        app.insert_resource(editing)
            .init_resource::<super::super::graph_view::GraphNav>()
            .insert_resource(InjectedReadResponse(None))
            .add_systems(
                Update,
                inject_read_response.after(InputFlush).before(UiSet::Update),
            );
        let document_version = app.world().resource::<Editor>().document_version;
        let generation = app.world().resource::<Session>().connection_generation;
        let ticket = app
            .world_mut()
            .resource_mut::<Editor>()
            .begin_read(generation, DocumentInputSnapshot::default());
        app.world_mut()
            .resource_mut::<Session>()
            .pending_read_ticket = Some(ticket);
        app.world_mut().resource_mut::<Session>().busy = BusyState::Loading("next.json".into());
        app.world_mut().resource_mut::<InjectedReadResponse>().0 =
            Some(loaded(ticket, Ok(loaded_content())));
        let action = match save {
            true => AppAction::SaveToRemote,
            false => AppAction::GraphEdit(GraphIntent {
                document_version,
                revision: 0,
                operation: GraphOperation::Command(GraphCommand::AddNode {
                    graph: app
                        .world()
                        .resource::<Editor>()
                        .data
                        .as_ref()
                        .unwrap()
                        .graph_edit
                        .root_key(),
                    value: serde_json::json!({"id":"old_document_edit","type":"custom"}),
                }),
            }),
        };
        app.world_mut().write_message(action);
        app.update();
        let editor = app.world().resource::<Editor>();
        assert_eq!(editor.document_version, document_version);
        assert_eq!(editor.document.as_ref().unwrap().filename, "task.json");
        match save {
            true => assert!(editor.pending_save.is_some()),
            false => {
                assert_eq!(
                    editor.data.as_ref().unwrap().graph.nodes[0].id,
                    "old_document_edit"
                );
                assert_eq!(editor.data.as_ref().unwrap().graph_edit.revision(), 1);
            }
        }
    }
}
