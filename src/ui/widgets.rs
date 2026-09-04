//! 通用界面构件：卡片、折叠区块、表单行、数值输入等
//!
//! 这些是 BSN 场景函数，返回 `impl Scene`，可以直接嵌进任何 `Children [...]`。
//! 统一在这里定义，保证整个界面的间距、圆角、字号一致。

use bevy::feathers::controls::{
    ButtonVariant, FeathersButton, FeathersCheckbox, FeathersNumberInput, FeathersTextInput,
    FeathersTextInputContainer, NumberFormat,
};
use bevy::feathers::theme::{
    ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemeToken, ThemedText,
};
use bevy::feathers::tokens;
use bevy::prelude::*;
use bevy::text::{EditableText, FontWeight, LineBreak};
use bevy::ui::{Checked, InteractionDisabled};

use super::theme;

/// 表单行的标签列宽（连接面板，中文短标签）
const FORM_LABEL_WIDTH: f32 = 76.0;

/// 字段行的标签列宽（context 字段名，常见 20~30 个字符的标识符）
const FIELD_LABEL_WIDTH: f32 = 190.0;

/// 装箱后的场景
///
/// 每个 `bsn!` 都是各自独立的匿名类型，要把它们放进同一个 `Vec`（动态列表、
/// 递归字段树）就必须装箱。`Box<dyn SceneBox>` 本身实现了 `Scene`，
/// `Vec<Box<dyn SceneBox>>` 也就自动是 `SceneList`。
pub type BoxedScene = Box<dyn bevy::scene::SceneBox>;

/// 把任意场景装箱
pub fn boxed(scene: impl Scene) -> BoxedScene {
    Box::new(scene)
}

/// 可插入 BSN 的标记组件
///
/// `template_value` 依赖 `impl<T: Clone + Default + Unpin> Template for T`，
/// 故标记组件必须同时可克隆、可默认构造。
pub trait Marker: Component + Clone + Default + Unpin {}

impl<T: Component + Clone + Default + Unpin> Marker for T {}

// ============================================================
// 折叠区块
// ============================================================

/// 可折叠区块的根节点
///
/// 约定子节点顺序为 `[头部, 内容]`，切换时改内容节点的 `Node::display`。
#[derive(Component, Clone)]
pub struct Collapsible {
    /// 是否展开
    pub open: bool,
}

impl Default for Collapsible {
    fn default() -> Self {
        Self { open: true }
    }
}

/// 折叠区块的头部（点击切换）
#[derive(Component, Default, Clone)]
pub struct CollapseHeader;

/// 折叠区块的内容容器
#[derive(Component, Default, Clone)]
pub struct CollapseBody;

/// 折叠指示三角（随展开状态换字形）
#[derive(Component, Default, Clone)]
pub struct CollapseChevron;

/// 展开状态的指示符
const CHEVRON_OPEN: &str = "▾";
/// 折叠状态的指示符
const CHEVRON_CLOSED: &str = "▸";

/// 点击头部：切换所属折叠区块的展开状态
fn on_header_click(
    click: On<Pointer<Click>>,
    headers: Query<(), With<CollapseHeader>>,
    parents: Query<&ChildOf>,
    mut sections: Query<&mut Collapsible>,
) {
    // 点中的多半是标题里的文字，往上找到头部那一层
    let Some(header) = self_or_ancestor(click.entity, &parents, |e| headers.contains(e)) else {
        return;
    };
    let Ok(parent) = parents.get(header) else {
        return;
    };
    if let Ok(mut section) = sections.get_mut(parent.parent()) {
        section.open = !section.open;
    }
}

