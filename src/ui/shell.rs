//! 界面骨架：顶栏 + 侧栏 + 主内容区 + 状态栏
//!
//! 骨架在启动时一次性建好，各面板往预留的插槽里填内容。
//! 布局：
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │ ● 标题      │ [参数 / 流程图] 文件 [编辑操作] │ 顶栏
//! ├─────────────┼────────────────────────────────┤
//! │ 连接卡片    │  元数据 + 字段分组编辑器        │
//! │ 文件列表    │  （可滚动）                     │
//! ├─────────────┴────────────────────────────────┤
//! │ [复制提示] 状态消息                  连接目标 │ 状态栏
//! └──────────────────────────────────────────────┘
//! ```

use bevy::clipboard::{Clipboard, ClipboardError};
use bevy::feathers::controls::{ButtonVariant, FeathersButton};
use bevy::feathers::theme::{
    InheritableThemeTextColor, ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemeToken,
    ThemedText,
};
use bevy::input::mouse::MouseWheel;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{FontWeight, LineBreak};
use bevy::ui::{InteractionDisabled, Pressed, UiScale};
use bevy::ui_widgets::{Activate, ScrollArea};

use super::theme;
use super::widgets;
use super::{Editor, Session, StatusLine, UiSet};

/// 顶栏右侧的远程操作按钮区
#[derive(Component, Default, Clone)]
pub struct ActionBarSlot;

/// 窄窗口把编辑操作放到第二行，视图入口与主题选择仍保持在第一行两端。
#[derive(Component, Clone, Default)]
enum AppBarCell {
    #[default]
    Layout,
    Filename,
    Spacer,
    Actions,
    Theme,
}

/// 侧栏中的连接面板插槽
#[derive(Component, Default, Clone)]
pub struct ConnectSlot;

/// 侧栏中的文件列表插槽
#[derive(Component, Default, Clone)]
pub struct FileListSlot;

/// 主内容区的编辑器插槽
#[derive(Component, Default, Clone)]
pub struct EditorSlot;

/// 参数编辑视图的外层容器（与流程图视图互斥显示）
#[derive(Component, Default, Clone)]
pub struct ParamsPane;

/// 流程图视图的外层容器
#[derive(Component, Default, Clone)]
pub struct GraphPane;

/// 流程图内容插槽
#[derive(Component, Default, Clone)]
pub struct GraphSlot;

/// 右侧功能区顶栏起始处的独立视图切换按钮组
#[derive(Component, Default, Clone)]
pub struct ViewSwitchSlot;

/// 顶栏的连接状态指示灯
#[derive(Component, Default, Clone)]
pub struct ConnectionDot;

/// 顶栏显示的当前文件名
#[derive(Component, Default, Clone)]
pub struct CurrentFileText;

/// 状态栏的状态文字
#[derive(Component, Default, Clone)]
pub struct StatusText;

/// 保存当前显示的完整提示，复制反馈只影响按钮，不覆盖原提示。
#[derive(Component, Default, Clone)]
struct StatusCopyButton {
    text: String,
    feedback: CopyFeedback,
}

#[derive(Default, Clone)]
enum CopyFeedback {
    #[default]
    Ready,
    Copied,
    Failed,
}

impl CopyFeedback {
    fn label(&self) -> &'static str {
        match self {
            Self::Ready => "复制提示",
            Self::Copied => "已复制",
            Self::Failed => "复制失败",
        }
    }
}

impl StatusCopyButton {
    fn copy_with(&mut self, write: impl FnOnce(&str) -> Result<(), ClipboardError>) {
        if self.text.is_empty() {
            return;
        }
        self.feedback = match write(&self.text) {
            Ok(()) => CopyFeedback::Copied,
            Err(error) => {
                warn!(%error, "复制状态提示失败");
                CopyFeedback::Failed
            }
        };
    }
}

#[derive(Component, Default, Clone)]
struct StatusCopyLabel;

/// 状态栏右侧的连接目标
#[derive(Component, Default, Clone)]
pub struct ConnectionTargetText;

/// 右键菜单浮层的根节点
#[derive(Component, Default, Clone)]
pub struct ContextMenuRoot;

/// UI 缩放上下限
const SCALE_RANGE: (f32, f32) = (0.5, 3.0);

/// 键盘每次缩放的步长
const SCALE_STEP_KEY: f32 = 0.1;

