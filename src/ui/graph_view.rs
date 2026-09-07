//! 流程图视图：把任务图的 nodes/edges 画成可下钻的分层流程图
//!
//! 位置由 [`super::graph_layout`] 算好，这里只负责画：
//! - 节点是绝对定位的卡片，左侧色条按类别着色
//! - 边是几段绝对定位的细矩形拼出的正交折线（Bevy UI 没有画线的原语，
//!   而正交折线本来就比曲线更适合流程图），终点补一个箭头字符
//! - 回边（`loop` 的循环体）走右侧通道，用强调色标出
//!
//! 流程本身只读——本工具编辑的是 context 参数，流程由机器人端定义。

use bevy::feathers::controls::ButtonVariant;
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemeToken};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::Interaction;
use bevy::ui_widgets::{Activate, ScrollArea};

use crate::model::{SubGraph, TaskNode};

use super::graph_layout::{self, GraphLayout, NODE_H, NODE_W, NodeSlot, PlacedEdge};
use super::shell::{GraphPane, GraphSlot, ParamsPane, ViewSwitchSlot};
use super::theme;
use super::widgets::{self, BoxedScene, boxed};
use super::{Editor, UiSet};

/// 折线粗细
const EDGE_W: f32 = 1.5;

/// 详情栏宽度
const DETAIL_W: f32 = 300.0;

/// 当前看的是哪个视图
#[derive(Resource, Default, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ViewMode {
    /// context 参数编辑
    #[default]
    Params,
    /// 任务流程图
    Graph,
}

/// 流程图的浏览状态
#[derive(Resource, Default)]
pub struct GraphNav {
    /// 下钻路径（节点 id 逐层）
    pub path: Vec<String>,
    /// 当前选中的节点
    pub selected: Option<String>,
    /// 版本号：路径或数据变化时递增，驱动重建
    pub version: u64,
}

impl GraphNav {
    /// 进入某个复合节点
    fn enter(&mut self, id: &str) {
        self.path.push(id.to_string());
        self.selected = None;
        self.version += 1;
    }

    /// 回到第 depth 层（0 为根）
    fn go_to(&mut self, depth: usize) {
        self.path.truncate(depth);
        self.selected = None;
        self.version += 1;
    }

    /// 换文件或重新加载后回到根
    fn reset(&mut self) {
        self.path.clear();
        self.selected = None;
        self.version += 1;
    }
}

/// 节点语义类别——颜色编码的是"这一步在做什么"，不是装饰
fn category_token(node_type: &str) -> ThemeToken {
    match node_type {
        "sequence" | "parallel" | "loop" | "condition" | "delay" => theme::GRAPH_FLOW,
        "ros2_action" | "behavior_tree" | "mock_action" | "preplan_navigation" | "leak_test"
        | "leak_test_prepare" | "area_guard" | "pause_task" => theme::GRAPH_ACT,
        "set_variable"
        | "copy_variable"
        | "compute"
        | "increment"
        | "compare"
        | "max_value"
        | "get_array_length"
        | "pop_array"
        | "peek_array"
        | "extract_pose_meta"
        | "extract_waist_height"
        | "set_waist_height"
        | "multi_pose_generator"
        | "get_leak_status" => theme::GRAPH_DATA,
        "log" | "publish_task_log" | "box_task_notify" => theme::GRAPH_LOG,
        _ => theme::GRAPH_DOMAIN,
    }
}

// ============================================================
// 标记组件
// ============================================================

/// 图上的一个节点，记录 id 供点击时定位
#[derive(Component, Clone, Default)]
struct GraphNodeMarker(String);

/// 图卡文字记录未选中时的角色，选中与取消时只切换 token。
#[derive(Component, Default, Clone)]
struct GraphNodeLabel(ThemeToken);

/// 详情栏插槽只更新选中节点，画布及其滚动位置保持不变。
#[derive(Component, Clone, Default)]
struct GraphDetailSlot {
    rendered: Option<(u64, Option<String>)>,
}

/// 画布刚建好、水平滚动还停在最左，等布局出尺寸后挪到中间
#[derive(Component, Clone, Default)]
struct CanvasNeedsCenter;

/// 面包屑上的一段，记录要回到第几层
#[derive(Component, Clone, Copy, Default)]
struct CrumbMarker(usize);

/// 「下钻」按钮
#[derive(Component, Clone, Default)]
struct DrillButton(String);

/// 视图切换按钮
#[derive(Component, Clone, Copy, Default)]
struct ViewToggle(bool);

/// 顶栏入口只生成一次，切换时更新组件而不销毁按钮与焦点。
#[derive(Component)]
struct ViewSwitchBuilt;

/// 已渲染的版本，避免每帧重建
#[derive(Resource, Default)]
struct RenderedGraph {
    /// 已渲染的浏览版本
    nav: Option<u64>,
    /// 已渲染时对应的数据结构版本
    structure: Option<u64>,
}

// ============================================================
// 场景构建
// ============================================================

