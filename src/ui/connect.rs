//! 连接面板：SSH 表单、`~/.ssh/config` 主机下拉、连接/断开等操作
//!
//! 表单只在启动时建一次，之后由 observer 单向同步到 [`Session`]；
//! 只有"套用 ssh config 主机"会反向刷新输入框（靠 `form_version` 触发）。
//! 会随状态变化的部分（按钮组、主机菜单项）拆成独立插槽单独重建，
//! 这样重建时不会打断正在输入的文本框。

use bevy::feathers::controls::{
    ButtonVariant, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
};
use bevy::feathers::theme::{ThemeTextColor, ThemedText};
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::input_focus::tab_navigation::{NavAction, TabIndex, TabNavigation};
use bevy::input_focus::{FocusCause, FocusedInput, InputFocus};
use bevy::prelude::*;
use bevy::text::{EditableText, FontWeight, TextEdit, TextEditChange};
use bevy::ui_widgets::{Activate, MenuAction, MenuEvent, MenuFocusState};
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};

use crate::ssh_config::SshHostEntry;

use super::password::PasswordInput;
use super::shell::ConnectSlot;
use super::theme;
use super::widgets::{self, BoxedScene, ButtonGate, boxed};
use super::worker_bridge::AppAction;
use super::{Session, UiSet};

/// 连接表单中的字段
#[derive(Component, Clone, Copy, Default, PartialEq, Eq)]
pub enum LoginField {
    /// 主机名 / IP
    #[default]
    Host,
    /// 端口
    Port,
    /// 用户名
    Username,
    /// 私钥文件路径
    IdentityFile,
    /// ROS_DOMAIN_ID
    RosDomainId,
    /// 远程任务图目录
    RemoteDir,
}

/// 按钮组插槽（连接状态变化时重建）
#[derive(Component, Default, Clone)]
struct ConnectButtonsSlot;

/// 主机下拉菜单弹层插槽（ssh config 重新解析后重建）
#[derive(Component, Default, Clone)]
struct HostMenuSlot;

#[derive(Component, Default, Clone)]
struct HostMenuRoot;

/// 保留打开意图，等配置刷新、BSN 子项落地后再同时显示菜单并获取焦点。
#[derive(Component, Default, Clone)]
struct HostMenuState {
    pending_open: Option<(NavAction, u64)>,
    pending_scroll: Option<NavAction>,
}

/// 记录已渲染的版本号，避免重复重建
///
/// 一律用 `Option` 表示"还没渲染过"，不要拿 0 当哨兵——
/// 版本号从 0 起算，否则第一次更新会被误判成已渲染而丢掉。
#[derive(Resource, Default)]
struct RenderedVersions {
    /// 已渲染的主机列表版本
    hosts: Option<u64>,
    /// 已刷回输入框的表单版本
    form: Option<u64>,
    /// 已渲染的按钮状态（已连接、重连中）
    buttons: Option<(bool, bool)>,
}

/// 构建连接面板
fn connect_panel(session: &Session) -> impl Scene {
    let login = session.login.clone();
    widgets::card(
        "SSH 连接",
        bsn_list![
            host_row(&login.host),
            widgets::form_row("端口", widgets::text_field(login.port, LoginField::Port)),
            widgets::form_row(
                "用户名",
                widgets::text_field(login.username, LoginField::Username)
            ),
            widgets::form_row(
                "密码",
                widgets::password_field(login.password, PasswordFieldMarker)
            ),
            widgets::hint("密码留空则用公钥认证：ssh-agent → 私钥文件"),
            widgets::form_row(
                "私钥",
                widgets::text_field(login.identity_file, LoginField::IdentityFile)
            ),
            widgets::hint("留空则依次尝试 ~/.ssh/id_rsa、id_ecdsa、id_ed25519"),
            widgets::form_row(
                "DOMAIN_ID",
                widgets::text_field(login.ros_domain_id, LoginField::RosDomainId)
            ),
            widgets::form_row(
                "远程目录",
                widgets::text_field(login.remote_dir, LoginField::RemoteDir)
            ),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: px(6),
                    width: percent(100),
                    margin: {UiRect::top(px(2.0))},
                }
                ConnectButtonsSlot
            )
        ],
    )
}