/// 滚轮每格缩放的步长
const SCALE_STEP_WHEEL: f32 = 0.05;

/// 构建骨架场景
fn shell() -> impl SceneList {
    bsn_list![Camera2d, root()]
}

/// 根节点
fn root() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
        }
        ThemeBackgroundColor({theme::CONTENT_BG})
        Children [
            app_bar(),
            body(),
            status_bar(),
            context_menu_layer(),
        ]
    }
}

/// 右键菜单浮层
///
/// 常驻但默认隐藏，位置和内容由 [`super::files`] 在右键时设置。
/// 放在根节点的最后一个子节点，保证 z 序压在其余界面之上。
fn context_menu_layer() -> impl Scene {
    bsn! {
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            flex_direction: FlexDirection::Column,
            min_width: px(150),
            padding: px(4),
            border: {UiRect::all(px(1.0))},
            border_radius: {BorderRadius::all(px(theme::RADIUS_SM))},
        }
        ContextMenuRoot
        ThemeBackgroundColor({bevy::feathers::tokens::MENU_BG})
        ThemeBorderColor({bevy::feathers::tokens::MENU_BORDER})
    }
}

/// 顶栏与主体共用侧栏宽度，左侧是选择区标题，右侧是文档功能。
fn app_bar() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            min_height: {px(theme::APPBAR_HEIGHT)},
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            border: {UiRect::bottom(px(1.0))},
        }
        ThemeBackgroundColor({theme::APPBAR_BG})
        ThemeBorderColor({theme::DIVIDER})
        Children [
            (
                Node {
                    width: {px(theme::SIDEBAR_WIDTH)},
                    flex_shrink: 0.0,
                    align_items: AlignItems::Center,
                    column_gap: px(10),
                    padding: {UiRect::horizontal(px(theme::PAD + 2.0))},
                }
                Children [
                    (widgets::status_dot(theme::DOT_DISCONNECTED) ConnectionDot),
                    (
                        Text("任务图编辑器")
                        ThemeTextColor({theme::SECTION_TEXT})
                        TextFont {
                            font_size: px(15.0),
                            weight: {FontWeight::BOLD}
                        }
                    ),
                ]
            ),
            (
                Node {
                    flex_grow: 1.0,
                    min_width: px(0),
                    display: Display::Grid,
                    align_items: AlignItems::Center,
                    column_gap: px(10),
                    row_gap: px(6),
                    padding: {UiRect::axes(px(theme::PAD + 2.0), px(theme::PAD_SM))},
                }
                template_value(AppBarCell::Layout)
                Children [
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::FlexStart,
                            align_items: AlignItems::Center,
                            flex_shrink: 0.0,
                            column_gap: px(4),
                            grid_row: {GridPlacement::start(1)},
                            grid_column: {GridPlacement::start(1)},
                        }
                        ViewSwitchSlot
                    ),
                    (
                        Node {
                            min_width: px(0),
                            overflow: {Overflow::clip()},
                            grid_row: {GridPlacement::start(1)},
                            grid_column: {GridPlacement::start(2)},
                        }
                        template_value(AppBarCell::Filename)
                        Children [(
                            Text("")
                            TextLayout { linebreak: LineBreak::NoWrap }
                            CurrentFileText
                            ThemeTextColor({theme::READONLY_TEXT})
                            TextFont { font_size: px(12.0) }
                            Node { flex_shrink: 0.0 }
                        )]
                    ),
                    (widgets::spacer() template_value(AppBarCell::Spacer)),
                    (
                        Node {
                            min_width: px(0),
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            justify_content: JustifyContent::FlexEnd,
                            align_items: AlignItems::Center,
                            column_gap: px(6),
                            row_gap: px(6),
                        }
                        ActionBarSlot
                        template_value(AppBarCell::Actions)
                    ),
                    (
                        Node { justify_self: JustifySelf::End }
                        template_value(AppBarCell::Theme)
                        Children [super::theme_picker::theme_picker()]
                    ),
                ]
            ),
        ]
    }
}