/// 整个流程图面板
fn graph_pane(graph: &SubGraph, nav: &GraphNav) -> Vec<BoxedScene> {
    let Some(current) = graph.subgraph_at(&nav.path) else {
        return vec![boxed(widgets::hint("这一层已不存在，请返回上一层"))];
    };
    let placed = graph_layout::layout(current);
    vec![
        boxed(toolbar(graph, nav, current)),
        boxed(bsn! {
            Node {
                width: percent(100),
                flex_grow: 1.0,
                flex_direction: FlexDirection::Row,
                min_height: px(0),
            }
            Children [
                (canvas(current, &placed, nav)),
                (
                    Node {
                        display: Display::None,
                        width: {px(DETAIL_W)},
                        flex_shrink: 0.0,
                        height: percent(100),
                    }
                    GraphDetailSlot
                )
            ]
        }),
    ]
}

/// 顶部：面包屑 + 本层统计 + 图例
fn toolbar(graph: &SubGraph, nav: &GraphNav, current: &SubGraph) -> impl Scene {
    let mut crumbs: Vec<BoxedScene> = vec![boxed(crumb("根", 0, nav.path.is_empty()))];
    for (i, id) in nav.path.iter().enumerate() {
        crumbs.push(boxed(widgets::readonly_value("›")));
        crumbs.push(boxed(crumb(id, i + 1, i + 1 == nav.path.len())));
    }

    let composite = current
        .nodes
        .iter()
        .filter(|n| n.children.is_some())
        .count();
    let summary = format!(
        "本层 {} 节点 · {} 边 · 可下钻 {}　全图 {} 节点 / {} 层",
        current.nodes.len(),
        current.edges.len(),
        composite,
        graph.total_nodes(),
        graph.depth()
    );

    bsn! {
        Node {
            width: percent(100),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            row_gap: px(6),
            padding: {UiRect::all(px(theme::PAD))},
            border: {UiRect::bottom(px(1.0))},
        }
        ThemeBackgroundColor({theme::CARD_HEADER_BG})
        BorderColor::all(Color::NONE)
        Children [
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(4),
                    width: percent(100),
                    flex_wrap: {FlexWrap::Wrap},
                }
                Children [{crumbs}]
            ),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(14),
                    width: percent(100),
                    flex_wrap: {FlexWrap::Wrap},
                }
                Children [
                    (widgets::hint(summary)),
                    (legend()),
                    (widgets::hint("点击节点查看输入参数，再点一次进入子图"))
                ]
            )
        ]
    }
}

/// 面包屑的一段
fn crumb(label: &str, depth: usize, current: bool) -> impl Scene {
    let token = match current {
        true => theme::SECTION_TEXT,
        false => theme::FIELD_LABEL,
    };
    bsn! {
        Node {
            padding: {UiRect::axes(px(7.0), px(3.0))},
            border_radius: {BorderRadius::all(px(4.0))},
        }
        Button
        template_value(CrumbMarker(depth))
        ThemeBackgroundColor({match current {
            true => theme::BADGE_BG,
            false => theme::CARD_HEADER_BG,
        }})
        Children [(
            Text({label.to_string()})
            ThemeTextColor({token})
            TextFont { font_size: px(12.0) }
        )]
    }
}

/// 类别图例
fn legend() -> impl Scene {
    let items = [
        ("控制流", theme::GRAPH_FLOW),
        ("执行", theme::GRAPH_ACT),
        ("数据", theme::GRAPH_DATA),
        ("日志", theme::GRAPH_LOG),
        ("领域", theme::GRAPH_DOMAIN),
        ("循环回边", theme::GRAPH_BACK),
    ];
    let chips: Vec<BoxedScene> = items
        .into_iter()
        .map(|(label, token)| {
            boxed(bsn! {
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(4),
                }
                Children [
                    (
                        Node {
                            width: px(9),
                            height: px(3),
                            border_radius: {BorderRadius::all(px(1.5))},
                            flex_shrink: 0.0,
                        }
                        ThemeBackgroundColor({token})
                    ),
                    (widgets::hint(label)),
                ]
            })
        })
        .collect();
    widgets::row(12.0, chips)
}

/// 画布：节点与边都绝对定位在同一个坐标系里
fn canvas(current: &SubGraph, placed: &GraphLayout, nav: &GraphNav) -> impl Scene {
    let mut items: Vec<BoxedScene> = Vec::new();
    // 边先入，压在节点下面
    for e in &placed.edges {
        items.extend(edge_scene(e));
    }
    for p in &placed.nodes {
        let node = current.node(&p.id);
        items.push(boxed(node_scene(
            p,
            node,
            nav.selected.as_deref() == Some(p.id.as_str()),
        )));
    }

    let (w, h) = (placed.size.x, placed.size.y);
    bsn! {
        Node {
            flex_grow: 1.0,
            height: percent(100),
            min_width: px(0),
            overflow: {Overflow::scroll()},
        }
        ScrollArea
        CanvasNeedsCenter
        Children [(
            Node {
                width: {px(w)},
                height: {px(h)},
                flex_shrink: 0.0,
            }
            Children [{items}]
        )]
    }
}

