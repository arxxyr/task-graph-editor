//! 应用主题：在 Feathers 深色主题基础上定制的现代化配色
//!
//! 配色用 OKLCH 定义（感知均匀色彩空间），保证同一亮度档的不同色相看起来
//! 一样亮，不会出现"蓝色显得比黄色暗"的问题。
//!
//! 表面按层级递增亮度：窗口底 → 面板 → 分组 → 控件，形成清晰的层次感；
//! 强调色用青蓝，语义色（成功/警告/危险）保持足够的色相间距以便快速区分。

use bevy::feathers::dark_theme::create_dark_theme;
use bevy::feathers::theme::{ThemeToken, UiTheme};
use bevy::feathers::tokens;
use bevy::prelude::*;

// ============================================================
// 基础色板（OKLCH：亮度 0..1，彩度 0..0.4，色相角 0..360）
// ============================================================

/// 窗口最底层背景（近黑的冷灰）
const SURFACE_0: Color = Color::oklch(0.185, 0.012, 265.0);
/// 侧栏 / 面板背景
const SURFACE_1: Color = Color::oklch(0.225, 0.014, 265.0);
/// 面板内容区背景
const SURFACE_2: Color = Color::oklch(0.262, 0.016, 265.0);
/// 分组卡片背景
const SURFACE_3: Color = Color::oklch(0.302, 0.018, 265.0);
/// 控件默认背景
const SURFACE_4: Color = Color::oklch(0.352, 0.020, 265.0);
/// 控件悬停背景
const SURFACE_5: Color = Color::oklch(0.402, 0.022, 265.0);

/// 分隔线 / 边框
const BORDER: Color = Color::oklch(0.335, 0.016, 265.0);
/// 强调边框（分组卡片描边）
const BORDER_STRONG: Color = Color::oklch(0.405, 0.020, 265.0);

/// 主文本
const TEXT: Color = Color::oklch(0.925, 0.005, 265.0);
/// 次级文本
const TEXT_DIM: Color = Color::oklch(0.720, 0.008, 265.0);
/// 弱化文本（提示、占位）
const TEXT_FAINT: Color = Color::oklch(0.580, 0.010, 265.0);

/// 强调色：青蓝，用于主按钮、焦点环、选中态
const ACCENT: Color = Color::oklch(0.660, 0.132, 232.0);
/// 强调色（悬停）
const ACCENT_HOVER: Color = Color::oklch(0.710, 0.138, 232.0);
/// 强调色（按下）
const ACCENT_PRESSED: Color = Color::oklch(0.755, 0.142, 232.0);

/// 成功 / 正常：青绿
const SUCCESS: Color = Color::oklch(0.760, 0.140, 158.0);
/// 警告 / 注意：琥珀
const WARNING: Color = Color::oklch(0.800, 0.145, 78.0);
/// 危险 / 失败：珊瑚红
const DANGER: Color = Color::oklch(0.660, 0.180, 22.0);
/// 危险（悬停）
const DANGER_HOVER: Color = Color::oklch(0.705, 0.185, 22.0);
/// 危险（按下）
const DANGER_PRESSED: Color = Color::oklch(0.745, 0.190, 22.0);

/// 位姿选中高亮：琥珀，与强调色的青蓝拉开色相距离
const HIGHLIGHT: Color = Color::oklch(0.800, 0.145, 78.0);

/// 禁用态背景
const DISABLED_BG: Color = Color::oklch(0.282, 0.010, 265.0);
/// 禁用态文本
const DISABLED_TEXT: Color = Color::oklch(0.500, 0.006, 265.0);

// ============================================================
// 应用自定义 token
// ============================================================

/// 状态栏：正常
pub const STATUS_OK: ThemeToken = ThemeToken::new_static("app.status.ok");
/// 状态栏：警告
pub const STATUS_WARN: ThemeToken = ThemeToken::new_static("app.status.warn");
/// 状态栏：错误
pub const STATUS_ERROR: ThemeToken = ThemeToken::new_static("app.status.error");