/// 密码框标记（密码值另走 [`PasswordInput`]，这里只作定位用）
#[derive(Component, Default, Clone)]
struct PasswordFieldMarker;

/// 主机行：输入框 + 下拉菜单按钮
fn host_row(host: &str) -> impl Scene {
    let host = host.to_string();
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(6),
            width: percent(100),
        }
        Children [
            (
                Node {
                    width: px(76),
                    flex_shrink: 0.0,
                    align_items: AlignItems::Center,
                    overflow: {Overflow::clip()},
                }
                Children [(
                    Text("主机")
                    ThemeTextColor({theme::FIELD_LABEL})
                    TextFont { font_size: px(12.0) }
                )]
            ),
            (widgets::text_field(host, LoginField::Host)),
            (
                // 菜单根自行管理打开时序；原生 FeathersMenu 会在子项重建前先获取旧焦点。
                Node {
                    height: {bevy::feathers::constants::size::ROW_HEIGHT},
                    justify_content: JustifyContent::Stretch,
                    align_items: AlignItems::Stretch,
                    flex_shrink: 0.0,
                }
                HostMenuRoot
                on(on_host_menu_event)
                on(on_host_menu_key)
                Children [
                    (
                        @FeathersMenuButton {
                            @caption: {bsn! { Text("☰") ThemedText }}
                        }
                        Node { flex_shrink: 0.0 }
                        AccessibleLabel("从 ~/.ssh/config 选择主机")
                    ),
                    (
                        @FeathersMenuPopup
                        Node {
                            min_width: px(300),
                            max_height: px(360),
                            overflow: {Overflow::scroll_y()},
                        }
                        bevy::ui_widgets::ScrollArea
                        HostMenuSlot
                        HostMenuState
                    )
                ]
            )
        ]
    }
}

/// 等待期间焦点仍在按钮上，Escape 不会经过其兄弟弹层，需在菜单根取消意图。
fn on_host_menu_key(
    mut event: On<FocusedInput<KeyboardInput>>,
    roots: Query<&Children, With<HostMenuRoot>>,
    popups: Query<&HostMenuState, With<HostMenuSlot>>,
    mut commands: Commands,
) {
    if event.input.key_code != KeyCode::Escape || event.input.state != ButtonState::Pressed {
        return;
    }
    let Ok(children) = roots.get(event.focused_entity) else {
        return;
    };
    if children.iter().any(|child| {
        popups
            .get(child)
            .is_ok_and(|state| state.pending_open.is_some())
    }) {
        event.propagate(false);
        commands.trigger(MenuEvent {
            source: event.focused_entity,
            action: MenuAction::CloseAll,
        });
    }
}

/// 菜单动作共享的会话、焦点及窗口唤醒资源。
#[derive(bevy::ecs::system::SystemParam)]
struct HostMenuContext<'w> {
    focus: ResMut<'w, InputFocus>,
    session: Res<'w, Session>,
    actions: MessageWriter<'w, AppAction>,
    proxy: Option<Res<'w, EventLoopProxyWrapper>>,
}

/// 鼠标、回车和方向键都经过 MenuEvent，保证每次打开都重新读取配置。
fn on_host_menu_event(
    mut event: On<MenuEvent>,
    roots: Query<&Children, With<HostMenuRoot>>,
    mut popups: Query<
        (&mut HostMenuState, &mut Visibility, &mut MenuFocusState),
        With<HostMenuSlot>,
    >,
    buttons: Query<(), With<FeathersMenuButton>>,
    mut context: HostMenuContext,
) {
    let Ok(children) = roots.get(event.source) else {
        return;
    };
    event.propagate(false);
    let Some(button) = children.iter().find(|&child| buttons.contains(child)) else {
        return;
    };
    let Some(popup) = children.iter().find(|&child| popups.contains(child)) else {
        return;
    };
    let Ok((mut state, mut visibility, mut menu_focus)) = popups.get_mut(popup) else {
        return;
    };
    let navigation = match event.action {
        MenuAction::FocusRoot => {
            context.focus.set(button, FocusCause::Navigated);
            return;
        }
        MenuAction::CloseAll => None,
        MenuAction::Toggle
            if *visibility == Visibility::Visible || state.pending_open.is_some() =>
        {
            None
        }
        MenuAction::Toggle => Some(NavAction::First),
        MenuAction::Open(navigation) => Some(navigation),
    };
    *visibility = Visibility::Hidden;
    *menu_focus = MenuFocusState::Closed;
    state.pending_scroll = None;
    state.pending_open = navigation.map(|navigation| (navigation, context.session.hosts_version));
    if navigation.is_some() {
        context.focus.set(button, FocusCause::Navigated);
        context.actions.write(AppAction::ReloadSshHosts);
        debug!("主机菜单等待配置刷新与子项就绪");
        // 无障碍激活可能晚于本帧动作处理，主动推进下一帧读取配置。
        if let Some(proxy) = &context.proxy {
            let _ = proxy.send_event(WinitUserEvent::WakeUp);
        }
    }
}