/// 一个节点卡片
fn node_scene(
    placed: &graph_layout::PlacedNode,
    node: Option<&TaskNode>,
    selected: bool,
) -> BoxedScene {
    // 虚拟入口 / 出口：只画一个淡淡的端点
    let Some(node) = node else {
        let label = match placed.slot {
            NodeSlot::Entry => "入口",
            _ => "出口",
        };
        return boxed(bsn! {
            Node {
                position_type: PositionType::Absolute,
                left: {px(placed.pos.x)},
                top: {px(placed.pos.y)},
                width: {px(NODE_W)},
                height: {px(NODE_H)},
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: {UiRect::all(px(1.0))},
                border_radius: {BorderRadius::all(px(theme::RADIUS_SM))},
            }
            ThemeBackgroundColor({theme::CARD_HEADER_BG})
            ThemeBorderColor({theme::GRAPH_BORDER})
            Children [(widgets::readonly_value(label))]
        });
    };

    let kids = node.children.as_ref().map(|c| c.nodes.len()).unwrap_or(0);
    let badge: Vec<BoxedScene> = (kids > 0)
        .then(|| boxed(widgets::badge(format!("{kids} ▸"))))
        .into_iter()
        .collect();
    let ckpt: Vec<BoxedScene> = node
        .checkpoint
        .then(|| {
            boxed(bsn! {
                Node {
                    width: px(7),
                    height: px(7),
                    border_radius: {BorderRadius::all(px(4.0))},
                    flex_shrink: 0.0,
                }
                ThemeBackgroundColor({theme::CHECKPOINT_DOT})
            })
        })
        .into_iter()
        .collect();

    let border = match selected {
        true => theme::GRAPH_SELECTED_BORDER,
        false => theme::GRAPH_BORDER,
    };
    let background = match selected {
        true => theme::GRAPH_SELECTED_BG,
        false => theme::CARD_BG,
    };

    boxed(bsn! {
        Node {
            position_type: PositionType::Absolute,
            left: {px(placed.pos.x)},
            top: {px(placed.pos.y)},
            width: {px(NODE_W)},
            height: {px(NODE_H)},
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(7),
            padding: {UiRect::axes(px(7.0), px(6.0))},
            border: {UiRect::all(px(1.0))},
            border_radius: {BorderRadius::all(px(theme::RADIUS_SM))},
            overflow: {Overflow::clip()},
        }
        Button
        template_value(GraphNodeMarker(node.id.clone()))
        ThemeBackgroundColor({background})
        ThemeBorderColor({border})
        Children [
            (
                Node {
                    width: px(3),
                    height: percent(76),
                    border_radius: {BorderRadius::all(px(1.5))},
                    flex_shrink: 0.0,
                }
                ThemeBackgroundColor({category_token(&node.node_type)})
            ),
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    min_width: px(0),
                    overflow: {Overflow::clip()},
                }
                Children [
                    (
                        Text({node.id.clone()})
                        GraphNodeLabel({theme::SECTION_TEXT})
                        ThemeTextColor({theme::SECTION_TEXT})
                        TextFont { font_size: px(11.5) }
                        TextLayout { linebreak: {LineBreak::AnyCharacter} }
                    ),
                    (
                        Text({node.node_type.clone()})
                        GraphNodeLabel({theme::READONLY_TEXT})
                        ThemeTextColor({theme::READONLY_TEXT})
                        TextFont { font_size: px(10.0) }
                    )
                ]
            ),
            {ckpt},
            {badge}
        ]
    })
}

/// 一条边：折线拆成若干水平/垂直的细矩形，终点补箭头
fn edge_scene(edge: &PlacedEdge) -> Vec<BoxedScene> {
    let token = match edge.back {
        true => theme::GRAPH_BACK,
        false => theme::GRAPH_BORDER,
    };
    let mut parts: Vec<BoxedScene> = edge
        .points
        .windows(2)
        .map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            let left = a.x.min(b.x) - EDGE_W / 2.0;
            let top = a.y.min(b.y) - EDGE_W / 2.0;
            let width = (a.x - b.x).abs().max(EDGE_W);
            let height = (a.y - b.y).abs().max(EDGE_W);
            boxed(bsn! {
                Node {
                    position_type: PositionType::Absolute,
                    left: {px(left)},
                    top: {px(top)},
                    width: {px(width)},
                    height: {px(height)},
                }
                ThemeBackgroundColor({token.clone()})
            })
        })
        .collect();

    // 箭头落在终点，方向取最后一段
    if let (Some(&end), Some(&prev)) = (
        edge.points.last(),
        edge.points.get(edge.points.len().wrapping_sub(2)),
    ) {
        let horizontal = (end.y - prev.y).abs() < f32::EPSILON;
        let (glyph, dx, dy) = match (horizontal, end.x < prev.x, end.y < prev.y) {
            (true, true, _) => ("◀", -9.0, -6.0),
            (true, false, _) => ("▶", -3.0, -6.0),
            (false, _, true) => ("▲", -5.0, -9.0),
            (false, _, false) => ("▼", -5.0, -4.0),
        };
        parts.push(boxed(bsn! {
            Node {
                position_type: PositionType::Absolute,
                left: {px(end.x + dx)},
                top: {px(end.y + dy)},
            }
            Text(glyph)
            ThemeTextColor({token})
            TextFont { font_size: px(9.0) }
        }));
    }
    parts
}