fn sync_app_bar_layout(
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    scale: Res<UiScale>,
    mut cells: Query<(&mut Node, Ref<AppBarCell>)>,
    mut previous: Local<Option<bool>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let compact = window.width() / scale.0 < 1200.0;
    for (mut node, cell) in &mut cells {
        if *previous == Some(compact) && !cell.is_added() {
            continue;
        }
        match *cell {
            AppBarCell::Layout => {
                node.display = match compact {
                    true => Display::Grid,
                    false => Display::Flex,
                };
                if compact {
                    node.grid_template_columns = vec![
                        RepeatedGridTrack::auto(1),
                        RepeatedGridTrack::flex(1, 1.0),
                        RepeatedGridTrack::auto(1),
                    ];
                }
            }
            AppBarCell::Filename => {
                node.max_width = match compact {
                    true => Val::Auto,
                    false => percent(20),
                };
            }
            AppBarCell::Spacer => {
                node.display = match compact {
                    true => Display::None,
                    false => Display::Flex,
                };
            }
            AppBarCell::Actions => {
                node.grid_row = GridPlacement::start(if compact { 2 } else { 1 });
                node.grid_column = match compact {
                    true => GridPlacement::start_span(1, 3),
                    false => GridPlacement::start(3),
                };
            }
            AppBarCell::Theme => {
                node.grid_row = GridPlacement::start(1);
                node.grid_column = GridPlacement::start(if compact { 3 } else { 4 });
            }
        }
    }
    *previous = Some(compact);
}

/// 中部主体：侧栏 + 内容区
fn body() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            flex_grow: 1.0,
            flex_direction: FlexDirection::Row,
            min_height: px(0),
        }
        Children [
            sidebar(),
            content(),
        ]
    }
}

/// 左侧栏：连接面板 + 文件列表
fn sidebar() -> impl Scene {
    bsn! {
        Node {
            width: {px(theme::SIDEBAR_WIDTH)},
            flex_shrink: 0.0,
            height: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(10),
            padding: {UiRect::all(px(theme::PAD))},
            border: {UiRect::right(px(1.0))},
            overflow: {Overflow::scroll_y()},
        }
        ScrollArea
        ThemeBackgroundColor({theme::SIDEBAR_BG})
        ThemeBorderColor({theme::DIVIDER})
        Children [
            (
                Node {
                    width: percent(100),
                    flex_direction: FlexDirection::Column,
                }
                ConnectSlot
            ),
            (
                Node {
                    width: percent(100),
                    flex_direction: FlexDirection::Column,
                }
                FileListSlot
            )
        ]
    }
}

/// 右侧主内容区
fn content() -> impl Scene {
    bsn! {
        Node {
            flex_grow: 1.0,
            height: percent(100),
            flex_direction: FlexDirection::Column,
            min_width: px(0),
        }
        ThemeBackgroundColor({theme::CONTENT_BG})
        Children [
            (
                Node {
                    width: percent(100),
                    height: percent(100),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(10),
                    padding: {UiRect::all(px(theme::PAD + 2.0))},
                    overflow: {Overflow::scroll_y()},
                }
                ScrollArea
                ParamsPane
                Children [(
                    Node {
                        width: percent(100),
                        flex_direction: FlexDirection::Column,
                        row_gap: px(10),
                    }
                    EditorSlot
                )]
            ),
            (
                Node {
                    display: {Display::None},
                    width: percent(100),
                    height: percent(100),
                    flex_direction: FlexDirection::Column,
                    min_height: px(0),
                }
                GraphPane
                GraphSlot
            )
        ]
    }
}

/// 底部状态栏
fn status_bar() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            height: {px(theme::STATUSBAR_HEIGHT)},
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(8),
            padding: {UiRect::horizontal(px(theme::PAD + 2.0))},
            border: {UiRect::top(px(1.0))},
        }
        ThemeBackgroundColor({theme::STATUSBAR_BG})
        ThemeBorderColor({theme::DIVIDER})
        Children [
            (
                @FeathersButton {
                    @variant: ButtonVariant::Plain,
                    @caption: {bsn! {
                        Text("复制提示")
                        StatusCopyLabel
                        ThemedText
                        TextFont { font_size: px(12.0) }
                    }}
                }
                Node { height: px(24), width: px(78), flex_shrink: 0.0 }
                StatusCopyButton
                InteractionDisabled
                AccessibleLabel("复制完整状态提示")
                on(on_copy_status)
            ),
            (
                Node {
                    flex_grow: 1.0,
                    flex_basis: px(0),
                    min_width: px(0),
                    height: percent(100),
                    align_items: AlignItems::Center,
                    overflow: {Overflow::clip()},
                }
                Children [(
                    Text("")
                    TextLayout { linebreak: LineBreak::NoWrap }
                    StatusText
                    ThemeTextColor({theme::STATUS_OK})
                    TextFont { font_size: px(12.0) }
                    Node { flex_shrink: 0.0 }
                )]
            ),
            (
                Node { max_width: percent(30), min_width: px(0), overflow: {Overflow::clip()} }
                Children [(
                    Text("")
                    TextLayout { linebreak: LineBreak::NoWrap }
                    ConnectionTargetText
                    ThemeTextColor({theme::READONLY_TEXT})
                    TextFont { font_size: px(12.0) }
                    Node { flex_shrink: 0.0 }
                )]
            )
        ]
    }
}

