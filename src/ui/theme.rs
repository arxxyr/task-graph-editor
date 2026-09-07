//! 四套已批准的应用配色；颜色仅从内嵌的 palettes.json 读取。
//!
//! 切换主题只替换 UiTheme 并刷新现有颜色组件，不重建控件、焦点或滚动位置。

use bevy::app::PropagateSet;
use bevy::feathers::theme::{ThemeToken, UiTheme};
use bevy::feathers::tokens;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

mod palette;
mod refresh;
#[cfg(test)]
mod tests;

/// 稳定标识用于持久化，显示名称与菜单顺序在此统一定义。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeId {
    #[default]
    GraphiteTeal,
    DuskSand,
    WarmPaper,
    MistIndigo,
}

impl ThemeId {
    pub const ALL: [Self; 4] = [
        Self::GraphiteTeal,
        Self::DuskSand,
        Self::WarmPaper,
        Self::MistIndigo,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::GraphiteTeal => "graphite-teal",
            Self::DuskSand => "dusk-sand",
            Self::WarmPaper => "warm-paper",
            Self::MistIndigo => "mist-indigo",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::GraphiteTeal => "石墨青",
            Self::DuskSand => "暮砂金",
            Self::WarmPaper => "暖纸橙",
            Self::MistIndigo => "雾白靛",
        }
    }

    pub const fn is_dark(self) -> bool {
        match self {
            Self::GraphiteTeal | Self::DuskSand => true,
            Self::WarmPaper | Self::MistIndigo => false,
        }
    }

    /// 菜单色块固定展示对应方案的强调色，不随当前激活主题改变。
    pub const fn swatch_token(self) -> ThemeToken {
        match self {
            Self::GraphiteTeal => ThemeToken::new_static("app.theme.swatch.graphite-teal"),
            Self::DuskSand => ThemeToken::new_static("app.theme.swatch.dusk-sand"),
            Self::WarmPaper => ThemeToken::new_static("app.theme.swatch.warm-paper"),
            Self::MistIndigo => ThemeToken::new_static("app.theme.swatch.mist-indigo"),
        }
    }
}

/// 用户当前选择；可在插件装配前注入，启动偏好加载也可以直接更新它。
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ThemeSelection(pub ThemeId);

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

// ── 流程图的节点类别色（编码"这一步在做什么"，不是装饰）──

/// 控制流：sequence / parallel / loop / condition / delay
pub const GRAPH_FLOW: ThemeToken = ThemeToken::new_static("app.graph.flow");
/// 执行：ros2_action / behavior_tree / 导航 / 检漏
pub const GRAPH_ACT: ThemeToken = ThemeToken::new_static("app.graph.act");
/// 数据：赋值、取值、比较
pub const GRAPH_DATA: ThemeToken = ThemeToken::new_static("app.graph.data");
/// 日志与通知
pub const GRAPH_LOG: ThemeToken = ThemeToken::new_static("app.graph.log");
/// 领域专用节点
pub const GRAPH_DOMAIN: ThemeToken = ThemeToken::new_static("app.graph.domain");
/// 循环回边
pub const GRAPH_BACK: ThemeToken = ThemeToken::new_static("app.graph.back");
/// checkpoint 标记点
pub const CHECKPOINT_DOT: ThemeToken = ThemeToken::new_static("app.graph.checkpoint");

// ============================================================
// 尺寸常量
// ============================================================

/// 顶栏最小高度，窄窗口下编辑操作换行时随内容增高。
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

/// 输入框描边：保留原始边界色，不使用容器的软分隔色。
pub const INPUT_BORDER: ThemeToken = ThemeToken::new_static("app.input.border");
/// 流程图卡片与正向边使用原始边界色。
pub const GRAPH_BORDER: ThemeToken = ThemeToken::new_static("app.graph.border");
/// 流程图选中背景。
pub const GRAPH_SELECTED_BG: ThemeToken = ThemeToken::new_static("app.graph.selected.bg");
/// 流程图选中边框，与位姿选择的颜色分开。
pub const GRAPH_SELECTED_BORDER: ThemeToken = ThemeToken::new_static("app.graph.selected.border");
/// 流程图选中文字。
pub const GRAPH_SELECTED_TEXT: ThemeToken = ThemeToken::new_static("app.graph.selected.text");
/// 状态栏复制提示按钮背景。
pub const COPY_BG: ThemeToken = ThemeToken::new_static("app.copy.bg");
/// 状态栏复制提示按钮悬停背景。
pub const COPY_BG_HOVER: ThemeToken = ThemeToken::new_static("app.copy.bg.hover");
/// 状态栏复制提示按钮文字。
pub const COPY_TEXT: ThemeToken = ThemeToken::new_static("app.copy.text");