/// 右侧详情：选中节点的输入参数
fn detail_panel(node: &TaskNode) -> impl Scene {
    let body: Vec<BoxedScene> = {
        let mut rows: Vec<BoxedScene> = vec![
            boxed(wrapping_title(node.id.clone())),
            boxed(widgets::row(
                6.0,
                vec![
                    boxed(bsn! {
                        Node {
                            width: px(9),
                            height: px(9),
                            border_radius: {BorderRadius::all(px(2.0))},
                            flex_shrink: 0.0,
                        }
                        ThemeBackgroundColor({category_token(&node.node_type)})
                    }),
                    boxed(widgets::readonly_value(node.node_type.clone())),
                ],
            )),
        ];
        if node.checkpoint {
            rows.push(boxed(widgets::badge("checkpoint")));
        }
        rows.push(boxed(bsn! {
            Node {
                width: percent(100),
                height: px(1),
                margin: {UiRect::vertical(px(4.0))},
                flex_shrink: 0.0,
            }
            ThemeBackgroundColor({theme::DIVIDER})
        }));

        match node.inputs.as_object() {
            Some(map) if !map.is_empty() => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for k in keys {
                    let raw = match &map[k] {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    rows.push(boxed(detail_row(k, clip(&raw, 300))));
                }
            }
            _ => rows.push(boxed(widgets::hint("该节点没有输入参数"))),
        }

        if let Some(children) = &node.children {
            rows.push(boxed(widgets::button(
                format!("进入子图 · {} 节点", children.nodes.len()),
                ButtonVariant::Primary,
                DrillButton(node.id.clone()),
            )));
        }
        rows
    };

    bsn! {
        Node {
            width: {px(DETAIL_W)},
            flex_shrink: 0.0,
            height: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(6),
            padding: {UiRect::all(px(theme::PAD))},
            border: {UiRect::left(px(1.0))},
            overflow: {Overflow::scroll_y()},
        }
        ScrollArea
        ThemeBackgroundColor({theme::SIDEBAR_BG})
        BorderColor::all(Color::NONE)
        Children [{body}]
    }
}

/// 截断过长文本
/// 详情栏标题：节点 id 是无空格标识符，必须允许逐字符断行，否则顶破面板宽度
fn wrapping_title(text: impl Into<String>) -> impl Scene {
    bsn! {
        Text({text.into()})
        ThemeTextColor({theme::SECTION_TEXT})
        TextFont {
            font_size: px(12.0),
            weight: {FontWeight::BOLD}
        }
        TextLayout { linebreak: {LineBreak::AnyCharacter} }
    }
}

/// 详情栏的一行输入参数：标签在上、值在下
///
/// 不复用 [`widgets::field_row`]——那是给参数编辑器用的横排布局，
/// 标签列固定 190px，塞进 300px 的详情栏只剩一条缝给值。
fn detail_row(key: impl Into<String>, value: impl Into<String>) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            row_gap: px(1),
            padding: {UiRect::vertical(px(3.0))},
        }
        Children [
            (
                Text({key.into()})
                ThemeTextColor({theme::FIELD_LABEL})
                TextFont { font_size: px(10.5) }
                TextLayout { linebreak: {LineBreak::AnyCharacter} }
            ),
            (
                Text({value.into()})
                ThemeTextColor({theme::READONLY_TEXT})
                TextFont { font_size: px(11.5) }
                TextLayout { linebreak: {LineBreak::AnyCharacter} }
            )
        ]
    }
}

fn clip(text: &str, max: usize) -> String {
    match text.chars().count() > max {
        true => text.chars().take(max - 1).chain(['…']).collect(),
        false => text.to_string(),
    }
}

// ============================================================
// 系统
// ============================================================

/// 切换视图时改两个面板的显示
fn sync_view_mode(
    mode: Res<ViewMode>,
    mut params: Query<&mut Node, (With<ParamsPane>, Without<GraphPane>)>,
    mut graph: Query<&mut Node, (With<GraphPane>, Without<ParamsPane>)>,
) {
    // 面板通过 BSN 跨帧生成，可能晚于视图变化落地；按值同步同时覆盖首次生成。
    let (p, g) = match *mode {
        ViewMode::Params => (Display::Flex, Display::None),
        ViewMode::Graph => (Display::None, Display::Flex),
    };
    for mut node in &mut params {
        if node.display != p {
            node.display = p;
        }
    }
    for mut node in &mut graph {
        if node.display != g {
            node.display = g;
        }
    }
}

/// 换文件后回到根层
fn reset_on_reload(
    editor: Res<Editor>,
    mut nav: ResMut<GraphNav>,
    mut rendered: ResMut<RenderedGraph>,
) {
    if rendered.structure == Some(editor.structure_version) {
        return;
    }
    rendered.structure = Some(editor.structure_version);
    nav.reset();
}

/// 按浏览状态重建流程图
fn rebuild_graph(
    editor: Res<Editor>,
    nav: Res<GraphNav>,
    mut rendered: ResMut<RenderedGraph>,
    slots: Query<Entity, With<GraphSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    let Ok(slot) = slots.single() else {
        return;
    };
    if rendered.nav == Some(nav.version) {
        return;
    }
    if pending.contains(slot) {
        return;
    }
    rendered.nav = Some(nav.version);

    let content = match &editor.data {
        Some(data) => graph_pane(&data.graph, &nav),
        None => vec![boxed(widgets::hint("请先从左侧选择一个文件"))],
    };
    widgets::replace_slot_children(&mut commands, slot, content);
}

