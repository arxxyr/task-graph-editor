//! 主题菜单：四个固定选项随入口一次生成，切换只更新资源和文字。

use bevy::feathers::controls::{
    FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
};
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeTextColor, ThemedText};
use bevy::feathers::tokens;
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use bevy::ui_widgets::popover::{Popover, PopoverAlign, PopoverPlacement, PopoverSide};
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};

use super::UiSet;
use super::theme::{ThemeId, ThemeSelection};

#[derive(Component, Clone, Default)]
struct ThemePickerPopup;

#[derive(Component, Clone, Default)]
struct ThemeChoice(ThemeId);

/// 入口与勾选标记共用同步查询，避免两个可变 Text 查询互相冲突。
#[derive(Component, Clone, Default)]
enum ThemeCaption {
    #[default]
    Current,
    Check(ThemeId),
}

/// 顶栏入口；无需连接会话，菜单的背景与交互色继承 Feathers 主题 token。
pub fn theme_picker() -> impl Scene {
    // 整个固定菜单内联到同一场景；按钮可交互时，所有可聚焦的子项已经存在。
    // 不在打开时重新排队生成，避免焦点落到随后被销毁的旧实体。
    let choices: Vec<_> = ThemeId::ALL.into_iter().map(theme_option).collect();
    bsn! {
        @FeathersMenu
        Node { flex_shrink: 0.0 }
        Children [
            (
                @FeathersMenuButton {
                    @caption: {bsn! {
                        Text("选择主题")
                        ThemedText
                        template_value(ThemeCaption::Current)
                    }}
                }
                AccessibleLabel("选择界面主题")
            ),
            (
                @FeathersMenuPopup
                ThemePickerPopup
                Node { min_width: px(210) }
                // 入口在顶栏最右侧，优先向左展开，仍保留靠窗口边缘时的浮层修正。
                Popover {
                    positions: {vec![
                        PopoverPlacement {
                            side: PopoverSide::Bottom,
                            align: PopoverAlign::End,
                            gap: 4.0,
                        },
                        PopoverPlacement {
                            side: PopoverSide::Top,
                            align: PopoverAlign::End,
                            gap: 4.0,
                        },
                    ]},
                    window_margin: 10.0,
                }
                Children [{choices}]
            )
        ]
    }
}

fn theme_option(id: ThemeId) -> impl Scene {
    let mode = match id.is_dark() {
        true => "深色",
        false => "浅色",
    };
    bsn! {
        @FeathersMenuItem {
            @caption: {bsn! {
                Node {
                    width: percent(100),
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                }
                // 继承文字颜色要求中间容器也加入传播链。
                ThemedText
                Children [
                    (
                        Text("")
                        ThemedText
                        TextFont { font_size: px(14) }
                        template_value(ThemeCaption::Check(id))
                        Node { width: px(14), flex_shrink: 0.0 }
                    ),
                    (
                        Node {
                            width: px(12),
                            height: px(12),
                            border_radius: {BorderRadius::all(px(3))},
                            flex_shrink: 0.0,
                        }
                        ThemeBackgroundColor({id.swatch_token()})
                    ),
                    (Text({id.label()}) ThemedText TextFont { font_size: px(14) }),
                    (Node { flex_grow: 1.0 }),
                    (
                        Text(mode)
                        ThemeTextColor({tokens::TEXT_DIM})
                        TextFont { font_size: px(11) }
                    )
                ]
            }}
        }
        Node { min_height: px(30), flex_shrink: 0.0 }
        template_value(ThemeChoice(id))
        on(select_theme)
    }
}

fn select_theme(
    event: On<Activate>,
    choices: Query<&ThemeChoice>,
    mut selection: ResMut<ThemeSelection>,
) {
    let Ok(choice) = choices.get(event.entity) else {
        return;
    };
    selection.set_if_neq(ThemeSelection(choice.0));
}

fn sync_theme_captions(
    selection: Res<ThemeSelection>,
    mut captions: Query<(&mut Text, Ref<ThemeCaption>)>,
) {
    for (mut text, caption) in &mut captions {
        if !selection.is_changed() && !caption.is_added() {
            continue;
        }
        let value = match *caption {
            ThemeCaption::Current => format!("选择主题：{}", selection.0.label()),
            ThemeCaption::Check(id) => match selection.0 == id {
                true => "✓".to_string(),
                false => String::new(),
            },
        };
        text.set_if_neq(Text(value));
    }
}

/// 菜单可由辅助功能事件在较晚阶段打开，补一次唤醒以提交浮层布局与焦点变化。
/// 仅在可见性变化时触发，不让空闲窗口持续重绘。
fn wake_popup_changes(
    changed: Query<(), (With<ThemePickerPopup>, Changed<Visibility>)>,
    proxy: Option<Res<EventLoopProxyWrapper>>,
) {
    if !changed.is_empty()
        && let Some(proxy) = proxy
    {
        let _ = proxy.send_event(WinitUserEvent::WakeUp);
    }
}

pub struct ThemePickerPlugin;

impl Plugin for ThemePickerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ThemeSelection>()
            .add_systems(Update, sync_theme_captions.in_set(UiSet::Rebuild))
            .add_systems(PostUpdate, wake_popup_changes);
    }
}

#[cfg(test)]
mod tests;