/// 等待打开的弹层及其焦点状态，按 BSN 就绪状态一起更新。
type HostMenuPopups<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ChildOf,
        &'static mut HostMenuState,
        &'static mut Visibility,
        &'static mut MenuFocusState,
        Has<widgets::SlotPending>,
    ),
    With<HostMenuSlot>,
>;

/// 子项就绪前保持隐藏；显示与焦点一起提交，避免 MenuPlugin 当帧因丢焦关闭。
fn open_ready_host_menu(
    session: Res<Session>,
    rendered: Res<RenderedVersions>,
    mut popups: HostMenuPopups,
    buttons: Query<(Entity, &ChildOf), With<FeathersMenuButton>>,
    tab_navigation: TabNavigation,
    mut focus: ResMut<InputFocus>,
    proxy: Option<Res<EventLoopProxyWrapper>>,
) {
    for (popup, parent, mut state, mut visibility, mut menu_focus, pending) in &mut popups {
        let Some((navigation, previous_version)) = state.pending_open else {
            continue;
        };
        let button = buttons
            .iter()
            .find_map(|(button, owner)| (owner.parent() == parent.parent()).then_some(button));
        // 打开尚未完成时，用户已点击其他控件也应取消，不能稍后夺回焦点。
        if button.is_none() || focus.get() != button {
            state.pending_open = None;
            continue;
        }
        if session.hosts_version == previous_version {
            // ReloadSshHosts 可能正在等尚未完成的粘贴；桥接层负责短时轮询。
            continue;
        }
        if rendered.hosts == Some(session.hosts_version)
            && !pending
            && let Ok(next) = tab_navigation.initialize(popup, navigation)
        {
            focus.set(next, FocusCause::Navigated);
            *menu_focus = MenuFocusState::Open;
            *visibility = Visibility::Visible;
            state.pending_open = None;
            state.pending_scroll = Some(navigation);
            debug!(hosts = session.ssh_hosts.len(), "主机菜单已打开");
            // 显示状态与焦点提交后，确保后续布局和渲染无需再等用户输入。
            if let Some(proxy) = &proxy {
                let _ = proxy.send_event(WinitUserEvent::WakeUp);
            }
        } else if let Some(proxy) = &proxy {
            // BSN 可能跨帧；主动唤醒，不能等 reactive 的五秒空闲周期。
            let _ = proxy.send_event(WinitUserEvent::WakeUp);
        }
    }
}

/// 使用本批子项的布局尺寸定位首尾，避免重新打开时保留上次滚动位置。
fn position_open_host_menu(
    mut popups: Query<(&mut HostMenuState, &ComputedNode, &mut ScrollPosition), With<HostMenuSlot>>,
    proxy: Option<Res<EventLoopProxyWrapper>>,
) {
    for (mut state, computed, mut scroll) in &mut popups {
        let Some(navigation) = state.pending_scroll.take() else {
            continue;
        };
        let y = match navigation {
            NavAction::Last => {
                (computed.content_size().y - computed.size().y).max(0.0)
                    * computed.inverse_scale_factor
            }
            _ => 0.0,
        };
        if scroll.y != y {
            scroll.y = y;
            // 滚动值在布局后计算，主动请求下一帧把位移应用到控件。
            if let Some(proxy) = &proxy {
                let _ = proxy.send_event(WinitUserEvent::WakeUp);
            }
        }
    }
}