/// 与预览的 color-mix(in srgb, border 42%, panel 58%) 保持一致。
fn soft_border(border: Color, panel: Color) -> Color {
    let a = border.to_srgba();
    let b = panel.to_srgba();
    Color::srgb(
        a.red * 0.42 + b.red * 0.58,
        a.green * 0.42 + b.green * 0.58,
        a.blue * 0.42 + b.blue * 0.58,
    )
}

/// 显式提供全部 Feathers 色彩 token，浅色主题不会继承深色默认值。
pub fn create_app_theme(id: ThemeId) -> UiTheme {
    let p = palette::colors(id);
    let divider = soft_border(p.border, p.panel);
    let colors = [
        // 全局文字与键盘焦点。
        (tokens::WINDOW_BG, p.canvas),
        (tokens::TEXT_MAIN, p.text),
        (tokens::TEXT_DIM, p.muted),
        (tokens::FOCUS_RING, p.accent),
        // 普通按钮的正文不会随按下改变，所以按下背景用 selected，避免浅色对比不足。
        (tokens::BUTTON_BG, p.button),
        (tokens::BUTTON_BG_HOVER, p.hover),
        (tokens::BUTTON_BG_PRESSED, p.selected),
        (tokens::BUTTON_BG_DISABLED, p.disabled_bg),
        (tokens::BUTTON_TEXT, p.text),
        (tokens::BUTTON_TEXT_DISABLED, p.disabled_text),
        (tokens::BUTTON_PRIMARY_BG, p.accent),
        (tokens::BUTTON_PRIMARY_BG_HOVER, p.accent_hover),
        (tokens::BUTTON_PRIMARY_BG_PRESSED, p.accent_hover),
        (tokens::BUTTON_PRIMARY_BG_DISABLED, p.disabled_bg),
        (tokens::BUTTON_PRIMARY_TEXT, p.on_accent),
        (tokens::BUTTON_PRIMARY_TEXT_DISABLED, p.disabled_text),
        (tokens::BUTTON_PLAIN_BG, Color::NONE),
        (tokens::BUTTON_PLAIN_BG_HOVER, p.hover),
        (tokens::BUTTON_PLAIN_BG_PRESSED, p.selected),
        (tokens::BUTTON_PLAIN_BG_DISABLED, Color::NONE),
        // 滑块文字横跨填充条和底色，两侧均需保持正文对比度。
        (tokens::SLIDER_BG, p.input),
        (tokens::SLIDER_BG_HOVER, p.header),
        (tokens::SLIDER_BG_PRESSED, p.header),
        (tokens::SLIDER_BG_DISABLED, p.disabled_bg),
        (tokens::SLIDER_BAR, p.selected),
        (tokens::SLIDER_BAR_HOVER, p.selected),
        (tokens::SLIDER_BAR_PRESSED, p.selected),
        (tokens::SLIDER_BAR_DISABLED, p.disabled_bg),
        (tokens::SLIDER_TEXT, p.text),
        (tokens::SLIDER_TEXT_DISABLED, p.disabled_text),
        (tokens::SCROLLBAR_BG, Color::NONE),
        (tokens::SCROLLBAR_THUMB, p.border),
        (tokens::SCROLLBAR_THUMB_HOVER, p.muted),
        // 复选框。
        (tokens::CHECKBOX_BG, p.input),
        (tokens::CHECKBOX_BG_HOVER, p.hover),
        (tokens::CHECKBOX_BG_PRESSED, p.selected),
        (tokens::CHECKBOX_BG_DISABLED, p.disabled_bg),
        (tokens::CHECKBOX_BG_CHECKED, p.accent),
        (tokens::CHECKBOX_BG_CHECKED_HOVER, p.accent_hover),
        (tokens::CHECKBOX_BG_CHECKED_PRESSED, p.accent_hover),
        (tokens::CHECKBOX_BG_CHECKED_DISABLED, p.disabled_bg),
        (tokens::CHECKBOX_BORDER, p.border),
        (tokens::CHECKBOX_BORDER_HOVER, p.accent),
        (tokens::CHECKBOX_BORDER_PRESSED, p.accent_hover),
        (tokens::CHECKBOX_BORDER_DISABLED, p.disabled_text),
        (tokens::CHECKBOX_BORDER_CHECKED, p.accent),
        (tokens::CHECKBOX_BORDER_CHECKED_HOVER, p.accent_hover),
        (tokens::CHECKBOX_BORDER_CHECKED_PRESSED, p.accent_hover),
        (tokens::CHECKBOX_BORDER_CHECKED_DISABLED, p.disabled_text),
        (tokens::CHECKBOX_MARK, p.on_accent),
        (tokens::CHECKBOX_MARK_DISABLED, p.disabled_text),
        (tokens::CHECKBOX_TEXT, p.text),
        (tokens::CHECKBOX_TEXT_DISABLED, p.disabled_text),
        // 单选框与开关同样显式定义，不能遗漏其禁用及选中组合。
        (tokens::RADIO_BORDER, p.border),
        (tokens::RADIO_BORDER_HOVER, p.accent),
        (tokens::RADIO_BORDER_PRESSED, p.accent_hover),
        (tokens::RADIO_BORDER_DISABLED, p.disabled_text),
        (tokens::RADIO_BORDER_CHECKED, p.accent),
        (tokens::RADIO_BORDER_CHECKED_HOVER, p.accent_hover),
        (tokens::RADIO_BORDER_CHECKED_PRESSED, p.accent_hover),
        (tokens::RADIO_BORDER_CHECKED_DISABLED, p.disabled_text),
        (tokens::RADIO_MARK, p.accent),
        (tokens::RADIO_MARK_HOVER, p.accent_hover),
        (tokens::RADIO_MARK_PRESSED, p.accent_hover),
        (tokens::RADIO_MARK_DISABLED, p.disabled_text),
        (tokens::RADIO_TEXT, p.text),
        (tokens::RADIO_TEXT_DISABLED, p.disabled_text),
        (tokens::SWITCH_BG, p.input),
        (tokens::SWITCH_BG_HOVER, p.hover),
        (tokens::SWITCH_BG_PRESSED, p.selected),
        (tokens::SWITCH_BG_DISABLED, p.disabled_bg),
        (tokens::SWITCH_BG_CHECKED, p.accent),
        (tokens::SWITCH_BG_CHECKED_HOVER, p.accent_hover),
        (tokens::SWITCH_BG_CHECKED_PRESSED, p.accent_hover),
        (tokens::SWITCH_BG_CHECKED_DISABLED, p.disabled_bg),
        (tokens::SWITCH_BORDER, p.border),
        (tokens::SWITCH_BORDER_HOVER, p.accent),
        (tokens::SWITCH_BORDER_PRESSED, p.accent_hover),
        (tokens::SWITCH_BORDER_DISABLED, p.disabled_text),
        (tokens::SWITCH_BORDER_CHECKED, p.accent),
        (tokens::SWITCH_BORDER_CHECKED_HOVER, p.accent_hover),
        (tokens::SWITCH_BORDER_CHECKED_PRESSED, p.accent_hover),
        (tokens::SWITCH_BORDER_CHECKED_DISABLED, p.disabled_text),
        (tokens::SWITCH_SLIDE_BG, p.muted),
        (tokens::SWITCH_SLIDE_BG_HOVER, p.text),
        (tokens::SWITCH_SLIDE_BG_PRESSED, p.text),
        (tokens::SWITCH_SLIDE_BG_DISABLED, p.disabled_text),
        (tokens::SWITCH_SLIDE_BG_CHECKED, p.on_accent),
        (tokens::SWITCH_SLIDE_BG_CHECKED_HOVER, p.on_accent),
        (tokens::SWITCH_SLIDE_BG_CHECKED_PRESSED, p.on_accent),
        (tokens::SWITCH_SLIDE_BG_CHECKED_DISABLED, p.disabled_text),
        (tokens::SWITCH_SLIDE_BORDER, p.muted),
        (tokens::SWITCH_SLIDE_BORDER_HOVER, p.text),
        (tokens::SWITCH_SLIDE_BORDER_PRESSED, p.text),
        (tokens::SWITCH_SLIDE_BORDER_DISABLED, p.disabled_text),
        (tokens::SWITCH_SLIDE_BORDER_CHECKED, p.on_accent),
        (tokens::SWITCH_SLIDE_BORDER_CHECKED_HOVER, p.on_accent),
        (tokens::SWITCH_SLIDE_BORDER_CHECKED_PRESSED, p.on_accent),
        (
            tokens::SWITCH_SLIDE_BORDER_CHECKED_DISABLED,
            p.disabled_text,
        ),
        (tokens::COLOR_PLANE_BG, p.panel),
        // 菜单的普通、悬停、按下与键盘选项均使用可承载正文的表面。
        (tokens::MENU_BG, p.panel),
        (tokens::MENU_BORDER, p.border),
        (tokens::MENUITEM_BG_HOVER, p.hover),
        (tokens::MENUITEM_BG_PRESSED, p.selected),
        (tokens::MENUITEM_BG_FOCUSED, p.selected),
        (tokens::MENUITEM_TEXT, p.text),
        (tokens::MENUITEM_TEXT_DISABLED, p.disabled_text),
        // 文本选区没有独立选中文字 token，背景必须保证原文字仍清晰。
        (tokens::TEXT_INPUT_BG, p.input),
        (tokens::TEXT_INPUT_LABEL_BG, p.header),
        (tokens::TEXT_INPUT_TEXT, p.text),
        (tokens::TEXT_INPUT_TEXT_DISABLED, p.disabled_text),
        (tokens::TEXT_INPUT_CURSOR, p.accent),
        (tokens::TEXT_INPUT_SELECTION, p.selected),
        (tokens::TEXT_INPUT_SELECTION_UNFOCUSED, p.header),
        (tokens::TEXT_INPUT_X_AXIS, p.axis_x),
        (tokens::TEXT_INPUT_Y_AXIS, p.axis_y),
        (tokens::TEXT_INPUT_Z_AXIS, p.axis_z),
        // 卡片和分组使用预览的软分隔；输入框及图边另外保留原始边界。
        (tokens::PANE_HEADER_BG, p.header),
        (tokens::PANE_HEADER_BORDER, divider),
        (tokens::PANE_HEADER_TEXT, p.text),
        (tokens::PANE_HEADER_DIVIDER, divider),
        (tokens::PANE_BODY_BG, p.panel),
        (tokens::SUBPANE_HEADER_BG, p.header),
        (tokens::SUBPANE_HEADER_BORDER, divider),
        (tokens::SUBPANE_HEADER_TEXT, p.muted),
        (tokens::SUBPANE_BODY_BG, p.panel),
        (tokens::SUBPANE_BODY_BORDER, divider),
        (tokens::GROUP_HEADER_BG, p.header),
        (tokens::GROUP_HEADER_BORDER, divider),
        (tokens::GROUP_HEADER_TEXT, p.text),
        (tokens::GROUP_BODY_BG, p.panel),
        (tokens::GROUP_BODY_BORDER, divider),
        (tokens::LISTROW_BG, Color::NONE),
        (tokens::LISTROW_BG_HOVER, p.hover),
        (tokens::LISTROW_BG_SELECTED, p.selected),
        (tokens::LISTROW_TEXT, p.text),
        (tokens::LISTROW_TEXT_DISABLED, p.disabled_text),
        // 应用语义色和区域表面。
        (STATUS_OK, p.success),
        (STATUS_WARN, p.warning),
        (STATUS_ERROR, p.danger),
        (DOT_CONNECTED, p.success),
        (DOT_DISCONNECTED, p.faint),
        (DOT_RECONNECTING, p.warning),
        (APPBAR_BG, p.shell),
        (SIDEBAR_BG, p.shell),
        (CONTENT_BG, p.canvas),
        (STATUSBAR_BG, p.shell),
        (DIVIDER, divider),
        (CARD_BG, p.panel),
        (CARD_BORDER, divider),
        (CARD_HEADER_BG, p.header),
        (INPUT_BORDER, p.border),
        (POSE_SELECTED_BG, p.pose_bg),
        (POSE_SELECTED_BORDER, p.pose_border),
        (POSE_SELECTED_TEXT, p.pose_text),
        (DANGER_BG, p.danger),
        (DANGER_BG_HOVER, p.danger),
        (DANGER_BG_PRESSED, p.danger),
        (DANGER_TEXT, p.on_accent),
        (BADGE_BG, p.button),
        (BADGE_TEXT, p.muted),
        (SECTION_TEXT, p.text),
        (FIELD_LABEL, p.muted),
        (READONLY_TEXT, p.faint),
        (GRAPH_FLOW, p.flow),
        (GRAPH_ACT, p.act),
        (GRAPH_DATA, p.data),
        (GRAPH_LOG, p.log),
        (GRAPH_DOMAIN, p.domain),
        (GRAPH_BACK, p.back),
        (GRAPH_BORDER, p.border),
        (GRAPH_SELECTED_BG, p.selected),
        (GRAPH_SELECTED_BORDER, p.accent),
        (GRAPH_SELECTED_TEXT, p.selected_text),
        (CHECKPOINT_DOT, p.warning),
        (COPY_BG, p.copy_bg),
        (COPY_BG_HOVER, p.copy_hover),
        (COPY_TEXT, p.copy_text),
    ];
    let mut theme = UiTheme(bevy::feathers::theme::ThemeProps {
        color: colors.into_iter().collect(),
    });
    for swatch in ThemeId::ALL {
        theme
            .0
            .color
            .insert(swatch.swatch_token(), palette::colors(swatch).accent);
    }
    theme
}