/// 选中变化只改颜色，保留画布和输入状态；跨帧新增的图卡与文字也同步当前选择。
fn sync_node_selection(
    nav: Res<GraphNav>,
    nodes: Query<(Entity, Ref<GraphNodeMarker>)>,
    labels: Query<(Entity, Ref<GraphNodeLabel>)>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for (entity, marker) in &nodes {
        if !nav.is_changed() && !marker.is_added() {
            continue;
        }
        let (background, border) = match nav.selected.as_deref() == Some(marker.0.as_str()) {
            true => (theme::GRAPH_SELECTED_BG, theme::GRAPH_SELECTED_BORDER),
            false => (theme::CARD_BG, theme::GRAPH_BORDER),
        };
        commands
            .entity(entity)
            .insert((ThemeBackgroundColor(background), ThemeBorderColor(border)));
    }
    for (entity, label) in &labels {
        if !nav.is_changed() && !label.is_added() {
            continue;
        }
        let Some(card) =
            widgets::self_or_ancestor(entity, &parents, |parent| nodes.contains(parent))
        else {
            continue;
        };
        let Ok((_, marker)) = nodes.get(card) else {
            continue;
        };
        let color = match nav.selected.as_deref() == Some(marker.0.as_str()) {
            true => theme::GRAPH_SELECTED_TEXT,
            false => label.0.clone(),
        };
        commands.entity(entity).insert(ThemeTextColor(color));
    }
}

/// 独立替换详情内容，不重建拥有 ScrollPosition 的画布。
fn rebuild_detail(
    editor: Res<Editor>,
    nav: Res<GraphNav>,
    mut slots: Query<(Entity, &mut GraphDetailSlot, &mut Node)>,
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    let state = (nav.version, nav.selected.clone());
    for (entity, mut slot, mut panel) in &mut slots {
        if slot.rendered.as_ref() == Some(&state) || pending.contains(entity) {
            continue;
        }
        let selected = editor.data.as_ref().and_then(|data| {
            data.graph
                .subgraph_at(&nav.path)
                .and_then(|graph| nav.selected.as_deref().and_then(|id| graph.node(id)))
        });
        let content = match selected {
            Some(node) => {
                panel.display = Display::Flex;
                vec![boxed(detail_panel(node))]
            }
            None => {
                panel.display = Display::None;
                Vec::new()
            }
        };
        widgets::replace_slot_children(&mut commands, entity, content);
        slot.rendered = Some(state.clone());
    }
}

/// 点击节点：选中；再点一次带子图的节点则下钻
fn handle_node_press(
    nodes: Query<(&Interaction, &GraphNodeMarker), Changed<Interaction>>,
    editor: Res<Editor>,
    mut nav: ResMut<GraphNav>,
) {
    for (state, marker) in &nodes {
        if *state != Interaction::Pressed {
            continue;
        }
        let already = nav.selected.as_deref() == Some(marker.0.as_str());
        let composite = editor.data.as_ref().is_some_and(|d| {
            d.graph
                .subgraph_at(&nav.path)
                .and_then(|g| g.node(&marker.0))
                .is_some_and(|n| n.children.is_some())
        });
        // 选中态下再点一次复合节点就进去，省得非要点右边那个按钮
        match already && composite {
            true => nav.enter(&marker.0),
            false => {
                nav.selected = Some(marker.0.clone());
            }
        }
    }
}

/// 面包屑使用原生 UI Button，继续由 Interaction 驱动。
fn handle_nav_press(
    crumbs: Query<(&Interaction, &CrumbMarker), Changed<Interaction>>,
    mut nav: ResMut<GraphNav>,
) {
    for (state, crumb) in &crumbs {
        if *state == Interaction::Pressed {
            nav.go_to(crumb.0);
        }
    }
}

/// 详情中的下钻入口是 FeathersButton，鼠标和键盘统一通过 Activate 触发。
fn handle_drill_activate(
    event: On<Activate>,
    drills: Query<&DrillButton>,
    editor: Res<Editor>,
    mut nav: ResMut<GraphNav>,
) {
    let Ok(drill) = drills.get(event.entity) else {
        return;
    };
    let exists = editor.data.as_ref().is_some_and(|data| {
        data.graph
            .subgraph_at(&nav.path)
            .and_then(|graph| graph.node(&drill.0))
            .is_some_and(|node| node.children.is_some())
    });
    if exists {
        nav.enter(&drill.0);
    }
}

/// 把画布的水平滚动挪到内容中线
///
/// 分层布局把每一层都对齐到内容中线，而滚动位置默认停在最左：
/// 内容比视口宽时主干会被推到右边缘，右侧的回边通道整条看不见。
/// 布局尺寸要等 `ui_layout_system` 跑完才有，所以靠标记组件轮询，量到了就居中并摘掉标记。
fn center_canvas(
    mut canvas: Query<(Entity, &ComputedNode, &mut ScrollPosition), With<CanvasNeedsCenter>>,
    mut commands: Commands,
) {
    for (entity, computed, mut scroll) in &mut canvas {
        let view = computed.size().x;
        let content = computed.content_size().x;
        if view <= 0.0 || content <= 0.0 {
            continue;
        }
        // ComputedNode 是物理像素，ScrollPosition 是逻辑像素
        scroll.0.x = ((content - view) / 2.0).max(0.0) * computed.inverse_scale_factor;
        commands.entity(entity).remove::<CanvasNeedsCenter>();
    }
}