/// 单个 ssh config 主机的菜单项
fn host_menu_item(index: usize, entry: &SshHostEntry) -> impl Scene {
    let alias = entry.alias.clone();
    let target = match &entry.user {
        Some(user) => format!("{user}@{}:{}", entry.host_name, entry.port),
        None => format!("{}:{}", entry.host_name, entry.port),
    };
    // 原先靠悬停提示展示的细节，这里直接排在第二行，信息不丢
    let mut details: Vec<String> = entry
        .identity_files
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    if let Some(jump) = &entry.proxy_jump {
        details.push(format!("ProxyJump {jump}（不支持跳板，将直连）"));
    }
    if let Some(command) = &entry.proxy_command {
        details.push(format!("ProxyCommand {command}（不支持，将直连）"));
    }
    let detail_line = details.join("  ");
    let has_proxy = entry.proxy_jump.is_some() || entry.proxy_command.is_some();
    let proxy_badges: Vec<_> = has_proxy
        .then(|| widgets::badge("跳板"))
        .into_iter()
        .collect();
    let detail_rows: Vec<_> = (!detail_line.is_empty())
        .then(|| widgets::hint(detail_line))
        .into_iter()
        .collect();

    bsn! {
        @FeathersMenuItem {
            @caption: {bsn! {
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(1),
                    width: percent(100),
                }
                ThemedText
                Children [
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: px(8),
                            width: percent(100),
                        }
                        ThemedText
                        Children [
                            (
                                Text(alias)
                                ThemedText
                                TextFont {
                                    font_size: px(12.0),
                                    weight: {FontWeight::BOLD}
                                }
                            ),
                            (widgets::readonly_value(target)),
                            (widgets::spacer()),
                            {proxy_badges}
                        ]
                    ),
                    {detail_rows}
                ]
            }}
        }
        Node {
            height: auto(),
            min_height: {bevy::feathers::constants::size::ROW_HEIGHT},
            flex_shrink: 0.0,
        }
        on(move |_: On<Activate>, mut writer: MessageWriter<AppAction>| {
            writer.write(AppAction::ApplySshHost(index));
        })
    }
}

/// 连接 / 断开 / 刷新 / 上传按钮组
///
/// 只按"连接状态"决定按钮有哪些；忙碌与否是属性，交给 [`ButtonGate`]。
fn connect_buttons(connected: bool, reconnecting: bool) -> Vec<BoxedScene> {
    match (connected, reconnecting) {
        (true, _) => vec![
            boxed(widgets::button_gated(
                "断开",
                ButtonVariant::Normal,
                ButtonGate::Always,
                ActionButton(AppAction::Disconnect),
            )),
            boxed(widgets::button_gated(
                "刷新列表",
                ButtonVariant::Normal,
                ButtonGate::WhenIdle,
                ActionButton(AppAction::RefreshFiles),
            )),
            boxed(widgets::button_gated(
                "上传文件",
                ButtonVariant::Normal,
                ButtonGate::WhenIdle,
                ActionButton(AppAction::UploadFile),
            )),
        ],
        // 连接和重连过程中也能取消，后台通过套接字 shutdown 打断等待。
        (false, true) => vec![boxed(widgets::button_gated(
            "断开",
            ButtonVariant::Normal,
            ButtonGate::Always,
            ActionButton(AppAction::Disconnect),
        ))],
        (false, false) => vec![boxed(widgets::button_gated(
            "连接",
            ButtonVariant::Primary,
            ButtonGate::WhenIdle,
            ActionButton(AppAction::Connect),
        ))],
    }
}

/// 点击后发出指定操作的按钮标记
#[derive(Component, Clone)]
pub struct ActionButton(pub AppAction);

impl Default for ActionButton {
    fn default() -> Self {
        Self(AppAction::RefreshFiles)
    }
}

/// 按钮激活 → 发出对应的操作消息
pub fn on_action_button(
    activate: On<Activate>,
    buttons: Query<&ActionButton>,
    mut writer: MessageWriter<AppAction>,
) {
    if let Ok(button) = buttons.get(activate.entity) {
        writer.write(button.0.clone());
    }
}

