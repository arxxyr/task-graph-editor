//! 补齐 Feathers 0.19.1 在资源换色时未更新的派生颜色。
//!
//! 上游 theme::update_theme 会刷新直接背景、边框和文字，但继承文字只在 token
//! 插入时传播；滑块和折叠图标则只在交互状态变化时改色。这里只补这些遗漏，
//! 不改 token、值、焦点、光标位置或实体层级。

use bevy::app::Propagate;
use bevy::feathers::controls::{
    FeathersDisclosureToggle, FeathersMenuButton, FeathersSlider, FeathersTextInput,
};
use bevy::feathers::theme::{
    InheritableThemeTextColor, ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, UiTheme,
};
use bevy::feathers::tokens;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::TextCursorStyle;
use bevy::ui::{InteractionDisabled, Pressed};

type SliderColors<'w, 's> = Query<
    'w,
    's,
    (
        Has<InteractionDisabled>,
        Has<Pressed>,
        &'static Hovered,
        &'static mut BackgroundGradient,
    ),
    With<FeathersSlider>,
>;

/// 上游直接颜色刷新没有排在 UiSystems::Content 之前；文字排版可能先读到旧色。
/// 此处在布局前补齐，按值判断后写入；上游稍后再运行也只会写同一颜色。
pub(super) fn refresh_direct_colors(
    theme: Res<UiTheme>,
    mut backgrounds: Query<(&mut BackgroundColor, &ThemeBackgroundColor)>,
    mut borders: Query<(&mut BorderColor, &ThemeBorderColor)>,
    mut text: Query<(&mut TextColor, &ThemeTextColor)>,
) {
    if !theme.is_changed() {
        return;
    }
    for (mut background, token) in &mut backgrounds {
        background.set_if_neq(BackgroundColor(theme.color(&token.0)));
    }
    for (mut border, token) in &mut borders {
        border.set_if_neq(BorderColor::all(theme.color(&token.0)));
    }
    for (mut color, token) in &mut text {
        color.set_if_neq(TextColor(theme.color(&token.0)));
    }
}

pub(super) fn refresh_inherited_text(
    theme: Res<UiTheme>,
    text: Query<(Entity, &InheritableThemeTextColor)>,
    mut commands: Commands,
) {
    if !theme.is_changed() {
        return;
    }
    for (entity, token) in &text {
        commands
            .entity(entity)
            .insert(Propagate(TextColor(theme.color(&token.0))));
    }
}

pub(super) fn refresh_text_cursor(
    theme: Res<UiTheme>,
    mut cursors: Query<&mut TextCursorStyle, With<FeathersTextInput>>,
) {
    if !theme.is_changed() {
        return;
    }
    // 上游在 PreUpdate 才检查主题；这里覆盖 Update 里选择主题的同一帧。
    for mut cursor in &mut cursors {
        apply_cursor_colors(&theme, &mut cursor);
    }
}

/// 懒生成的 Feathers 输入框也必须初始化，不能等待下一次用户切换主题。
/// 分别观察标记与样式的插入，兼容 BSN 按任意顺序补齐这两个组件。
pub(super) fn initialize_text_cursor<C: Component>(
    event: On<Insert, C>,
    theme: Res<UiTheme>,
    mut cursors: Query<&mut TextCursorStyle, With<FeathersTextInput>>,
) {
    if let Ok(mut cursor) = cursors.get_mut(event.entity) {
        apply_cursor_colors(&theme, &mut cursor);
    }
}

fn apply_cursor_colors(theme: &UiTheme, cursor: &mut TextCursorStyle) {
    cursor.color = theme.color(&tokens::TEXT_INPUT_CURSOR);
    cursor.selection_color = theme.color(&tokens::TEXT_INPUT_SELECTION);
    cursor.unfocused_selection_color = theme.color(&tokens::TEXT_INPUT_SELECTION_UNFOCUSED);
}