/// 从当前已经显示的提示复制，长文本的视觉裁剪不影响剪贴板内容。
fn on_copy_status(
    event: On<Activate>,
    mut buttons: Query<&mut StatusCopyButton>,
    mut clipboard: ResMut<Clipboard>,
) {
    if let Ok(mut button) = buttons.get_mut(event.entity) {
        button.copy_with(|text| clipboard.set_text(text));
    }
}

type CopyButtonStyles<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Hovered,
        Has<Pressed>,
        Has<InteractionDisabled>,
        &'static ThemeBackgroundColor,
        &'static InheritableThemeTextColor,
    ),
    With<StatusCopyButton>,
>;

/// 复制入口使用独立主题色；保留 Feathers 的键鼠与焦点行为，仅接管颜色样式。
fn sync_copy_button_style(
    new_buttons: Query<Entity, (With<StatusCopyButton>, With<ButtonVariant>)>,
    buttons: CopyButtonStyles,
    mut commands: Commands,
) {
    // 去掉通用样式分发标记，避免鼠标悬停时被普通按钮底色覆盖。
    for entity in &new_buttons {
        commands.entity(entity).remove::<ButtonVariant>();
    }
    for (entity, hovered, pressed, disabled, background, text) in &buttons {
        let (bg, fg) = match (disabled, pressed || hovered.0) {
            (true, _) => (
                bevy::feathers::tokens::BUTTON_BG_DISABLED,
                bevy::feathers::tokens::BUTTON_TEXT_DISABLED,
            ),
            (false, true) => (theme::COPY_BG_HOVER, theme::COPY_TEXT),
            (false, false) => (theme::COPY_BG, theme::COPY_TEXT),
        };
        if background.0 != bg {
            commands.entity(entity).insert(ThemeBackgroundColor(bg));
        }
        if text.0 != fg {
            commands
                .entity(entity)
                .insert(InheritableThemeTextColor(fg));
        }
    }
}

/// 同步复制源、空提示禁用态及独立反馈，也覆盖跨帧生成的按钮。
fn sync_status_copy(
    messages: Query<&Text, With<StatusText>>,
    mut buttons: Query<(Entity, &mut StatusCopyButton, Has<InteractionDisabled>)>,
    mut labels: Query<&mut Text, (With<StatusCopyLabel>, Without<StatusText>)>,
    mut commands: Commands,
) {
    let Ok(message) = messages.single() else {
        return;
    };
    for (entity, mut button, disabled) in &mut buttons {
        if button.text != message.0 {
            button.text.clone_from(&message.0);
            button.feedback = CopyFeedback::Ready;
        }
        match (button.text.is_empty(), disabled) {
            (true, false) => {
                commands.entity(entity).insert(InteractionDisabled);
            }
            (false, true) => {
                commands.entity(entity).remove::<InteractionDisabled>();
            }
            _ => {}
        }
        for mut label in &mut labels {
            let expected = button.feedback.label();
            if label.0 != expected {
                label.0 = expected.into();
            }
        }
    }
}

