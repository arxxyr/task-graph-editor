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
use bevy::ui_widgets::ScrollArea;

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
#[derive(Resource, Default, PartialEq, Eq, Clone, Copy)]
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

/// 已渲染的版本，避免每帧重建
#[derive(Resource, Default)]
struct RenderedGraph {
    /// 已渲染的浏览版本
    nav: Option<u64>,
    /// 已渲染时对应的数据结构版本
    structure: Option<u64>,
    /// 已渲染的切换按钮状态（有数据、当前视图）
    switch: Option<(bool, ViewMode)>,
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
    // 没选中节点时不占那 300px——留着也只是显示一句提示，画布本来就不够宽
    let detail: Vec<BoxedScene> = match nav.selected.as_deref().and_then(|id| current.node(id)) {
        Some(node) => vec![boxed(detail_panel(node))],
        None => Vec::new(),
    };

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
                {detail}
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

    // 详情栏只在选中后才出现，提示得放这儿，否则没人知道节点可以点
    let tip: Vec<BoxedScene> = match nav.selected.is_none() {
        true => vec![boxed(widgets::hint(
            "点击节点查看输入参数，再点一次进入子图",
        ))],
        false => Vec::new(),
    };

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
                    {tip}
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
            ThemeBorderColor({theme::CARD_BORDER})
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
        true => theme::POSE_SELECTED_BORDER,
        false => theme::CARD_BORDER,
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
        ThemeBackgroundColor({theme::CARD_BG})
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
                        ThemeTextColor({theme::SECTION_TEXT})
                        TextFont { font_size: px(11.5) }
                        TextLayout { linebreak: {LineBreak::AnyCharacter} }
                    ),
                    (
                        Text({node.node_type.clone()})
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
        false => theme::CARD_BORDER,
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
    if !mode.is_changed() {
        return;
    }
    let (p, g) = match *mode {
        ViewMode::Params => (Display::Flex, Display::None),
        ViewMode::Graph => (Display::None, Display::Flex),
    };
    for mut node in &mut params {
        node.display = p;
    }
    for mut node in &mut graph {
        node.display = g;
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
                nav.version += 1;
            }
        }
    }
}

/// 面包屑与下钻按钮
fn handle_nav_press(
    crumbs: Query<(&Interaction, &CrumbMarker), Changed<Interaction>>,
    drills: Query<(&Interaction, &DrillButton), Changed<Interaction>>,
    mut nav: ResMut<GraphNav>,
) {
    for (state, crumb) in &crumbs {
        if *state == Interaction::Pressed {
            nav.go_to(crumb.0);
        }
    }
    for (state, drill) in &drills {
        if *state == Interaction::Pressed {
            nav.enter(&drill.0);
        }
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

/// 顶栏的视图切换按钮
fn handle_view_toggle(
    toggles: Query<(&Interaction, &ViewToggle), Changed<Interaction>>,
    mut mode: ResMut<ViewMode>,
) {
    for (state, toggle) in &toggles {
        if *state != Interaction::Pressed {
            continue;
        }
        let want = match toggle.0 {
            true => ViewMode::Graph,
            false => ViewMode::Params,
        };
        if *mode != want {
            *mode = want;
        }
    }
}

/// 只在有数据时露出切换按钮，并跟随当前视图高亮
fn rebuild_view_switch(
    editor: Res<Editor>,
    mode: Res<ViewMode>,
    mut rendered: ResMut<RenderedGraph>,
    slots: Query<Entity, With<ViewSwitchSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    let state = (editor.data.is_some(), *mode);
    if rendered.switch == Some(state) {
        return;
    }
    let Ok(slot) = slots.single() else {
        return;
    };
    if pending.contains(slot) {
        return;
    }
    rendered.switch = Some(state);
    let content = match state.0 {
        true => view_switch(state.1),
        false => Vec::new(),
    };
    widgets::replace_slot_children(&mut commands, slot, content);
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
            .add_systems(
                Update,
                (handle_node_press, handle_nav_press, handle_view_toggle).in_set(UiSet::Input),
            )
            .add_systems(
                Update,
                (
                    reset_on_reload,
                    rebuild_graph,
                    rebuild_view_switch,
                    sync_view_mode,
                    center_canvas,
                )
                    .in_set(UiSet::Rebuild),
            );
    }
}