/// 上游 FeathersTextInput 场景没有 ThemedText，EditableText 因而保留默认白字。
/// 直接绑定主题文字，同时覆盖默认色；不用依赖容器层级或 BSN 的组件插入顺序。
pub(super) fn initialize_input_text<C: Component>(
    event: On<Insert, C>,
    inputs: Query<Has<InteractionDisabled>, With<FeathersTextInput>>,
    theme: Res<UiTheme>,
    mut commands: Commands,
) {
    if let Ok(disabled) = inputs.get(event.entity) {
        set_input_text(event.entity, disabled, &theme, &mut commands);
    }
}

pub(super) fn enable_input_text(
    event: On<Remove, InteractionDisabled>,
    inputs: Query<(), With<FeathersTextInput>>,
    theme: Res<UiTheme>,
    mut commands: Commands,
) {
    if inputs.contains(event.entity) {
        set_input_text(event.entity, false, &theme, &mut commands);
    }
}

fn set_input_text(entity: Entity, disabled: bool, theme: &UiTheme, commands: &mut Commands) {
    let token = match disabled {
        true => tokens::TEXT_INPUT_TEXT_DISABLED,
        false => tokens::TEXT_INPUT_TEXT,
    };
    let color = theme.color(&token);
    commands
        .entity(entity)
        .try_insert((ThemeTextColor(token), TextColor(color)));
}

pub(super) fn refresh_slider(theme: Res<UiTheme>, mut sliders: SliderColors) {
    if !theme.is_changed() {
        return;
    }
    for (disabled, pressed, hovered, mut gradient) in &mut sliders {
        let (bar, background) = match (disabled, pressed, hovered.0) {
            (true, _, _) => (tokens::SLIDER_BAR_DISABLED, tokens::SLIDER_BG_DISABLED),
            (false, true, _) => (tokens::SLIDER_BAR_PRESSED, tokens::SLIDER_BG_PRESSED),
            (false, false, true) => (tokens::SLIDER_BAR_HOVER, tokens::SLIDER_BG_HOVER),
            (false, false, false) => (tokens::SLIDER_BAR, tokens::SLIDER_BG),
        };
        if let [Gradient::Linear(linear)] = &mut gradient.0[..]
            && let [bar_start, bar_end, background_start, background_end] = &mut linear.stops[..]
        {
            bar_start.color = theme.color(&bar);
            bar_end.color = theme.color(&bar);
            background_start.color = theme.color(&background);
            background_end.color = theme.color(&background);
        }
    }
}

pub(super) fn refresh_disclosure(
    theme: Res<UiTheme>,
    toggles: Query<(Has<InteractionDisabled>, &Children), With<FeathersDisclosureToggle>>,
    mut images: Query<&mut ImageNode>,
) {
    if !theme.is_changed() {
        return;
    }
    for (disabled, children) in &toggles {
        if let Some(child) = children.first()
            && let Ok(mut image) = images.get_mut(*child)
        {
            let token = match disabled {
                true => tokens::BUTTON_TEXT_DISABLED,
                false => tokens::BUTTON_TEXT,
            };
            image.color = theme.color(&token);
        }
    }
}

/// 菜单的箭头由上游直接生成 ImageNode，默认白色，也不参与文字主题传播。
/// 仅检查菜单按钮的直接图像子项，兼容 BSN 跨帧补齐和禁用状态变更；同色不写入。
pub(super) fn refresh_menu_icons(
    theme: Res<UiTheme>,
    buttons: Query<(Has<InteractionDisabled>, &Children), With<FeathersMenuButton>>,
    mut images: Query<&mut ImageNode>,
) {
    for (disabled, children) in &buttons {
        let token = match disabled {
            true => tokens::BUTTON_TEXT_DISABLED,
            false => tokens::BUTTON_TEXT,
        };
        let color = theme.color(&token);
        for child in children {
            if let Ok(mut image) = images.get_mut(*child)
                && image.color != color
            {
                image.color = color;
            }
        }
    }
}