/// 首次构建连接面板
fn spawn_connect_panel(
    session: Res<Session>,
    slots: Query<Entity, Added<ConnectSlot>>,
    mut commands: Commands,
) {
    for slot in &slots {
        commands
            .entity(slot)
            .queue_spawn_related_scenes::<Children>(bsn_list![connect_panel(&session)]);
    }
}

/// 按连接状态重建按钮组
fn rebuild_buttons(
    session: Res<Session>,
    mut rendered: ResMut<RenderedVersions>,
    slots: Query<Entity, With<ConnectButtonsSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    let state = (
        session.is_connected,
        session.reconnect_status.is_some()
            || matches!(session.busy, crate::worker::BusyState::Connecting),
    );
    if rendered.buttons == Some(state) {
        return;
    }
    let Ok(slot) = slots.single() else {
        return;
    };
    // 上一批还没落地就先等着，不要推进版本号，下一帧自动重试
    if pending.contains(slot) {
        return;
    }
    rendered.buttons = Some(state);
    widgets::replace_slot_children(&mut commands, slot, connect_buttons(state.0, state.1));
}

/// ssh config 重新解析后重建菜单项
fn rebuild_host_menu(
    session: Res<Session>,
    mut rendered: ResMut<RenderedVersions>,
    slots: Query<Entity, With<HostMenuSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    if rendered.hosts == Some(session.hosts_version) {
        return;
    }
    let Ok(slot) = slots.single() else {
        return;
    };
    if pending.contains(slot) {
        return;
    }
    rendered.hosts = Some(session.hosts_version);

    let items: Vec<BoxedScene> = match session.ssh_hosts.is_empty() {
        true => vec![boxed(bsn! {
            Node { padding: {UiRect::all(px(8.0))} }
            TabIndex(0)
            AccessibleLabel("~/.ssh/config 中没有可用的 Host 条目")
            Children [(widgets::hint("~/.ssh/config 中没有可用的 Host 条目"))]
        })],
        false => session
            .ssh_hosts
            .iter()
            .enumerate()
            .map(|(index, entry)| boxed(host_menu_item(index, entry)))
            .collect(),
    };

    widgets::replace_slot_children(&mut commands, slot, items);
}

/// 文本框内容 → 会话表单
fn on_login_field_edit(
    change: On<TextEditChange>,
    fields: Query<(&LoginField, &EditableText)>,
    mut session: ResMut<Session>,
) {
    let Ok((field, editable)) = fields.get(change.event_target()) else {
        return;
    };
    let value = editable.value().to_string();
    let target = match field {
        LoginField::Host => &mut session.login.host,
        LoginField::Port => &mut session.login.port,
        LoginField::Username => &mut session.login.username,
        LoginField::IdentityFile => &mut session.login.identity_file,
        LoginField::RosDomainId => &mut session.login.ros_domain_id,
        LoginField::RemoteDir => &mut session.login.remote_dir,
    };
    if *target != value {
        *target = value;
    }
}

/// 密码框 → 会话表单
///
/// 密码的真实值由 [`PasswordInput`] 维护（文本框里只有遮罩），
/// 用系统同步可以避开与遮罩 observer 的执行顺序问题。
pub(super) fn sync_password(
    inputs: Query<&PasswordInput, Changed<PasswordInput>>,
    mut session: ResMut<Session>,
) {
    for input in &inputs {
        if session.login.password != input.real {
            session.login.password.clone_from(&input.real);
        }
    }
}

/// 套用 ssh config 主机后，把新值刷回输入框
fn refresh_form_fields(
    session: Res<Session>,
    mut rendered: ResMut<RenderedVersions>,
    mut fields: Query<(&LoginField, &mut EditableText)>,
) {
    if rendered.form == Some(session.form_version) || fields.is_empty() {
        return;
    }
    for (field, mut editable) in &mut fields {
        let expected = match field {
            LoginField::Host => &session.login.host,
            LoginField::Port => &session.login.port,
            LoginField::Username => &session.login.username,
            LoginField::IdentityFile => &session.login.identity_file,
            LoginField::RosDomainId => &session.login.ros_domain_id,
            LoginField::RemoteDir => &session.login.remote_dir,
        };
        // 只在确实不一致时改写，避免打断正在输入的内容
        if editable.value().to_string() != *expected {
            editable.queue_edit(TextEdit::SelectAll);
            editable.queue_edit(TextEdit::Insert(expected.as_str().into()));
        }
    }
    rendered.form = Some(session.form_version);
}