/// FeathersButton 没有旧 Interaction 组件，实际点击及键盘触发均发出 Activate。
fn handle_view_toggle(
    event: On<Activate>,
    toggles: Query<&ViewToggle>,
    mut variants: Query<(&ViewToggle, &mut ButtonVariant)>,
    mut mode: ResMut<ViewMode>,
) {
    let Ok(toggle) = toggles.get(event.entity) else {
        return;
    };
    let want = match toggle.0 {
        true => ViewMode::Graph,
        false => ViewMode::Params,
    };
    mode.set_if_neq(want);
    // picking 的 observer 在 PreUpdate 中触发；提前写 variant，让同帧 Feathers 样式同步可见。
    update_view_variants(want, &mut variants);
}

fn update_view_variants(mode: ViewMode, toggles: &mut Query<(&ViewToggle, &mut ButtonVariant)>) {
    for (toggle, mut variant) in toggles.iter_mut() {
        let selected = matches!(
            (toggle.0, mode),
            (true, ViewMode::Graph) | (false, ViewMode::Params)
        );
        variant.set_if_neq(match selected {
            true => ButtonVariant::Primary,
            false => ButtonVariant::Normal,
        });
    }
}

/// 处理程序切换与跨帧刚生成的按钮，选中样式只更新组件。
fn sync_view_switch(mode: Res<ViewMode>, mut toggles: Query<(&ViewToggle, &mut ButtonVariant)>) {
    update_view_variants(*mode, &mut toggles);
}

/// 入口始终可见；未加载文档时切到流程图显示明确的加载提示。
fn build_view_switch(
    mode: Res<ViewMode>,
    slots: Query<Entity, (With<ViewSwitchSlot>, Without<ViewSwitchBuilt>)>,
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    for slot in &slots {
        if pending.contains(slot) {
            continue;
        }
        widgets::replace_slot_children(&mut commands, slot, view_switch(*mode));
        commands.entity(slot).insert(ViewSwitchBuilt);
    }
}

/// 视图切换按钮组，供顶栏使用
fn view_switch(current: ViewMode) -> Vec<BoxedScene> {
    let variant = |on: bool| match on {
        true => ButtonVariant::Primary,
        false => ButtonVariant::Normal,
    };
    vec![
        boxed(widgets::button(
            "参数",
            variant(current == ViewMode::Params),
            ViewToggle(false),
        )),
        boxed(widgets::button(
            "流程图",
            variant(current == ViewMode::Graph),
            ViewToggle(true),
        )),
    ]
}

/// 流程图视图插件
pub struct GraphViewPlugin;