/// 同步展开状态到内容节点的显示与指示符字形
fn sync_collapse_state(
    sections: Query<(&Collapsible, &Children), Changed<Collapsible>>,
    mut bodies: Query<&mut Node, With<CollapseBody>>,
    chevrons: Query<Entity, With<CollapseChevron>>,
    descendants: Query<&Children>,
    mut texts: Query<&mut Text>,
) {
    for (section, children) in &sections {
        let mark = match section.open {
            true => CHEVRON_OPEN,
            false => CHEVRON_CLOSED,
        };
        for child in children.iter() {
            // 内容节点：切换显示
            if let Ok(mut node) = bodies.get_mut(child) {
                node.display = match section.open {
                    true => Display::Flex,
                    false => Display::None,
                };
            }
            // 头部里的指示符：换字形
            for descendant in descendants.iter_descendants(child) {
                if chevrons.contains(descendant)
                    && let Ok(mut text) = texts.get_mut(descendant)
                {
                    text.0 = mark.into();
                }
            }
        }
    }
}

/// 折叠区块：可点击的标题行 + 可隐藏的内容区
///
/// - `title`：标题文字
/// - `count`：右侧计数徽章内容，`None` 时不显示
/// - `open`：初始是否展开
/// - `body`：内容
pub fn collapsible(
    title: impl Into<String>,
    count: Option<String>,
    open: bool,
    body: impl SceneList,
) -> impl Scene {
    collapsible_titled(title, NoMarker, count, open, body)
}

/// 折叠区块，并给标题文字挂上标记组件（便于之后单独改标题配色）
pub fn collapsible_titled(
    title: impl Into<String>,
    title_marker: impl Marker,
    count: Option<String>,
    open: bool,
    body: impl SceneList,
) -> impl Scene {
    let mark = match open {
        true => CHEVRON_OPEN,
        false => CHEVRON_CLOSED,
    };
    let body_display = match open {
        true => Display::Flex,
        false => Display::None,
    };
    // 无徽章时用空列表，避免多spawn一个空实体
    let badges: Vec<_> = count.into_iter().map(badge).collect();
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
        }
        Collapsible { open: open }
        Children [
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(6),
                    padding: {UiRect::axes(px(theme::PAD_SM), px(5.0))},
                    border_radius: {BorderRadius::all(px(theme::RADIUS_SM))},
                    width: percent(100),
                }
                CollapseHeader
                ThemeBackgroundColor({theme::CARD_HEADER_BG})
                Children [
                    (
                        Text(mark)
                        CollapseChevron
                        ThemeTextColor({theme::SECTION_TEXT})
                        TextFont { font_size: px(10.0) }
                    ),
                    (
                        Text({title.into()})
                        template_value(title_marker)
                        ThemeTextColor({theme::SECTION_TEXT})
                        TextFont {
                            font_size: px(13.0),
                            weight: {FontWeight::BOLD}
                        }
                    ),
                    {badges}
                ]
            ),
            (
                Node {
                    display: body_display,
                    flex_direction: FlexDirection::Column,
                    row_gap: px(4),
                    padding: {UiRect::new(px(theme::INDENT), px(0.0), px(4.0), px(2.0))},
                    width: percent(100),
                }
                CollapseBody
                Children [{body}]
            )
        ]
    }
}

// ============================================================
// 基础构件
// ============================================================

/// 计数徽章
pub fn badge(text: impl Into<String>) -> impl Scene {
    bsn! {
        Node {
            padding: {UiRect::axes(px(5.0), px(1.0))},
            border_radius: {BorderRadius::all(px(7.0))},
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
        }
        ThemeBackgroundColor({theme::BADGE_BG})
        Children [(
            Text({text.into()})
            ThemeTextColor({theme::BADGE_TEXT})
            TextFont { font_size: px(10.0) }
        )]
    }
}

/// 占位标记：不需要标记时传它
#[derive(Component, Clone, Default)]
pub struct NoMarker;

/// 卡片：带标题栏的内容容器
pub fn card(title: impl Into<String>, body: impl SceneList) -> impl Scene {
    card_titled(title, NoMarker, body)
}