/// 连接面板插件
pub struct ConnectPanelPlugin;

impl Plugin for ConnectPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderedVersions>()
            .add_observer(on_login_field_edit)
            .add_observer(on_action_button)
            .add_systems(
                Update,
                (
                    spawn_connect_panel,
                    rebuild_buttons,
                    rebuild_host_menu,
                    open_ready_host_menu.after(rebuild_host_menu),
                    refresh_form_fields,
                )
                    .in_set(UiSet::Rebuild),
            )
            .add_systems(
                PostUpdate,
                position_open_host_menu.after(bevy::ui::UiSystems::Layout),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LoginConfig;
    use crate::ui::{Editor, StatusLine};
    use bevy::input::InputPlugin;
    use bevy::input::keyboard::Key;
    use bevy::input_focus::InputDispatchPlugin;
    use bevy::scene::ScenePlugin;
    use bevy::text::{FontCx, LayoutCx};
    use bevy::ui_widgets::{MenuItem, MenuPlugin};
    use bevy::window::PrimaryWindow;

    /// 只替代读取磁盘的动作；其余菜单、场景、焦点和表单更新使用实际实现。
    #[derive(Resource)]
    struct TestHosts {
        entries: Vec<SshHostEntry>,
        reloads: usize,
        selections: usize,
        delay_reload: bool,
        pending_reloads: u64,
    }

    fn process_test_actions(
        mut actions: MessageReader<AppAction>,
        mut source: ResMut<TestHosts>,
        mut session: ResMut<Session>,
        mut status: ResMut<StatusLine>,
    ) {
        for action in actions.read() {
            match action {
                AppAction::ReloadSshHosts => {
                    source.reloads += 1;
                    source.pending_reloads += 1;
                }
                AppAction::ApplySshHost(index) => {
                    source.selections += 1;
                    super::super::worker_bridge::apply_ssh_host(&mut session, &mut status, *index);
                }
                _ => panic!("菜单测试收到了无关动作"),
            }
        }
        if !source.delay_reload && source.pending_reloads > 0 {
            session.ssh_hosts = source.entries.clone();
            session.hosts_version += source.pending_reloads;
            source.pending_reloads = 0;
        }
    }

    fn entry(alias: &str) -> SshHostEntry {
        SshHostEntry {
            alias: alias.into(),
            host_name: format!("{alias}.example"),
            port: 2222,
            user: Some("robot".into()),
            identity_files: Vec::new(),
            proxy_jump: None,
            proxy_command: None,
        }
    }

    fn menu_app(entries: Vec<SshHostEntry>) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            ScenePlugin,
            InputPlugin,
            InputDispatchPlugin,
            MenuPlugin,
        ))
        .init_asset::<Font>()
        .init_asset::<Image>()
        .init_resource::<InputFocus>()
        .init_resource::<FontCx>()
        .init_resource::<LayoutCx>()
        .init_resource::<bevy::clipboard::Clipboard>()
        .init_resource::<Editor>()
        .init_resource::<StatusLine>()
        .insert_resource(Session::new(LoginConfig::default(), entries.clone()))
        .insert_resource(TestHosts {
            entries,
            reloads: 0,
            selections: 0,
            delay_reload: false,
            pending_reloads: 0,
        })
        .add_message::<AppAction>()
        .configure_sets(
            Update,
            (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
        )
        .add_plugins((widgets::WidgetsPlugin, ConnectPanelPlugin))
        .add_systems(Update, process_test_actions.in_set(UiSet::Update))
        .add_systems(PostUpdate, bevy::text::apply_text_edits);
        let font = Font::from_bytes(
            include_bytes!("../../assets/fonts/SarasaTermSCNerd-Regular.ttf").to_vec(),
        );
        let mut fonts = app.world_mut().resource_mut::<FontCx>();
        let registered = fonts.collection.register_fonts(font.data, None);
        let family = fonts
            .collection
            .family_name(registered[0].0)
            .unwrap()
            .to_string();
        fonts.set_sans_serif_family(&family).unwrap();
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.world_mut().spawn_scene(host_row("")).unwrap();
        for _ in 0..4 {
            app.update();
        }
        app
    }

    fn entity_with<T: Component>(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<T>>()
            .single(app.world())
            .unwrap()
    }

    fn popup(app: &mut App) -> Entity {
        entity_with::<HostMenuSlot>(app)
    }

    fn activate_button(app: &mut App) {
        let entity = entity_with::<FeathersMenuButton>(app);
        app.world_mut().trigger(Activate { entity });
        app.world_mut().flush();
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
    }

    fn open_and_settle(app: &mut App) {
        let popup = popup(app);
        for _ in 0..8 {
            app.update();
            if *app.world().get::<Visibility>(popup).unwrap() == Visibility::Visible {
                // 再推进两帧，让真实 MenuPlugin 检查菜单焦点没有因重建而消失。
                app.update();
                app.update();
                assert_eq!(
                    app.world().get::<MenuFocusState>(popup),
                    Some(&MenuFocusState::Open)
                );
                assert_eq!(
                    app.world().get::<Visibility>(popup),
                    Some(&Visibility::Visible)
                );
                return;
            }
        }
        panic!("菜单没有在 BSN 子项就绪后打开");
    }

    fn items(app: &App, popup: Entity) -> Vec<Entity> {
        app.world()
            .get::<Children>(popup)
            .unwrap()
            .iter()
            .filter(|&entity| app.world().get::<MenuItem>(entity).is_some())
            .collect()
    }

    #[test]
    fn 主机菜单刷新旧子项后保持打开且重复切换不产生重复项() {
        let mut app = menu_app(vec![entry("old")]);
        let popup = popup(&mut app);
        let old_item = items(&app, popup)[0];
        app.world_mut().resource_mut::<TestHosts>().entries = vec![entry("first"), entry("last")];
        activate_button(&mut app);
        assert!(
            app.world()
                .get::<HostMenuState>(popup)
                .unwrap()
                .pending_open
                .is_some()
        );
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
        open_and_settle(&mut app);
        assert!(app.world().get_entity(old_item).is_err());
        assert_eq!(items(&app, popup).len(), 2);
        for item in items(&app, popup) {
            let node = app.world().get::<Node>(item).unwrap();
            assert_eq!(node.height, Val::Auto);
            assert_eq!(node.flex_shrink, 0.0);
        }
        assert_eq!(app.world().resource::<TestHosts>().reloads, 1);
        for expected_reloads in 2..5 {
            activate_button(&mut app);
            app.update();
            assert_eq!(
                app.world().get::<Visibility>(popup),
                Some(&Visibility::Hidden)
            );
            activate_button(&mut app);
            open_and_settle(&mut app);
            assert_eq!(items(&app, popup).len(), 2);
            assert_eq!(
                app.world().resource::<TestHosts>().reloads,
                expected_reloads
            );
        }
    }

    #[test]
    fn 空主机菜单占位可聚焦并能用逃逸键关闭() {
        let mut app = menu_app(Vec::new());
        activate_button(&mut app);
        open_and_settle(&mut app);
        let popup = popup(&mut app);
        let focus = app.world().resource::<InputFocus>().get().unwrap();
        assert_eq!(app.world().get::<ChildOf>(focus).unwrap().parent(), popup);
        assert!(app.world().get::<TabIndex>(focus).is_some());
        key(&mut app, KeyCode::Escape, Key::Escape);
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
        let button = entity_with::<FeathersMenuButton>(&mut app);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(button));
    }

    #[test]
    fn 方向键打开时重读主机并聚焦首尾选择后同帧更新表单() {
        let mut app = menu_app(vec![entry("first"), entry("last")]);
        let button = entity_with::<FeathersMenuButton>(&mut app);
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(button, FocusCause::Navigated);
        key(&mut app, KeyCode::ArrowUp, Key::ArrowUp);
        open_and_settle(&mut app);
        let popup = popup(&mut app);
        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            items(&app, popup).last().copied()
        );
        key(&mut app, KeyCode::Enter, Key::Enter);
        app.update();
        assert_eq!(app.world().resource::<Session>().login.host, "last.example");
        assert_eq!(app.world().resource::<TestHosts>().selections, 1);
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
        let host_text = app
            .world_mut()
            .query::<(&LoginField, &EditableText)>()
            .iter(app.world())
            .find(|(field, _)| **field == LoginField::Host)
            .unwrap()
            .1
            .value()
            .to_string();
        assert_eq!(host_text, "last.example");

        app.world_mut().resource_mut::<TestHosts>().entries = vec![entry("replacement")];
        key(&mut app, KeyCode::ArrowDown, Key::ArrowDown);
        open_and_settle(&mut app);
        assert_eq!(
            app.world().resource::<InputFocus>().get(),
            items(&app, popup).first().copied()
        );
        assert_eq!(app.world().resource::<TestHosts>().reloads, 2);
        let window = entity_with::<PrimaryWindow>(&mut app);
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(window, FocusCause::Navigated);
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
    }

    #[test]
    fn 菜单等待子项时重复点击或转移焦点会取消打开意图() {
        let mut app = menu_app(vec![entry("first")]);
        let popup = popup(&mut app);
        activate_button(&mut app);
        activate_button(&mut app);
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
        activate_button(&mut app);
        let window = entity_with::<PrimaryWindow>(&mut app);
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(window, FocusCause::Navigated);
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
        assert!(
            app.world()
                .get::<HostMenuState>(popup)
                .unwrap()
                .pending_open
                .is_none()
        );
        assert_eq!(items(&app, popup).len(), 1);
    }

    #[test]
    fn 菜单等待配置时逃逸键取消意图且延迟刷新完成后保持关闭() {
        let mut app = menu_app(vec![entry("first")]);
        let popup = popup(&mut app);
        let version = app.world().resource::<Session>().hosts_version;
        app.world_mut().resource_mut::<TestHosts>().delay_reload = true;
        activate_button(&mut app);
        app.update();
        assert_eq!(app.world().resource::<Session>().hosts_version, version);
        assert!(
            app.world()
                .get::<HostMenuState>(popup)
                .unwrap()
                .pending_open
                .is_some()
        );

        key(&mut app, KeyCode::Escape, Key::Escape);
        app.update();
        let state = app.world().get::<HostMenuState>(popup).unwrap();
        assert!(state.pending_open.is_none());
        assert!(state.pending_scroll.is_none());
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world().get::<MenuFocusState>(popup),
            Some(&MenuFocusState::Closed)
        );

        app.world_mut().resource_mut::<TestHosts>().delay_reload = false;
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(app.world().resource::<Session>().hosts_version, version + 1);
        assert_eq!(
            app.world().get::<Visibility>(popup),
            Some(&Visibility::Hidden)
        );
        let button = entity_with::<FeathersMenuButton>(&mut app);
        assert_eq!(app.world().resource::<InputFocus>().get(), Some(button));
        activate_button(&mut app);
        open_and_settle(&mut app);
        assert_eq!(app.world().resource::<TestHosts>().reloads, 2);
    }

    #[test]
    fn 主机菜单每次打开按方向键重置逻辑像素滚动位置() {
        let mut app = menu_app(vec![entry("first"), entry("last")]);
        let popup = popup(&mut app);
        app.world_mut().entity_mut(popup).insert(ComputedNode {
            size: Vec2::new(600.0, 200.0),
            content_size: Vec2::new(600.0, 1800.0),
            inverse_scale_factor: 0.5,
            ..default()
        });
        let button = entity_with::<FeathersMenuButton>(&mut app);
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(button, FocusCause::Navigated);
        key(&mut app, KeyCode::ArrowUp, Key::ArrowUp);
        open_and_settle(&mut app);
        assert_eq!(app.world().get::<ScrollPosition>(popup).unwrap().y, 800.0);
        key(&mut app, KeyCode::Escape, Key::Escape);
        app.update();
        activate_button(&mut app);
        open_and_settle(&mut app);
        assert_eq!(app.world().get::<ScrollPosition>(popup).unwrap().y, 0.0);
    }
}