impl Plugin for GraphViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewMode>()
            .init_resource::<GraphNav>()
            .init_resource::<RenderedGraph>()
            .add_observer(handle_view_toggle)
            .add_observer(handle_drill_activate)
            .add_systems(
                Update,
                (handle_node_press, handle_nav_press).in_set(UiSet::Input),
            )
            .add_systems(
                Update,
                (
                    reset_on_reload,
                    rebuild_graph,
                    sync_node_selection,
                    rebuild_detail,
                    build_view_switch,
                    sync_view_switch,
                    sync_view_mode,
                    center_canvas,
                )
                    .chain()
                    .in_set(UiSet::Rebuild),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_editor() -> Editor {
        Editor {
            data: Some(crate::model::parse_task_graph(
                r#"{"map_id":"m","task_id":"t","config":{"context":{},"nodes":[{"id":"step","type":"sequence","nodes":[{"id":"child","type":"log"}],"edges":[]}],"edges":[]}}"#,
            ).unwrap()),
            ..default()
        }
    }

    #[test]
    fn 选中节点保留画布实体和双轴滚动位置() {
        let mut app = App::new();
        app.insert_resource(test_editor())
            .init_resource::<GraphNav>()
            .insert_resource(RenderedGraph {
                nav: Some(0),
                ..default()
            })
            .add_systems(
                Update,
                (handle_node_press, rebuild_graph, sync_node_selection).chain(),
            );
        let slot = app.world_mut().spawn(GraphSlot).id();
        let scroll = Vec2::new(120.0, 1600.0);
        let canvas = app
            .world_mut()
            .spawn((ScrollPosition(scroll), ChildOf(slot)))
            .id();
        let card = app
            .world_mut()
            .spawn((
                GraphNodeMarker("step".into()),
                Interaction::None,
                ThemeBorderColor(theme::GRAPH_BORDER),
                ChildOf(canvas),
            ))
            .id();
        app.update();
        *app.world_mut().get_mut::<Interaction>(card).unwrap() = Interaction::Pressed;
        app.update();

        assert_eq!(app.world().resource::<GraphNav>().version, 0);
        assert_eq!(
            app.world().resource::<GraphNav>().selected.as_deref(),
            Some("step")
        );
        assert_eq!(app.world().get::<ScrollPosition>(canvas).unwrap().0, scroll);
        assert_eq!(app.world().get::<ChildOf>(card).unwrap().parent(), canvas);
        assert_eq!(
            app.world().get::<ThemeBorderColor>(card).unwrap().0,
            theme::GRAPH_SELECTED_BORDER
        );
        assert_eq!(
            app.world().get::<ThemeBackgroundColor>(card).unwrap().0,
            theme::GRAPH_SELECTED_BG
        );
        // 标签可能比卡片晚生成，随后取消选择应恢复各自的原始文字角色。
        let title = app
            .world_mut()
            .spawn((GraphNodeLabel(theme::SECTION_TEXT), ChildOf(card)))
            .id();
        let subtitle = app
            .world_mut()
            .spawn((GraphNodeLabel(theme::READONLY_TEXT), ChildOf(card)))
            .id();
        app.update();
        for label in [title, subtitle] {
            assert_eq!(
                app.world().get::<ThemeTextColor>(label).unwrap().0,
                theme::GRAPH_SELECTED_TEXT
            );
        }
        app.world_mut().resource_mut::<GraphNav>().selected = None;
        app.update();
        assert_eq!(
            app.world().get::<ThemeBackgroundColor>(card).unwrap().0,
            theme::CARD_BG
        );
        assert_eq!(
            app.world().get::<ThemeBorderColor>(card).unwrap().0,
            theme::GRAPH_BORDER
        );
        assert_eq!(
            app.world().get::<ThemeTextColor>(title).unwrap().0,
            theme::SECTION_TEXT
        );
        assert_eq!(
            app.world().get::<ThemeTextColor>(subtitle).unwrap().0,
            theme::READONLY_TEXT
        );
        assert!(app.world().get::<widgets::SlotPending>(slot).is_none());
        assert!(app.world().get::<CanvasNeedsCenter>(canvas).is_none());
    }

    #[test]
    fn 再次点击已选中复合节点才触发下钻重建() {
        let mut app = App::new();
        app.insert_resource(test_editor())
            .init_resource::<GraphNav>()
            .add_systems(Update, handle_node_press);
        let card = app
            .world_mut()
            .spawn((GraphNodeMarker("step".into()), Interaction::Pressed))
            .id();
        app.update();
        assert_eq!(app.world().resource::<GraphNav>().version, 0);
        *app.world_mut().get_mut::<Interaction>(card).unwrap() = Interaction::None;
        app.update();
        *app.world_mut().get_mut::<Interaction>(card).unwrap() = Interaction::Pressed;
        app.update();
        let nav = app.world().resource::<GraphNav>();
        assert_eq!(nav.path, ["step"]);
        assert!(nav.selected.is_none());
        assert_eq!(nav.version, 1);
    }

    #[test]
    fn 延迟生成的卡片立即使用当前选中样式() {
        let mut app = App::new();
        app.insert_resource(GraphNav {
            selected: Some("step".into()),
            ..default()
        })
        .add_systems(Update, sync_node_selection);
        app.update();
        let card = app.world_mut().spawn(GraphNodeMarker("step".into())).id();
        app.update();
        assert_eq!(
            app.world().get::<ThemeBorderColor>(card).unwrap().0,
            theme::GRAPH_SELECTED_BORDER
        );
    }

    /// 真实 BSN/Feathers 按钮和输入分发；不创建原生窗口，不读取用户配置。
    fn graph_app(editor: Editor) -> App {
        use bevy::input::InputPlugin;
        use bevy::input_focus::{InputDispatchPlugin, InputFocus};
        use bevy::scene::ScenePlugin;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            ScenePlugin,
            InputPlugin,
            InputDispatchPlugin,
            bevy::ui_widgets::ButtonPlugin,
        ))
        .init_asset::<Font>()
        .init_resource::<InputFocus>()
        .insert_resource(editor)
        .insert_resource(super::super::Session::new(
            crate::model::LoginConfig::default(),
            Vec::new(),
        ))
        .configure_sets(
            Update,
            (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
        )
        .add_plugins((widgets::WidgetsPlugin, GraphViewPlugin));
        app.world_mut()
            .spawn((Window::default(), bevy::window::PrimaryWindow));
        app
    }

    fn settle_scenes(app: &mut App) {
        for _ in 0..4 {
            app.update();
        }
    }

    fn view_button(app: &mut App, graph: bool) -> Entity {
        app.world_mut()
            .query::<(Entity, &ViewToggle)>()
            .iter(app.world())
            .find(|(_, marker)| marker.0 == graph)
            .unwrap()
            .0
    }

    fn click_caption(app: &mut App, button: Entity) {
        use bevy::picking::pointer::{Location, PointerButton, PointerId};
        let entity = app.world().get::<Children>(button).unwrap()[0];
        let location = Location {
            target: bevy::camera::NormalizedRenderTarget::None {
                width: 1,
                height: 1,
            },
            position: Vec2::ZERO,
        };
        let hit = bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None);
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location.clone(),
            Press {
                button: PointerButton::Primary,
                hit: hit.clone(),
                count: 1,
            },
            entity,
        ));
        app.world_mut().flush();
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location.clone(),
            Click {
                button: PointerButton::Primary,
                hit: hit.clone(),
                count: 1,
                duration: std::time::Duration::ZERO,
            },
            entity,
        ));
        app.world_mut().flush();
        app.world_mut().trigger(Pointer::new(
            PointerId::Mouse,
            location,
            Release {
                button: PointerButton::Primary,
                hit,
            },
            entity,
        ));
        app.world_mut().flush();
    }

    #[test]
    fn 真实feathers流程图按钮激活后同帧切换视图() {
        let mut app = graph_app(test_editor());
        let params = app.world_mut().spawn((ParamsPane, Node::default())).id();
        let graph = app
            .world_mut()
            .spawn((
                GraphPane,
                Node {
                    display: Display::None,
                    ..default()
                },
            ))
            .id();
        let button = app
            .world_mut()
            .spawn_scene(widgets::button(
                "流程图",
                ButtonVariant::Normal,
                ViewToggle(true),
            ))
            .unwrap()
            .id();
        settle_scenes(&mut app);
        assert!(
            app.world()
                .get::<bevy::ui_widgets::Button>(button)
                .is_some()
        );
        assert!(
            app.world().get::<Interaction>(button).is_none(),
            "Feathers按钮没有旧Interaction组件"
        );
        app.world_mut().trigger(Activate { entity: button });
        app.update();
        assert_eq!(*app.world().resource::<ViewMode>(), ViewMode::Graph);
        assert_eq!(
            app.world().get::<Node>(params).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(graph).unwrap().display,
            Display::Flex
        );
    }

    #[test]
    fn 无文档时点击按钮文字仍能切换并显示加载提示() {
        let mut app = graph_app(Editor::default());
        app.world_mut().spawn((ViewSwitchSlot, Node::default()));
        let params = app.world_mut().spawn((ParamsPane, Node::default())).id();
        let graph = app
            .world_mut()
            .spawn((
                GraphPane,
                GraphSlot,
                Node {
                    display: Display::None,
                    ..default()
                },
            ))
            .id();
        settle_scenes(&mut app);
        let button = view_button(&mut app, true);
        click_caption(&mut app, button);
        app.update();
        assert_eq!(*app.world().resource::<ViewMode>(), ViewMode::Graph);
        assert_eq!(
            app.world().get::<Node>(params).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(graph).unwrap().display,
            Display::Flex
        );
        assert!(
            app.world_mut()
                .query::<&Text>()
                .iter(app.world())
                .any(|text| text.0 == "请先从左侧选择一个文件")
        );
        assert_eq!(view_button(&mut app, true), button, "点击不重建按钮");
        assert_eq!(
            *app.world().get::<ButtonVariant>(button).unwrap(),
            ButtonVariant::Primary
        );

        app.world_mut()
            .resource_mut::<Editor>()
            .load(test_editor().data);
        settle_scenes(&mut app);
        assert_eq!(
            view_button(&mut app, true),
            button,
            "加载文档也保持入口实体"
        );
        assert!(
            app.world_mut()
                .query::<&GraphNodeMarker>()
                .iter(app.world())
                .any(|node| node.0 == "step")
        );
    }

    #[test]
    fn 回车与空格通过真实键盘分发切换且保留按钮焦点() {
        use bevy::input::{
            ButtonState,
            keyboard::{Key, KeyboardInput},
        };
        use bevy::input_focus::{FocusCause, InputFocus};
        let mut app = graph_app(test_editor());
        app.world_mut().spawn((ViewSwitchSlot, Node::default()));
        settle_scenes(&mut app);
        let graph_button = view_button(&mut app, true);
        let params_button = view_button(&mut app, false);
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        for (button, key_code, logical_key, wanted) in [
            (graph_button, KeyCode::Enter, Key::Enter, ViewMode::Graph),
            (params_button, KeyCode::Space, Key::Space, ViewMode::Params),
        ] {
            app.world_mut()
                .resource_mut::<InputFocus>()
                .set(button, FocusCause::Navigated);
            app.world_mut().write_message(KeyboardInput {
                key_code,
                logical_key,
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window,
            });
            app.update();
            assert_eq!(*app.world().resource::<ViewMode>(), wanted);
            assert_eq!(app.world().resource::<InputFocus>().get(), Some(button));
            assert_eq!(
                *app.world().get::<ButtonVariant>(button).unwrap(),
                ButtonVariant::Primary
            );
        }
        assert_eq!(view_button(&mut app, true), graph_button);
        assert_eq!(view_button(&mut app, false), params_button);
    }

    #[test]
    fn 迟到的bsn面板按已经选定的视图初始化() {
        let mut app = graph_app(Editor::default());
        *app.world_mut().resource_mut::<ViewMode>() = ViewMode::Graph;
        app.update();
        let params = app.world_mut().spawn((ParamsPane, Node::default())).id();
        let graph = app
            .world_mut()
            .spawn((
                GraphPane,
                Node {
                    display: Display::None,
                    ..default()
                },
            ))
            .id();
        app.world_mut().spawn((ViewSwitchSlot, Node::default()));
        settle_scenes(&mut app);
        assert_eq!(
            app.world().get::<Node>(params).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(graph).unwrap().display,
            Display::Flex
        );
        let button = view_button(&mut app, true);
        assert_eq!(
            *app.world().get::<ButtonVariant>(button).unwrap(),
            ButtonVariant::Primary
        );
    }

    #[test]
    fn 详情feathers按钮文字可下钻且原生面包屑仍能返回() {
        let mut app = graph_app(test_editor());
        let button = app
            .world_mut()
            .spawn_scene(widgets::button(
                "进入子图",
                ButtonVariant::Primary,
                DrillButton("step".into()),
            ))
            .unwrap()
            .id();
        let root_crumb = app
            .world_mut()
            .spawn_scene(crumb("根", 0, false))
            .unwrap()
            .id();
        settle_scenes(&mut app);
        assert!(app.world().get::<Interaction>(button).is_none());
        click_caption(&mut app, button);
        app.update();
        assert_eq!(app.world().resource::<GraphNav>().path, ["step"]);
        *app.world_mut().get_mut::<Interaction>(root_crumb).unwrap() = Interaction::Pressed;
        app.update();
        assert!(app.world().resource::<GraphNav>().path.is_empty());
    }
}