/// 卡片，并给标题文字挂上标记组件（便于之后单独更新标题）
pub fn card_titled(
    title: impl Into<String>,
    title_marker: impl Marker,
    body: impl SceneList,
) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            border: {UiRect::all(px(1.0))},
            border_radius: {BorderRadius::all(px(theme::RADIUS))},
            overflow: {Overflow::clip()},
            flex_shrink: 0.0,
        }
        ThemeBackgroundColor({theme::CARD_BG})
        ThemeBorderColor({theme::CARD_BORDER})
        Children [
            (
                Node {
                    padding: {UiRect::axes(px(theme::PAD), px(6.0))},
                    align_items: AlignItems::Center,
                    width: percent(100),
                }
                ThemeBackgroundColor({theme::CARD_HEADER_BG})
                Children [(
                    Text({title.into()})
                    template_value(title_marker)
                    ThemeTextColor({theme::SECTION_TEXT})
                    TextFont {
                        font_size: px(13.0),
                        weight: {FontWeight::BOLD}
                    }
                )]
            ),
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(6),
                    padding: {UiRect::all(px(theme::PAD))},
                    width: percent(100),
                }
                Children [{body}]
            )
        ]
    }
}

/// 表单行：左侧固定宽度标签 + 右侧控件（连接面板用）
pub fn form_row(label: impl Into<String>, control: impl Scene) -> impl Scene {
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
                    width: {px(FORM_LABEL_WIDTH)},
                    flex_shrink: 0.0,
                    align_items: AlignItems::Center,
                    overflow: {Overflow::clip()},
                }
                Children [(
                    Text({label.into()})
                    ThemeTextColor({theme::FIELD_LABEL})
                    TextFont { font_size: px(12.0) }
                    TextLayout { linebreak: {LineBreak::AnyCharacter} }
                )]
            ),
            (control)
        ]
    }
}

/// 字段行：标签在左（最小宽度），控件在右侧撑满（编辑器用）
pub fn field_row(label: impl Into<String>, control: impl Scene) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(6),
            width: percent(100),
            padding: {UiRect::vertical(px(1.0))},
        }
        Children [
            (
                Node {
                    width: {px(FIELD_LABEL_WIDTH)},
                    flex_shrink: 0.0,
                    align_items: AlignItems::Center,
                    overflow: {Overflow::clip()},
                }
                Children [(
                    Text({label.into()})
                    ThemeTextColor({theme::FIELD_LABEL})
                    TextFont { font_size: px(12.0) }
                    // context 字段名是不含空格的标识符，按词换行等于不换行，
                    // min-content 会顶破标签列宽把文字压到控件上；允许逐字符断行
                    TextLayout { linebreak: {LineBreak::AnyCharacter} }
                )]
            ),
            (control)
        ]
    }
}

/// 横向排列容器
pub fn row(gap: f32, body: impl SceneList) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: {px(gap)},
            width: percent(100),
        }
        Children [{body}]
    }
}

/// 弱化提示文字
pub fn hint(text: impl Into<String>) -> impl Scene {
    bsn! {
        Text({text.into()})
        ThemeTextColor({theme::READONLY_TEXT})
        TextFont { font_size: px(11.0) }
    }
}

/// 只读值文字
pub fn readonly_value(text: impl Into<String>) -> impl Scene {
    bsn! {
        Text({text.into()})
        ThemeTextColor({theme::READONLY_TEXT})
        TextFont { font_size: px(12.0) }
    }
}

/// 小标题（位姿部位名等）
pub fn subheading(text: impl Into<String>) -> impl Scene {
    bsn! {
        Text({text.into()})
        ThemeTextColor({theme::SECTION_TEXT})
        TextFont {
            font_size: px(12.0),
            weight: {FontWeight::BOLD}
        }
    }
}

/// 弹性占位（把后续内容推到另一端）
pub fn spacer() -> impl Scene {
    bsn! {
        Node { flex_grow: 1.0 }
    }
}

