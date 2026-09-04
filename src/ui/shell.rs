//! 界面骨架：顶栏 + 侧栏 + 主内容区 + 状态栏
//!
//! 骨架在启动时一次性建好，各面板往预留的插槽里填内容。
//! 布局：
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │ ● 任务图编辑器   当前文件      [远程操作按钮] │ 顶栏
//! ├─────────────┬────────────────────────────────┤
//! │ 连接卡片    │  元数据 + 字段分组编辑器        │
//! │ 文件列表    │  （可滚动）                     │
//! ├─────────────┴────────────────────────────────┤
//! │ ● 状态消息                      忙碌/重连提示 │ 状态栏
//! └──────────────────────────────────────────────┘
//! ```

use bevy::feathers::theme::{ThemeBackgroundColor, ThemeTextColor, ThemeToken};
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::text::FontWeight;
use bevy::ui::UiScale;
use bevy::ui_widgets::ScrollArea;

use super::theme;
use super::widgets;
use super::{Editor, FileBrowser, Session, StatusLine, UiSet};

/// 顶栏右侧的远程操作按钮区
#[derive(Component, Default, Clone)]
pub struct ActionBarSlot;

/// 侧栏中的连接面板插槽
#[derive(Component, Default, Clone)]
pub struct ConnectSlot;

/// 侧栏中的文件列表插槽
#[derive(Component, Default, Clone)]
pub struct FileListSlot;

/// 主内容区的编辑器插槽
#[derive(Component, Default, Clone)]
pub struct EditorSlot;

/// 顶栏的连接状态指示灯
#[derive(Component, Default, Clone)]
pub struct ConnectionDot;

/// 顶栏显示的当前文件名
#[derive(Component, Default, Clone)]
pub struct CurrentFileText;

/// 状态栏的状态文字
#[derive(Component, Default, Clone)]
pub struct StatusText;

/// 状态栏右侧的忙碌 / 重连提示
#[derive(Component, Default, Clone)]
pub struct BusyText;

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
        BorderColor::all(Color::NONE)
    }
}

/// 顶栏
fn app_bar() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            height: {px(theme::APPBAR_HEIGHT)},
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(10),
            padding: {UiRect::horizontal(px(theme::PAD + 2.0))},
            border: {UiRect::bottom(px(1.0))},
        }
        ThemeBackgroundColor({theme::APPBAR_BG})
        BorderColor::all(Color::NONE)
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
            (
                Node { margin: {UiRect::left(px(4.0))} }
                Text("")
                CurrentFileText
                ThemeTextColor({theme::READONLY_TEXT})
                TextFont { font_size: px(12.0) }
            ),
            widgets::spacer(),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(6),
                }
                ActionBarSlot
            )
        ]
    }
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
        BorderColor::all(Color::NONE)
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
            row_gap: px(10),
            padding: {UiRect::all(px(theme::PAD + 2.0))},
            overflow: {Overflow::scroll_y()},
        }
        ScrollArea
        ThemeBackgroundColor({theme::CONTENT_BG})
        Children [(
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(10),
            }
            EditorSlot
        )]
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
        BorderColor::all(Color::NONE)
        Children [
            (
                Text("")
                StatusText
                ThemeTextColor({theme::STATUS_OK})
                TextFont { font_size: px(12.0) }
            ),
            widgets::spacer(),
            (
                Text("")
                BusyText
                ThemeTextColor({theme::STATUS_WARN})
                TextFont { font_size: px(12.0) }
            )
        ]
    }
}

/// 状态栏文字与配色跟随状态资源
fn sync_status_bar(
    status: Res<StatusLine>,
    session: Res<Session>,
    texts: Query<Entity, With<StatusText>>,
    busy_texts: Query<Entity, With<BusyText>>,
    dots: Query<Entity, With<ConnectionDot>>,
    mut all_text: Query<&mut Text>,
    mut commands: Commands,
) {
    if !status.is_changed() && !session.is_changed() {
        return;
    }

    // 状态消息：文字 + 语义色
    let token = theme::status_token(status.level());
    for entity in &texts {
        if let Ok(mut text) = all_text.get_mut(entity) {
            text.0.clone_from(&status.text);
        }
        // ThemeTextColor 是不可变组件，改色要整体替换
        commands
            .entity(entity)
            .insert(ThemeTextColor(token.clone()));
    }

    // 忙碌 / 重连提示：重连优先显示
    let busy_label = match (&session.reconnect_status, session.is_busy()) {
        (Some(reconnect), _) => reconnect.clone(),
        (None, true) => session.busy_text(),
        (None, false) => String::new(),
    };
    for entity in &busy_texts {
        if let Ok(mut text) = all_text.get_mut(entity) {
            text.0.clone_from(&busy_label);
        }
    }

    // 连接指示灯
    let dot_token = connection_dot_token(&session);
    for entity in &dots {
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
    browser: Res<FileBrowser>,
    editor: Res<Editor>,
    labels: Query<Entity, With<CurrentFileText>>,
    mut all_text: Query<&mut Text>,
) {
    if !browser.is_changed() && !editor.is_changed() {
        return;
    }
    let label = match (&browser.selected, editor.data.is_some()) {
        (Some(name), true) => format!("— {name}"),
        _ => String::new(),
    };
    for entity in &labels {
        if let Ok(mut text) = all_text.get_mut(entity) {
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
            (sync_status_bar, sync_current_file, handle_ui_scale).in_set(UiSet::Rebuild),
        );
    }
}