/// 按状态级别取状态栏文字 token。
pub fn status_token(level: super::StatusLevel) -> ThemeToken {
    match level {
        super::StatusLevel::Ok => STATUS_OK,
        super::StatusLevel::Warn => STATUS_WARN,
        super::StatusLevel::Error => STATUS_ERROR,
    }
}

fn apply_theme_selection(selection: Res<ThemeSelection>, mut theme: ResMut<UiTheme>) {
    if selection.is_changed() {
        *theme = create_app_theme(selection.0);
    }
}

/// 主题在本帧输入、动作和重建结束后应用，赶在 Feathers 的 PostUpdate 和文本传播之前。
pub struct ThemePlugin;

impl Plugin for ThemePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ThemeSelection>();
        let selection = app.world().resource::<ThemeSelection>().0;
        app.insert_resource(create_app_theme(selection))
            .add_observer(
                refresh::initialize_text_cursor::<bevy::feathers::controls::FeathersTextInput>,
            )
            .add_observer(refresh::initialize_text_cursor::<bevy::text::TextCursorStyle>)
            .add_observer(
                refresh::initialize_input_text::<bevy::feathers::controls::FeathersTextInput>,
            )
            .add_observer(refresh::initialize_input_text::<bevy::ui::InteractionDisabled>)
            .add_observer(refresh::enable_input_text)
            .add_systems(Update, apply_theme_selection.after(super::UiSet::Rebuild))
            .configure_sets(
                PostUpdate,
                PropagateSet::<TextColor>::default()
                    .in_set(bevy::ui::UiSystems::Propagate)
                    .before(bevy::ui::UiSystems::Content),
            )
            .add_systems(
                PostUpdate,
                (
                    refresh::refresh_direct_colors,
                    refresh::refresh_inherited_text,
                    refresh::refresh_text_cursor,
                    refresh::refresh_slider,
                    refresh::refresh_disclosure,
                    refresh::refresh_menu_icons,
                )
                    .before(PropagateSet::<TextColor>::default())
                    .before(bevy::ui::UiSystems::Content),
            );
    }
}