/// 状态指示圆点
pub fn status_dot(token: ThemeToken) -> impl Scene {
    bsn! {
        Node {
            width: px(8),
            height: px(8),
            border_radius: {BorderRadius::all(px(4.0))},
            flex_shrink: 0.0,
        }
        ThemeBackgroundColor({token})
    }
}

// ============================================================
// 输入控件
// ============================================================

/// 单行文本输入
///
/// `marker` 会挂到输入框实体上，供 observer 识别数据来源。
pub fn text_field(initial: impl Into<String>, marker: impl Marker) -> impl Scene {
    let initial = initial.into();
    bsn! {
        @FeathersTextInputContainer
        Node { flex_grow: 1.0 }
        Children [(
            @FeathersTextInput
            template_value(EditableText::new(initial))
            template_value(marker)
        )]
    }
}

/// 密码输入（内容以遮罩显示）
pub fn password_field(initial: impl Into<String>, marker: impl Marker) -> impl Scene {
    let initial = initial.into();
    bsn! {
        @FeathersTextInputContainer
        Node { flex_grow: 1.0 }
        Children [(
            @FeathersTextInput
            template_value(super::password::PasswordInput::new(initial))
            template_value(marker)
            on(super::password::on_password_edit)
        )]
    }
}

/// f64 数值输入
///
/// `axis` 决定左侧色条与标签，用于区分 x/y/z 轴；`None` 时无色条。
pub fn number_field(
    value: f64,
    axis: Option<(ThemeToken, &'static str)>,
    marker: impl Marker,
) -> impl Scene {
    let (sigil, label) = match axis {
        Some((token, text)) => (token, Some(text)),
        None => (tokens::TEXT_INPUT_BG, None),
    };
    bsn! {
        @FeathersNumberInput {
            @sigil_color: sigil,
            @label_text: label,
            @number_format: {NumberFormat::F64}
        }
        Node { flex_grow: 1.0 }
        template_value(marker)
        template_value(NumberFieldInit(NumberInitValue::F64(value)))
    }
}

/// i64 数值输入
pub fn int_field(value: i64, marker: impl Marker) -> impl Scene {
    bsn! {
        @FeathersNumberInput {
            @number_format: {NumberFormat::I64}
        }
        Node { flex_grow: 1.0 }
        template_value(marker)
        template_value(NumberFieldInit(NumberInitValue::I64(value)))
    }
}

/// 数值输入的初值（新建后由系统推送给控件）
///
/// Feathers 的数值输入没有"初始值" prop，只能在实体建好后触发
/// `UpdateNumberInput` 事件写入，这里用一个一次性组件记录待写入的值。
#[derive(Component, Clone, Copy, Default)]
pub struct NumberFieldInit(pub NumberInitValue);

/// 数值输入初值的类型
#[derive(Clone, Copy, Default)]
pub enum NumberInitValue {
    /// 64 位浮点
    #[default]
    Zero,
    /// 64 位浮点
    F64(f64),
    /// 64 位整数
    I64(i64),
}

/// 复选框
pub fn checkbox(checked: bool, marker: impl Marker) -> impl Scene {
    // Checked 是标记组件，只在选中时插入
    let checked_marker = checked.then(|| template_value(Checked));
    bsn! {
        @FeathersCheckbox
        template_value(marker)
        {checked_marker}
    }
}

/// 按钮
pub fn button(label: impl Into<String>, variant: ButtonVariant, marker: impl Marker) -> impl Scene {
    button_gated(label, variant, ButtonGate::Always, marker)
}

/// 按钮的可用条件
///
/// 禁用态是**属性**变化，不能拿它当重建条件——忙碌状态一翻转就重建整组按钮，
/// 会撞上场景排队落地的竞态，凭空多出一份。这里只挂条件，由
/// [`sync_button_gates`] 增删 `InteractionDisabled`。
#[derive(Component, Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonGate {
    /// 始终可用
    #[default]
    Always,
    /// 空闲时可用（忙碌则灰显）
    WhenIdle,
    /// 空闲且已选中位姿时可用
    WhenIdleAndPose,
}