/// 状态栏文字与配色跟随状态资源
///
/// 左侧永远只显示一条：重连 > 忙碌 > 最近一条结果。三者本来说的就是同一件事的
/// 不同阶段，各占一边会变成"左下角和右下角都在说正在加载"。
/// 右侧改放连接目标，忙时也能一眼看出连的是哪台机器。
fn sync_status_bar(
    status: Res<StatusLine>,
    session: Res<Session>,
    texts: Query<(Entity, Ref<StatusText>)>,
    targets: Query<(Entity, Ref<ConnectionTargetText>)>,
    dots: Query<(Entity, Ref<ConnectionDot>)>,
    mut all_text: Query<&mut Text>,
    mut commands: Commands,
) {
    if !status.is_changed()
        && !session.is_changed()
        && !texts.iter().any(|(_, marker)| marker.is_added())
        && !targets.iter().any(|(_, marker)| marker.is_added())
        && !dots.iter().any(|(_, marker)| marker.is_added())
    {
        return;
    }

    let (message, token) = match (&session.reconnect_status, session.is_busy()) {
        (Some(reconnect), _) => (reconnect.clone(), theme::STATUS_WARN),
        (None, true) => (session.busy_text(), theme::STATUS_WARN),
        (None, false) => (status.text.clone(), theme::status_token(status.level())),
    };
    for (entity, _) in &texts {
        if let Ok(mut text) = all_text.get_mut(entity) {
            text.0.clone_from(&message);
        }
        // ThemeTextColor 是不可变组件，改色要整体替换
        commands
            .entity(entity)
            .insert(ThemeTextColor(token.clone()));
    }

    let target = match session.is_connected {
        true => session
            .target
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        false => String::new(),
    };
    for (entity, _) in &targets {
        if let Ok(mut text) = all_text.get_mut(entity) {
            text.0.clone_from(&target);
        }
    }

    let dot_token = connection_dot_token(&session);
    for (entity, _) in &dots {
        commands
            .entity(entity)
            .insert(ThemeBackgroundColor(dot_token.clone()));
    }
}

/// 按会话状态选连接指示灯颜色
fn connection_dot_token(session: &Session) -> ThemeToken {
    match (session.is_connected, session.reconnect_status.is_some()) {
        (true, _) => theme::DOT_CONNECTED,
        (false, true) => theme::DOT_RECONNECTING,
        (false, false) => theme::DOT_DISCONNECTED,
    }
}

/// 顶栏显示当前打开的文件
fn sync_current_file(
    editor: Res<Editor>,
    editing: Option<Res<super::graph_edit::GraphEditing>>,
    labels: Query<Entity, With<CurrentFileText>>,
    mut all_text: Query<&mut Text>,
) {
    let dirty = editor.has_unsaved_changes()
        || editing
            .as_ref()
            .is_some_and(|editing| editing.has_draft_changes());
    let label = match (&editor.document, editor.data.is_some()) {
        (Some(document), true) => format!(
            "— {}{}",
            document.filename,
            if dirty { " · 未保存" } else { "" }
        ),
        (None, true) => match dirty {
            true => "— 本地文档 · 未保存".into(),
            false => "— 本地文档".into(),
        },
        _ => String::new(),
    };
    for entity in &labels {
        if let Ok(mut text) = all_text.get_mut(entity)
            && text.0 != label
        {
            text.0.clone_from(&label);
        }
    }
}

/// UI 缩放：Shift + `+`/`-` 或 Shift + 滚轮
fn handle_ui_scale(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    mut ui_scale: ResMut<UiScale>,
) {
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if !shift {
        // 不按 Shift 时滚轮属于滚动区域，这里必须把消息读掉之外的处理留给 ScrollArea
        wheel.clear();
        return;
    }

    let mut delta = 0.0;
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) {
        delta += SCALE_STEP_KEY;
    }
    if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        delta -= SCALE_STEP_KEY;
    }
    for event in wheel.read() {
        let scroll = match event.x.abs() > event.y.abs() {
            true => event.x,
            false => event.y,
        };
        delta += scroll * SCALE_STEP_WHEEL;
    }

    if delta.abs() > f32::EPSILON {
        ui_scale.0 = (ui_scale.0 + delta).clamp(SCALE_RANGE.0, SCALE_RANGE.1);
    }
}

/// 骨架插件
pub struct ShellPlugin;

impl Plugin for ShellPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, shell.spawn()).add_systems(
            Update,
            (
                handle_ui_scale.in_set(UiSet::Input),
                (
                    sync_status_bar,
                    sync_status_copy.after(sync_status_bar),
                    sync_copy_button_style.after(sync_status_copy),
                    sync_current_file,
                    sync_app_bar_layout,
                )
                    .in_set(UiSet::Rebuild),
            ),
        );
    }
}

#[cfg(test)]
mod tests;