/// 连接状态指示灯：已连接
pub const DOT_CONNECTED: ThemeToken = ThemeToken::new_static("app.dot.connected");
/// 连接状态指示灯：未连接
pub const DOT_DISCONNECTED: ThemeToken = ThemeToken::new_static("app.dot.disconnected");
/// 连接状态指示灯：重连中
pub const DOT_RECONNECTING: ThemeToken = ThemeToken::new_static("app.dot.reconnecting");

/// 顶栏背景
pub const APPBAR_BG: ThemeToken = ThemeToken::new_static("app.appbar.bg");
/// 侧栏背景
pub const SIDEBAR_BG: ThemeToken = ThemeToken::new_static("app.sidebar.bg");
/// 主内容区背景
pub const CONTENT_BG: ThemeToken = ThemeToken::new_static("app.content.bg");
/// 状态栏背景
pub const STATUSBAR_BG: ThemeToken = ThemeToken::new_static("app.statusbar.bg");
/// 通用分隔线
pub const DIVIDER: ThemeToken = ThemeToken::new_static("app.divider");

/// 卡片背景
pub const CARD_BG: ThemeToken = ThemeToken::new_static("app.card.bg");
/// 卡片边框
pub const CARD_BORDER: ThemeToken = ThemeToken::new_static("app.card.border");
/// 卡片标题背景
pub const CARD_HEADER_BG: ThemeToken = ThemeToken::new_static("app.card.header.bg");

/// 位姿卡片：选中背景
pub const POSE_SELECTED_BG: ThemeToken = ThemeToken::new_static("app.pose.selected.bg");
/// 位姿卡片：选中边框
pub const POSE_SELECTED_BORDER: ThemeToken = ThemeToken::new_static("app.pose.selected.border");
/// 位姿卡片：选中标题文字
pub const POSE_SELECTED_TEXT: ThemeToken = ThemeToken::new_static("app.pose.selected.text");

/// 危险按钮背景
pub const DANGER_BG: ThemeToken = ThemeToken::new_static("app.danger.bg");
/// 危险按钮背景（悬停）
pub const DANGER_BG_HOVER: ThemeToken = ThemeToken::new_static("app.danger.bg.hover");
/// 危险按钮背景（按下）
pub const DANGER_BG_PRESSED: ThemeToken = ThemeToken::new_static("app.danger.bg.pressed");
/// 危险按钮文字
pub const DANGER_TEXT: ThemeToken = ThemeToken::new_static("app.danger.text");

/// 计数徽章背景
pub const BADGE_BG: ThemeToken = ThemeToken::new_static("app.badge.bg");
/// 计数徽章文字
pub const BADGE_TEXT: ThemeToken = ThemeToken::new_static("app.badge.text");

/// 分组标题文字
pub const SECTION_TEXT: ThemeToken = ThemeToken::new_static("app.section.text");
/// 字段名文字
pub const FIELD_LABEL: ThemeToken = ThemeToken::new_static("app.field.label");
/// 只读值文字
pub const READONLY_TEXT: ThemeToken = ThemeToken::new_static("app.readonly.text");

// ============================================================
// 尺寸常量
// ============================================================

/// 顶栏高度
pub const APPBAR_HEIGHT: f32 = 46.0;
/// 状态栏高度
pub const STATUSBAR_HEIGHT: f32 = 28.0;
/// 侧栏宽度
pub const SIDEBAR_WIDTH: f32 = 320.0;
/// 标准内边距
pub const PAD: f32 = 10.0;
/// 紧凑内边距
pub const PAD_SM: f32 = 6.0;
/// 卡片圆角
pub const RADIUS: f32 = 8.0;
/// 小圆角
pub const RADIUS_SM: f32 = 5.0;
/// 每层嵌套的缩进量
pub const INDENT: f32 = 12.0;

/// 写入一条 token → 颜色映射
///
/// `UiTheme::set_color` 只接受 `&str`，而这里的 token 都是 `ThemeToken` 常量，
/// 故直接写 `ThemeProps` 的公开映射表，避免多余的字符串转换。
fn set(theme: &mut UiTheme, token: ThemeToken, color: Color) {
    theme.0.color.insert(token, color);
}

