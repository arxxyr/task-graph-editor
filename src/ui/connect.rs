//! 连接面板：SSH 表单、`~/.ssh/config` 主机下拉、连接/断开等操作
//!
//! 表单只在启动时建一次，之后由 observer 单向同步到 [`Session`]；
//! 只有"套用 ssh config 主机"会反向刷新输入框（靠 `form_version` 触发）。
//! 会随状态变化的部分（按钮组、主机菜单项）拆成独立插槽单独重建，
//! 这样重建时不会打断正在输入的文本框。

use bevy::feathers::controls::{
    ButtonVariant, FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
};
use bevy::feathers::theme::{ThemeTextColor, ThemedText};
use bevy::prelude::*;
use bevy::text::{EditableText, FontWeight, TextEdit, TextEditChange};
use bevy::ui_widgets::Activate;

use crate::ssh_config::SshHostEntry;

use super::password::PasswordInput;
use super::shell::ConnectSlot;
use super::theme;
use super::widgets::{self, BoxedScene, boxed};
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
    /// 已渲染的按钮状态（已连接、重连中、忙碌）
    buttons: Option<(bool, bool, bool)>,
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
                }
                Children [(
                    Text("主机")
                    ThemeTextColor({theme::FIELD_LABEL})
                    TextFont { font_size: px(12.0) }
                )]
            ),
            (widgets::text_field(host, LoginField::Host)),
            (
                @FeathersMenu
                Node { flex_shrink: 0.0 }
                Children [
                    (
                        @FeathersMenuButton {
                            @caption: {bsn! { Text("☰") ThemedText }}
                        }
                        Node { flex_shrink: 0.0 }
                        AccessibleLabel("从 ~/.ssh/config 选择主机")
                        on(|_: On<Activate>, mut writer: MessageWriter<AppAction>| {
                            writer.write(AppAction::ReloadSshHosts);
                        })
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
                    )
                ]
            )
        ]
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
                Children [
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: px(8),
                            width: percent(100),
                        }
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
        on(move |_: On<Activate>, mut writer: MessageWriter<AppAction>| {
            writer.write(AppAction::ApplySshHost(index));
        })
    }
}

/// 连接 / 断开 / 刷新 / 上传按钮组
fn connect_buttons(connected: bool, reconnecting: bool, busy: bool) -> Vec<BoxedScene> {
    // 重连中只保留断开；已连接给出全套远程操作；未连接只有连接按钮
    let idle = !busy;
    match (connected, reconnecting) {
        (true, _) => vec![
            boxed(widgets::button_enabled(
                "断开",
                ButtonVariant::Normal,
                idle,
                ActionButton(AppAction::Disconnect),
            )),
            boxed(widgets::button_enabled(
                "刷新列表",
                ButtonVariant::Normal,
                idle,
                ActionButton(AppAction::RefreshFiles),
            )),
            boxed(widgets::button_enabled(
                "上传文件",
                ButtonVariant::Normal,
                idle,
                ActionButton(AppAction::UploadFile),
            )),
        ],
        // 重连过程中「断开」始终可用，否则无法中止自动重连
        (false, true) => vec![boxed(widgets::button(
            "断开",
            ButtonVariant::Normal,
            ActionButton(AppAction::Disconnect),
        ))],
        (false, false) => {
            let label = match busy {
                true => "连接中...",
                false => "连接",
            };
            vec![boxed(widgets::button_enabled(
                label,
                ButtonVariant::Primary,
                idle,
                ActionButton(AppAction::Connect),
            ))]
        }
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
    mut commands: Commands,
) {
    let state = (
        session.is_connected,
        session.reconnect_status.is_some(),
        session.is_busy(),
    );
    if rendered.buttons == Some(state) {
        return;
    }
    let Ok(slot) = slots.single() else {
        return;
    };
    rendered.buttons = Some(state);
    commands
        .entity(slot)
        .despawn_related::<Children>()
        .queue_spawn_related_scenes::<Children>(connect_buttons(state.0, state.1, state.2));
}

/// ssh config 重新解析后重建菜单项
fn rebuild_host_menu(
    session: Res<Session>,
    mut rendered: ResMut<RenderedVersions>,
    slots: Query<Entity, With<HostMenuSlot>>,
    mut commands: Commands,
) {
    if rendered.hosts == Some(session.hosts_version) {
        return;
    }
    let Ok(slot) = slots.single() else {
        return;
    };
    rendered.hosts = Some(session.hosts_version);

    let items: Vec<BoxedScene> = match session.ssh_hosts.is_empty() {
        true => vec![boxed(bsn! {
            Node { padding: {UiRect::all(px(8.0))} }
            Children [(widgets::hint("~/.ssh/config 中没有可用的 Host 条目"))]
        })],
        false => session
            .ssh_hosts
            .iter()
            .enumerate()
            .map(|(index, entry)| boxed(host_menu_item(index, entry)))
            .collect(),
    };

    commands
        .entity(slot)
        .despawn_related::<Children>()
        .queue_spawn_related_scenes::<Children>(items);
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
fn sync_password(
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
    if rendered.form == Some(session.form_version) {
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
                    sync_password,
                    refresh_form_fields,
                )
                    .in_set(UiSet::Rebuild),
            );
    }
}