/// 按钮，带可用条件
pub fn button_gated(
    label: impl Into<String>,
    variant: ButtonVariant,
    gate: ButtonGate,
    marker: impl Marker,
) -> impl Scene {
    bsn! {
        @FeathersButton {
            @caption: {bsn! { Text({label.into()}) ThemedText }},
            @variant: variant
        }
        template_value(marker)
        template_value(gate)
    }
}

/// 按可用条件增删 `InteractionDisabled`
///
/// Feathers 见到该组件会切灰显样式并拦掉交互，与旧版 `add_enabled(false, ..)` 一致。
fn sync_button_gates(
    session: Res<super::Session>,
    editor: Res<super::Editor>,
    buttons: Query<(Entity, &ButtonGate, Has<InteractionDisabled>)>,
    mut commands: Commands,
) {
    if !session.is_changed() && !editor.is_changed() {
        return;
    }
    let idle = !session.is_busy();
    let has_pose = editor.has_pose_selection();
    for (entity, gate, disabled) in &buttons {
        let enabled = match gate {
            ButtonGate::Always => true,
            ButtonGate::WhenIdle => idle,
            ButtonGate::WhenIdleAndPose => idle && has_pose,
        };
        match (enabled, disabled) {
            (true, true) => {
                commands.entity(entity).remove::<InteractionDisabled>();
            }
            (false, false) => {
                commands.entity(entity).insert(InteractionDisabled);
            }
            _ => {}
        }
    }
}

/// 从命中实体向上找到带标记的那一层
///
/// UI 事件命中的往往是最内层的文字节点，而标记组件（`MenuAction`、`FileRow`、
/// `PoseCard` 之类）挂在外层容器上。直接拿命中实体去查会查不到，
/// observer 一 return 就成了"点了没反应"。
pub fn self_or_ancestor(
    entity: Entity,
    parents: &Query<&ChildOf>,
    has_marker: impl Fn(Entity) -> bool,
) -> Option<Entity> {
    core::iter::once(entity)
        .chain(parents.iter_ancestors(entity))
        .find(|&candidate| has_marker(candidate))
}

// ============================================================
// 插槽内容替换
// ============================================================

/// 插槽有一批内容正在等待 spawn
///
/// BSN 场景要等 asset 依赖加载完才落地，`queue_spawn_related_scenes` 因此可能跨帧。
/// 其间若再提交一批，`despawn_related` 只能删掉已挂上的子节点，删不掉排队中的那批，
/// 结果两批都挂上去——表现就是按钮、列表项凭空多出一份。
#[derive(Component)]
pub struct SlotPending;

/// 把插槽的子节点整体换成新内容
///
/// 保证同一时刻只有一批在飞：调用方先用 [`slot_is_pending`] 判断，
/// 上一批没落地就跳过本次重建（并且不要推进"已渲染版本号"，下一帧会自动重试）。
pub fn replace_slot_children(commands: &mut Commands, slot: Entity, scenes: Vec<BoxedScene>) {
    let mut entity = commands.entity(slot);
    entity.despawn_related::<Children>();
    // 空内容不会产生 Children 变化，挂上标记就没人来摘了
    if scenes.is_empty() {
        return;
    }
    entity
        .insert(SlotPending)
        .queue_spawn_related_scenes::<Children>(scenes);
}

/// 内容落地后摘掉等待标记
fn clear_slot_pending(
    slots: Query<Entity, (With<SlotPending>, Changed<Children>)>,
    mut commands: Commands,
) {
    for slot in &slots {
        commands.entity(slot).remove::<SlotPending>();
    }
}

/// 构件插件：注册折叠区块的交互与同步系统
pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_header_click).add_systems(
            Update,
            (sync_collapse_state, clear_slot_pending, sync_button_gates),
        );
    }
}