/// 构建应用主题：以 Feathers 深色主题为底，覆盖为本应用配色
pub fn create_app_theme() -> UiTheme {
    let mut theme = UiTheme(create_dark_theme());

    // ── 全局 ──
    set(&mut theme, tokens::WINDOW_BG, SURFACE_0);
    set(&mut theme, tokens::TEXT_MAIN, TEXT);
    set(&mut theme, tokens::TEXT_DIM, TEXT_DIM);
    set(&mut theme, tokens::FOCUS_RING, ACCENT.with_alpha(0.62));

    // ── 普通按钮 ──
    set(&mut theme, tokens::BUTTON_BG, SURFACE_4);
    set(&mut theme, tokens::BUTTON_BG_HOVER, SURFACE_5);
    set(&mut theme, tokens::BUTTON_BG_PRESSED, ACCENT);
    set(&mut theme, tokens::BUTTON_BG_DISABLED, DISABLED_BG);
    set(&mut theme, tokens::BUTTON_TEXT, TEXT);
    set(&mut theme, tokens::BUTTON_TEXT_DISABLED, DISABLED_TEXT);

    // ── 主按钮 ──
    set(&mut theme, tokens::BUTTON_PRIMARY_BG, ACCENT);
    set(&mut theme, tokens::BUTTON_PRIMARY_BG_HOVER, ACCENT_HOVER);
    set(
        &mut theme,
        tokens::BUTTON_PRIMARY_BG_PRESSED,
        ACCENT_PRESSED,
    );
    set(&mut theme, tokens::BUTTON_PRIMARY_BG_DISABLED, DISABLED_BG);
    set(&mut theme, tokens::BUTTON_PRIMARY_TEXT, SURFACE_0);
    set(
        &mut theme,
        tokens::BUTTON_PRIMARY_TEXT_DISABLED,
        DISABLED_TEXT,
    );

    // ── 无底色按钮（工具栏图标等） ──
    set(&mut theme, tokens::BUTTON_PLAIN_BG, Color::NONE);
    set(&mut theme, tokens::BUTTON_PLAIN_BG_HOVER, SURFACE_3);
    set(&mut theme, tokens::BUTTON_PLAIN_BG_PRESSED, SURFACE_4);
    set(&mut theme, tokens::BUTTON_PLAIN_BG_DISABLED, Color::NONE);

    // ── 文本输入 ──
    set(&mut theme, tokens::TEXT_INPUT_BG, SURFACE_1);
    set(&mut theme, tokens::TEXT_INPUT_TEXT, TEXT);
    set(&mut theme, tokens::TEXT_INPUT_TEXT_DISABLED, DISABLED_TEXT);
    set(&mut theme, tokens::TEXT_INPUT_CURSOR, ACCENT);
    set(
        &mut theme,
        tokens::TEXT_INPUT_SELECTION,
        ACCENT.with_alpha(0.35),
    );
    set(
        &mut theme,
        tokens::TEXT_INPUT_SELECTION_UNFOCUSED,
        SURFACE_4.with_alpha(0.5),
    );
    set(&mut theme, tokens::TEXT_INPUT_LABEL_BG, SURFACE_3);
    // 位姿三轴沿用行业惯例：X 红 / Y 绿 / Z 蓝
    set(
        &mut theme,
        tokens::TEXT_INPUT_X_AXIS,
        Color::oklch(0.640, 0.170, 22.0),
    );
    set(
        &mut theme,
        tokens::TEXT_INPUT_Y_AXIS,
        Color::oklch(0.720, 0.160, 145.0),
    );
    set(
        &mut theme,
        tokens::TEXT_INPUT_Z_AXIS,
        Color::oklch(0.640, 0.140, 252.0),
    );

    // ── 复选框 ──
    set(&mut theme, tokens::CHECKBOX_BG, SURFACE_1);
    set(&mut theme, tokens::CHECKBOX_BG_HOVER, SURFACE_2);
    set(&mut theme, tokens::CHECKBOX_BG_PRESSED, SURFACE_3);
    set(&mut theme, tokens::CHECKBOX_BG_DISABLED, DISABLED_BG);
    set(&mut theme, tokens::CHECKBOX_BG_CHECKED, ACCENT);
    set(&mut theme, tokens::CHECKBOX_BG_CHECKED_HOVER, ACCENT_HOVER);
    set(
        &mut theme,
        tokens::CHECKBOX_BG_CHECKED_PRESSED,
        ACCENT_PRESSED,
    );
    set(
        &mut theme,
        tokens::CHECKBOX_BG_CHECKED_DISABLED,
        DISABLED_BG,
    );
    set(&mut theme, tokens::CHECKBOX_BORDER, BORDER_STRONG);
    set(&mut theme, tokens::CHECKBOX_BORDER_HOVER, ACCENT);
    set(&mut theme, tokens::CHECKBOX_BORDER_PRESSED, ACCENT_PRESSED);
    set(&mut theme, tokens::CHECKBOX_BORDER_DISABLED, BORDER);
    set(&mut theme, tokens::CHECKBOX_BORDER_CHECKED, ACCENT);
    set(
        &mut theme,
        tokens::CHECKBOX_BORDER_CHECKED_HOVER,
        ACCENT_HOVER,
    );
    set(
        &mut theme,
        tokens::CHECKBOX_BORDER_CHECKED_PRESSED,
        ACCENT_PRESSED,
    );
    set(&mut theme, tokens::CHECKBOX_BORDER_CHECKED_DISABLED, BORDER);
    set(&mut theme, tokens::CHECKBOX_MARK, SURFACE_0);
    set(&mut theme, tokens::CHECKBOX_MARK_DISABLED, DISABLED_TEXT);
    set(&mut theme, tokens::CHECKBOX_TEXT, TEXT);
    set(&mut theme, tokens::CHECKBOX_TEXT_DISABLED, DISABLED_TEXT);

    // ── 滚动条 ──
    set(&mut theme, tokens::SCROLLBAR_BG, Color::NONE);
    set(&mut theme, tokens::SCROLLBAR_THUMB, SURFACE_4);
    set(&mut theme, tokens::SCROLLBAR_THUMB_HOVER, SURFACE_5);

    // ── 菜单（含文件右键菜单） ──
    set(&mut theme, tokens::MENU_BG, SURFACE_2);
    set(&mut theme, tokens::MENU_BORDER, BORDER_STRONG);
    set(&mut theme, tokens::MENUITEM_BG_HOVER, SURFACE_4);
    set(&mut theme, tokens::MENUITEM_BG_PRESSED, ACCENT);
    set(&mut theme, tokens::MENUITEM_BG_FOCUSED, SURFACE_4);
    set(&mut theme, tokens::MENUITEM_TEXT, TEXT);
    set(&mut theme, tokens::MENUITEM_TEXT_DISABLED, DISABLED_TEXT);

    // ── 面板 / 分组容器 ──
    set(&mut theme, tokens::PANE_HEADER_BG, SURFACE_2);
    set(&mut theme, tokens::PANE_HEADER_BORDER, BORDER);
    set(&mut theme, tokens::PANE_HEADER_TEXT, TEXT);
    set(&mut theme, tokens::PANE_HEADER_DIVIDER, BORDER);
    set(&mut theme, tokens::PANE_BODY_BG, SURFACE_1);
    set(&mut theme, tokens::SUBPANE_HEADER_BG, SURFACE_2);
    set(&mut theme, tokens::SUBPANE_HEADER_BORDER, BORDER);
    set(&mut theme, tokens::SUBPANE_HEADER_TEXT, TEXT_DIM);
    set(&mut theme, tokens::SUBPANE_BODY_BG, SURFACE_2);
    set(&mut theme, tokens::SUBPANE_BODY_BORDER, BORDER);
    set(&mut theme, tokens::GROUP_HEADER_BG, SURFACE_3);
    set(&mut theme, tokens::GROUP_HEADER_BORDER, BORDER_STRONG);
    set(&mut theme, tokens::GROUP_HEADER_TEXT, TEXT);
    set(&mut theme, tokens::GROUP_BODY_BG, SURFACE_2);
    set(&mut theme, tokens::GROUP_BODY_BORDER, BORDER);

    // ── 列表行（文件列表） ──
    set(&mut theme, tokens::LISTROW_BG, Color::NONE);
    set(&mut theme, tokens::LISTROW_BG_HOVER, SURFACE_3);
    set(
        &mut theme,
        tokens::LISTROW_BG_SELECTED,
        ACCENT.with_alpha(0.30),
    );
    set(&mut theme, tokens::LISTROW_TEXT, TEXT);
    set(&mut theme, tokens::LISTROW_TEXT_DISABLED, DISABLED_TEXT);

    // ── 滑块（数值输入内部使用） ──
    set(&mut theme, tokens::SLIDER_BG, SURFACE_1);
    set(&mut theme, tokens::SLIDER_BG_HOVER, SURFACE_2);
    set(&mut theme, tokens::SLIDER_BG_PRESSED, SURFACE_2);
    set(&mut theme, tokens::SLIDER_BG_DISABLED, DISABLED_BG);
    set(&mut theme, tokens::SLIDER_BAR, ACCENT.with_alpha(0.7));
    set(&mut theme, tokens::SLIDER_BAR_HOVER, ACCENT);
    set(&mut theme, tokens::SLIDER_BAR_PRESSED, ACCENT_PRESSED);
    set(&mut theme, tokens::SLIDER_BAR_DISABLED, DISABLED_BG);
    set(&mut theme, tokens::SLIDER_TEXT, TEXT);
    set(&mut theme, tokens::SLIDER_TEXT_DISABLED, DISABLED_TEXT);

    // ── 开关 ──
    set(&mut theme, tokens::SWITCH_BG, SURFACE_1);
    set(&mut theme, tokens::SWITCH_BG_HOVER, SURFACE_2);
    set(&mut theme, tokens::SWITCH_BG_CHECKED, ACCENT);
    set(&mut theme, tokens::SWITCH_BG_CHECKED_HOVER, ACCENT_HOVER);
    set(&mut theme, tokens::SWITCH_SLIDE_BG, TEXT_DIM);
    set(&mut theme, tokens::SWITCH_SLIDE_BG_CHECKED, SURFACE_0);

    // ── 应用自定义 ──
    set(&mut theme, STATUS_OK, SUCCESS);
    set(&mut theme, STATUS_WARN, WARNING);
    set(&mut theme, STATUS_ERROR, DANGER);

    set(&mut theme, DOT_CONNECTED, SUCCESS);
    set(&mut theme, DOT_DISCONNECTED, TEXT_FAINT);
    set(&mut theme, DOT_RECONNECTING, WARNING);

    set(&mut theme, APPBAR_BG, SURFACE_1);
    set(&mut theme, SIDEBAR_BG, SURFACE_1);
    set(&mut theme, CONTENT_BG, SURFACE_0);
    set(&mut theme, STATUSBAR_BG, SURFACE_1);
    set(&mut theme, DIVIDER, BORDER);

    set(&mut theme, CARD_BG, SURFACE_2);
    set(&mut theme, CARD_BORDER, BORDER);
    set(&mut theme, CARD_HEADER_BG, SURFACE_3);

    set(&mut theme, POSE_SELECTED_BG, HIGHLIGHT.with_alpha(0.12));
    set(&mut theme, POSE_SELECTED_BORDER, HIGHLIGHT.with_alpha(0.75));
    set(&mut theme, POSE_SELECTED_TEXT, HIGHLIGHT);

    set(&mut theme, DANGER_BG, DANGER);
    set(&mut theme, DANGER_BG_HOVER, DANGER_HOVER);
    set(&mut theme, DANGER_BG_PRESSED, DANGER_PRESSED);
    set(&mut theme, DANGER_TEXT, SURFACE_0);

    set(&mut theme, BADGE_BG, SURFACE_4);
    set(&mut theme, BADGE_TEXT, TEXT_DIM);

    set(&mut theme, SECTION_TEXT, TEXT);
    set(&mut theme, FIELD_LABEL, TEXT_DIM);
    set(&mut theme, READONLY_TEXT, TEXT_FAINT);

    theme
}

/// 按状态级别取状态栏文字 token
pub fn status_token(level: super::StatusLevel) -> ThemeToken {
    match level {
        super::StatusLevel::Ok => STATUS_OK,
        super::StatusLevel::Warn => STATUS_WARN,
        super::StatusLevel::Error => STATUS_ERROR,
    }
}

/// 主题插件：注册应用主题资源
pub struct ThemePlugin;

impl Plugin for ThemePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(create_app_theme());
    }
}
