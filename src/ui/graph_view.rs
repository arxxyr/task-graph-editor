//! 流程图视图：把任务图的 nodes/edges 画成可下钻的分层流程图
//!
//! 位置由 [`super::graph_layout`] 算好，这里只负责画：
//! - 节点是绝对定位的卡片，左侧色条按类别着色
//! - 边是几段绝对定位的细矩形拼出的正交折线（Bevy UI 没有画线的原语，
//!   而正交折线本来就比曲线更适合流程图），终点补一个箭头字符
//! - 回边（`loop` 的循环体）走右侧通道，用强调色标出
//!
//! 编辑模式复用模型命令；手工布局与缩放只属于本地视图状态。

use bevy::feathers::controls::ButtonVariant;
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemeToken};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::Interaction;
use bevy::ui_widgets::{Activate, ScrollArea};

use crate::model::{SubGraph, TaskNode};

use super::document_guard::DocumentGuard;
use super::graph_edit::{GraphEditPanelSlot, GraphEditToolbarSlot, GraphEditing};
use super::graph_layout::{self, GraphLayout, NODE_H, NODE_W, NodeSlot};
use super::shell::{GraphPane, GraphSlot, ParamsPane, ViewSwitchSlot};
use super::theme;
use super::widgets::{self, BoxedScene, boxed};
use super::{Editor, UiSet};

mod canvas_edit;
mod details;

#[cfg(test)]
mod regression_tests;

/// 折线粗细
const EDGE_W: f32 = 1.5;

/// 连线使用独立的透明命中区域，便于选中细线。
const EDGE_HIT_W: f32 = 10.0;

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
    /// 当前选中的边在本层边数组中的索引，同端点的边仍能分别定位。
    pub selected_edge: Option<usize>,
    /// 版本号：路径或数据变化时递增，驱动重建
    pub version: u64,
}

impl GraphNav {
    /// 进入某个复合节点
    fn enter(&mut self, id: &str) {
        self.path.push(id.to_string());
        self.selected = None;
        self.selected_edge = None;
        self.version += 1;
    }

    /// 回到第 depth 层（0 为根）
    fn go_to(&mut self, depth: usize) {
        self.path.truncate(depth);
        self.selected = None;
        self.selected_edge = None;
        self.version += 1;
    }

    /// 换文件或重新加载后回到根
    fn reset(&mut self) {
        self.path.clear();
        self.selected = None;
        self.selected_edge = None;
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

/// 每批图场景及其详情保留自己的导航版本，旧实体事件不能被新路径重新解释。
#[derive(Component, Clone, Copy, Default)]
pub(super) struct GraphSceneVersion(u64);

/// 图卡文字记录未选中时的角色，选中与取消时只切换 token。
#[derive(Component, Default, Clone)]
struct GraphNodeLabel(ThemeToken);

#[derive(Component, Clone, Default)]
struct GraphNodeText {
    id: String,
    kind: bool,
}
#[derive(Component, Clone, Default)]
struct GraphNodeCategory(String);
#[derive(Component, Clone, Default)]
struct GraphCheckpoint(String);

/// 线段命中目标记录画布版本，跨帧旧画布的输入不能转移到新子图。
#[derive(Component, Clone, Default)]
struct GraphEdgeMarker {
    index: usize,
    version: u64,
}

/// 可见线段和箭头共用源边身份，以便整条路径同时高亮。
#[derive(Component, Clone, Default)]
struct GraphEdgeVisual {
    index: usize,
    back: bool,
}

#[derive(Resource, Default)]
struct HoveredEdge(Option<usize>, u64);

/// 详情栏插槽只更新选中节点，画布及其滚动位置保持不变。
#[derive(Component, Clone, Default)]
struct GraphDetailSlot {
    rendered: Option<(u64, Option<String>, Option<usize>, u64)>,
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
    /// 已渲染时对应的文档加载代次，context 创建位姿不影响流程导航。
    document: Option<u64>,
}

// ============================================================
// 场景构建
// ============================================================

/// 整个流程图面板
fn graph_pane(
    graph: &SubGraph,
    nav: &GraphNav,
    state: Option<&canvas_edit::CanvasState>,
) -> Vec<BoxedScene> {
    let Some(current) = graph.subgraph_at(&nav.path) else {
        return vec![boxed(widgets::hint("这一层已不存在，请返回上一层"))];
    };
    let placed = graph_layout::layout(current);
    let mut content = vec![boxed(toolbar(graph, nav, current, placed.as_ref().ok()))];
    content.push(boxed(bsn! {
        Node { width: percent(100), flex_shrink: 0.0 }
        GraphEditToolbarSlot
    }));
    content.push(boxed(canvas_edit::controls()));
    if !current.diagnostics.is_empty() {
        let messages = current
            .diagnostics
            .iter()
            .map(|issue| format!("{}：{}", issue.path, issue.message))
            .collect::<Vec<_>>()
            .join("\n");
        content.push(boxed(bsn! {
            Node {
                max_height: px(160),
                flex_shrink: 0.0,
                padding: {UiRect::axes(px(theme::PAD), px(6))},
                overflow: {Overflow::scroll_y()},
            }
            ScrollArea
            Children [(details::text_section(
                format!("结构诊断 · {} 项（原始文件内容保留）", current.diagnostics.len()),
                messages,
            ))]
        }));
    }
    let placed = match placed {
        Ok(placed) => placed,
        Err(error) => {
            content.push(boxed(widgets::hint(format!(
                "本层无法绘制：{error}。请修正源文件后重新加载；其他 context 参数仍可查看和保存。"
            ))));
            return content;
        }
    };
    content.push(boxed(bsn! {
        Node {
            width: percent(100),
            flex_grow: 1.0,
            flex_direction: FlexDirection::Row,
            min_height: px(0),
        }
        Children [
            (canvas(current, state.filter(|state| state.layout_error.is_none()).map_or(&placed, |state| &state.layout), nav)),
            (
                Node {
                    display: Display::None,
                    width: {px(DETAIL_W)},
                    flex_shrink: 0.0,
                    height: percent(100),
                }
                GraphDetailSlot
            ),
            (
                Node {
                    display: Display::None,
                    width: {px(DETAIL_W)}, min_width: {px(DETAIL_W)},
                    flex_shrink: 0.0, height: percent(100), min_height: px(0),
                }
                GraphEditPanelSlot
            )
        ]
    }));
    content
}

/// 顶部：面包屑 + 本层统计 + 图例
fn toolbar(
    graph: &SubGraph,
    nav: &GraphNav,
    current: &SubGraph,
    placed: Option<&GraphLayout>,
) -> impl Scene {
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
    let (source_nodes, source_edges) = current
        .source_counts
        .unwrap_or((current.nodes.len(), current.edges.len()));
    let (drawn_nodes, drawn_edges) = placed.map_or((0, 0), |layout| {
        (
            layout
                .nodes
                .iter()
                .filter(|node| node.slot == NodeSlot::Real)
                .count(),
            layout.edges.len(),
        )
    });
    let summary = format!(
        "本层原始 {source_nodes} 节点 / {source_edges} 边 · 可绘制 {drawn_nodes} 节点 / {drawn_edges} 边 · 可下钻 {composite}　全图已识别 {} 节点 / {} 层 · {} 项诊断",
        graph.total_nodes(),
        graph.depth(),
        graph.diagnostic_count()
    );
    let parallel_note: Vec<BoxedScene> = nav
        .path
        .split_last()
        .and_then(|(id, parent)| graph.subgraph_at(parent)?.node(id))
        .filter(|node| node.node_type == "parallel")
        .map(|_| {
            boxed(widgets::hint(
                "parallel 的直接子节点并行执行；本层连线不控制执行顺序。",
            ))
        })
        .into_iter()
        .collect();

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
                    (widgets::hint("点击节点查看参数，详情按钮进入子图；悬停连线追踪路径，点击查看起终点"))
                ]
            ),
            {parallel_note}
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
    let mut items: Vec<BoxedScene> = vec![boxed(bsn! {
        Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100) }
        canvas_edit::CanvasEdges
    })];
    for p in &placed.nodes {
        let node = current.node(&p.id);
        items.push(boxed(node_scene(
            p,
            node,
            nav.selected.as_deref() == Some(p.id.as_str()),
        )));
    }
    for node in &placed.nodes {
        if node.slot != NodeSlot::Entry {
            items.push(canvas_edit::port_scene(
                &node.id,
                true,
                node.pos + Vec2::new(NODE_W / 2.0, 0.0),
            ));
        }
        if node.slot != NodeSlot::Exit {
            items.push(canvas_edit::port_scene(
                &node.id,
                false,
                node.pos + Vec2::new(NODE_W / 2.0, NODE_H),
            ));
        }
    }

    let (w, h) = (placed.size.x, placed.size.y);
    bsn! {
        Node {
            flex_grow: 1.0,
            height: percent(100),
            min_width: px(0),
            overflow: {Overflow::scroll()},
        }
        ScrollPosition::default()
        canvas_edit::CanvasViewport({nav.version})
        CanvasNeedsCenter
        Children [(
            Node {
                width: {px(w)},
                height: {px(h)},
                flex_shrink: 0.0,
            }
            canvas_edit::CanvasExtent
            Children [(
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0), top: px(0), width: {px(w)}, height: {px(h)},
                }
                UiTransform::default()
                canvas_edit::CanvasContent
                Children [{items}]
            )]
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
            template_value(canvas_edit::CanvasItem(placed.id.clone()))
            Children [(widgets::readonly_value(label))]
        });
    };

    let badge: Vec<BoxedScene> = node
        .children
        .as_ref()
        .map(|children| {
            let issues = children.diagnostic_count();
            let label = match issues {
                0 => format!("{} ▸", children.nodes.len()),
                _ => format!("问题 {issues} ▸"),
            };
            boxed(widgets::badge(label))
        })
        .into_iter()
        .collect();
    let ckpt = boxed(bsn! {
        Node {
            display: {if node.checkpoint { Display::Flex } else { Display::None }},
            width: px(7),
            height: px(7),
            border_radius: {BorderRadius::all(px(4.0))},
            flex_shrink: 0.0,
        }
        ThemeBackgroundColor({theme::CHECKPOINT_DOT})
        template_value(GraphCheckpoint(node.id.clone()))
    });

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
        template_value(canvas_edit::CanvasItem(node.id.clone()))
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
                template_value(GraphNodeCategory(node.id.clone()))
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
                        template_value(GraphNodeText { id: node.id.clone(), kind: false })
                        ThemeTextColor({theme::SECTION_TEXT})
                        TextFont { font_size: px(11.5) }
                        TextLayout { linebreak: {LineBreak::AnyCharacter} }
                    ),
                    (
                        Text({node.node_type.clone()})
                        GraphNodeLabel({theme::READONLY_TEXT})
                        template_value(GraphNodeText { id: node.id.clone(), kind: true })
                        ThemeTextColor({theme::READONLY_TEXT})
                        TextFont { font_size: px(10.0) }
                    )
                ]
            ),
            (ckpt),
            {badge}
        ]
    })
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

/// 文档加载才重置导航；context 的结构和数值变化不影响流程视口。
fn reset_on_reload(
    editor: Res<Editor>,
    mut nav: ResMut<GraphNav>,
    mut rendered: ResMut<RenderedGraph>,
) {
    if rendered.document == Some(editor.document_version) {
        // 防御性修复失效路径，保留最近仍存在的祖先；普通参数输入不推进导航版本。
        if editor.is_changed()
            && let Some(data) = &editor.data
        {
            let mut depth = nav.path.len();
            while data.graph.subgraph_at(&nav.path[..depth]).is_none() && depth > 0 {
                depth -= 1;
            }
            if depth != nav.path.len() {
                nav.go_to(depth);
            }
            if let Some(graph) = data.graph.subgraph_at(&nav.path) {
                if nav
                    .selected
                    .as_deref()
                    .is_some_and(|id| graph.node(id).is_none())
                {
                    nav.selected = None;
                }
                if nav
                    .selected_edge
                    .is_some_and(|index| index >= graph.edges.len())
                {
                    nav.selected_edge = None;
                }
            }
        }
        return;
    }
    rendered.document = Some(editor.document_version);
    nav.reset();
}

/// 按浏览状态重建流程图；后代插槽的异步场景落地前必须保留父树。
#[allow(clippy::too_many_arguments)]
fn rebuild_graph(
    editor: Res<Editor>,
    canvas: Option<Res<canvas_edit::CanvasState>>,
    nav: Res<GraphNav>,
    mut rendered: ResMut<RenderedGraph>,
    slots: Query<Entity, With<GraphSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    children: Query<&Children>,
    proxy: Option<Res<bevy::winit::EventLoopProxyWrapper>>,
    mut commands: Commands,
) {
    let Ok(slot) = slots.single() else {
        return;
    };
    if rendered.nav == Some(nav.version) {
        return;
    }
    if pending.contains(slot)
        || children
            .iter_descendants(slot)
            .any(|child| pending.contains(child))
    {
        // BSN 队列中的 ChildOf 仍指向旧插槽。先让子场景完成，下一帧再整体销毁，避免孤儿 UI。
        // SlotPending 在本阶段之后才摘除，主动唤醒下一帧，不能等 reactive 的五秒空闲周期。
        if let Some(proxy) = &proxy {
            let _ = proxy.send_event(bevy::winit::WinitUserEvent::WakeUp);
        }
        return;
    }
    rendered.nav = Some(nav.version);

    let content = match &editor.data {
        Some(data) => {
            let children = graph_pane(&data.graph, &nav, canvas.as_deref());
            vec![boxed(bsn! {
                Node {
                    width: percent(100),
                    height: percent(100),
                    min_height: px(0),
                    flex_direction: FlexDirection::Column,
                }
                GraphSceneVersion({nav.version})
                Children [{children}]
            })]
        }
        None => vec![boxed(widgets::hint("请先从左侧选择一个文件"))],
    };
    widgets::replace_slot_children(&mut commands, slot, content);
}

/// 选中变化只改颜色，保留画布和输入状态；跨帧新增的图卡与文字也同步当前选择。
fn sync_node_selection(
    nav: Res<GraphNav>,
    editor: Option<Res<Editor>>,
    editing: Option<Res<GraphEditing>>,
    nodes: Query<(Entity, Ref<GraphNodeMarker>)>,
    labels: Query<(Entity, Ref<GraphNodeLabel>)>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let selected = |id: &str| {
        let in_selection = editor
            .as_ref()
            .and_then(|editor| editor.data.as_ref())
            .is_some_and(|data| {
                data.graph_edit
                    .graph_key_at(&nav.path)
                    .and_then(|graph| data.graph_edit.node_key(graph, id))
                    .is_some_and(|key| {
                        editing.as_ref().is_some_and(|editing| {
                            editing.enabled && editing.selected_nodes.contains(&key)
                        })
                    })
            });
        in_selection || nav.selected.as_deref() == Some(id)
    };
    for (entity, marker) in &nodes {
        if !nav.is_changed()
            && !editing.as_ref().is_some_and(|editing| editing.is_changed())
            && !marker.is_added()
        {
            continue;
        }
        let (background, border) = match selected(&marker.0) {
            true => (theme::GRAPH_SELECTED_BG, theme::GRAPH_SELECTED_BORDER),
            false => (theme::CARD_BG, theme::GRAPH_BORDER),
        };
        commands
            .entity(entity)
            .insert((ThemeBackgroundColor(background), ThemeBorderColor(border)));
    }
    for (entity, label) in &labels {
        if !nav.is_changed()
            && !editing.as_ref().is_some_and(|editing| editing.is_changed())
            && !label.is_added()
        {
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
        let color = match selected(&marker.0) {
            true => theme::GRAPH_SELECTED_TEXT,
            false => label.0.clone(),
        };
        commands.entity(entity).insert(ThemeTextColor(color));
    }
}

/// 属性编辑只刷新已有标签、类别色条和 checkpoint，不重建卡片或画布。
fn sync_node_text(
    editor: Res<Editor>,
    nav: Res<GraphNav>,
    mut labels: Query<(&GraphNodeText, &mut Text)>,
    categories: Query<(Entity, Ref<GraphNodeCategory>)>,
    mut checkpoints: Query<(&GraphCheckpoint, &mut Node)>,
    mut commands: Commands,
) {
    let Some(graph) = editor
        .data
        .as_ref()
        .and_then(|data| data.graph.subgraph_at(&nav.path))
    else {
        return;
    };
    for (label, mut text) in &mut labels {
        if let Some(node) = graph.node(&label.id) {
            let value = if label.kind {
                &node.node_type
            } else {
                &node.id
            };
            if &text.0 != value {
                text.0.clone_from(value);
            }
        }
    }
    for (entity, category) in &categories {
        if (editor.is_changed() || nav.is_changed() || category.is_added())
            && let Some(node) = graph.node(&category.0)
        {
            commands
                .entity(entity)
                .insert(ThemeBackgroundColor(category_token(&node.node_type)));
        }
    }
    for (marker, mut panel) in &mut checkpoints {
        if let Some(node) = graph.node(&marker.0) {
            let display = if node.checkpoint {
                Display::Flex
            } else {
                Display::None
            };
            if panel.display != display {
                panel.display = display;
            }
        }
    }
}

/// 独立替换详情内容，不重建拥有 ScrollPosition 的画布。
#[allow(clippy::too_many_arguments)]
fn rebuild_detail(
    editor: Res<Editor>,
    nav: Res<GraphNav>,
    editing: Res<GraphEditing>,
    mut slots: Query<(Entity, &mut GraphDetailSlot, &mut Node)>,
    pending: Query<(), With<widgets::SlotPending>>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    mut commands: Commands,
) {
    let revision = editor
        .data
        .as_ref()
        .map_or(0, |data| data.graph_edit.revision());
    let state = (
        nav.version,
        nav.selected.clone(),
        nav.selected_edge,
        revision,
    );
    for (entity, mut slot, mut panel) in &mut slots {
        if stale_scene(entity, nav.version, &parents, &versions) {
            panel.display = Display::None;
            continue;
        }
        if editing.enabled {
            panel.display = Display::None;
            continue;
        }
        panel.display = match nav.selected.is_some() || nav.selected_edge.is_some() {
            true => Display::Flex,
            false => Display::None,
        };
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
                let raw = editor.data.as_ref().and_then(|data| {
                    let graph = data.graph_edit.graph_key_at(&nav.path)?;
                    let key = data.graph_edit.node_key(graph, &node.id)?;
                    data.graph_edit.node_json(key)
                });
                vec![boxed(details::detail_panel(node, raw))]
            }
            None => {
                let edge = editor.data.as_ref().and_then(|data| {
                    let graph = data.graph.subgraph_at(&nav.path)?;
                    let index = nav.selected_edge?;
                    Some((index, graph.edges.get(index)?))
                });
                match edge {
                    Some((index, edge)) => {
                        panel.display = Display::Flex;
                        let raw = editor
                            .data
                            .as_ref()
                            .and_then(|data| raw_edge_at(data, &nav.path, index));
                        vec![boxed(edge_detail_panel(index, edge, raw))]
                    }
                    None => {
                        panel.display = Display::None;
                        Vec::new()
                    }
                }
            }
        };
        widgets::replace_slot_children(&mut commands, entity, content);
        slot.rendered = Some(state.clone());
    }
}

/// 根据已验证的同层 ID 查找原始容器，未知节点字段不需要经过展示模型重建。
fn raw_subgraph_at<'a>(
    data: &'a crate::model::TaskGraphData,
    path: &[String],
) -> Option<&'a serde_json::Value> {
    data.graph_edit
        .graph_key_at(path)
        .and_then(|key| data.graph_edit.graph_json(key))
}

/// 展示索引对应解析后边数组；缺失字符串端点的原始条目不进入该数组。
fn raw_edge_at<'a>(
    data: &'a crate::model::TaskGraphData,
    path: &[String],
    index: usize,
) -> Option<(usize, &'a serde_json::Value)> {
    raw_subgraph_at(data, path)?
        .get("edges")?
        .as_array()?
        .iter()
        .enumerate()
        .filter(|(_, value)| {
            value.get("from").and_then(|v| v.as_str()).is_some()
                && value.get("to").and_then(|v| v.as_str()).is_some()
        })
        .nth(index)
}

fn edge_detail_panel(
    index: usize,
    edge: &crate::model::GraphEdge,
    raw: Option<(usize, &serde_json::Value)>,
) -> impl Scene {
    let source_index = raw.map_or(index, |(index, _)| index);
    let raw_section: Vec<BoxedScene> = raw
        .map(|(_, value)| {
            let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
            boxed(details::text_section("原始连接 JSON", text))
        })
        .into_iter()
        .collect();
    bsn! {
        Node {
            width: {px(DETAIL_W)},
            height: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(6),
            padding: {UiRect::all(px(theme::PAD))},
            overflow: {Overflow::scroll_y()},
        }
        ScrollArea
        ThemeBackgroundColor({theme::SIDEBAR_BG})
        Children [
            (widgets::subheading(format!("连接 · 第 {} 条", source_index + 1))),
            (details::text_section("起点", edge.from.clone())),
            (details::text_section("终点", edge.to.clone())),
            (widgets::hint("高亮路径连接以上两个端点；同端点的多条连接也可分别选择。")),
            {raw_section}
        ]
    }
}

/// 悬停时追踪整条路径，点击后保留选择；选边不重建画布。
fn handle_edge_interaction(
    edges: Query<(&Interaction, &GraphEdgeMarker)>,
    mut nav: ResMut<GraphNav>,
    mut hovered: ResMut<HoveredEdge>,
    editing: Option<Res<GraphEditing>>,
    guard: Option<Res<DocumentGuard>>,
) {
    if guard.as_ref().is_some_and(|guard| guard.active()) {
        hovered.0 = None;
        return;
    }
    let mut active = None;
    let mut pressed = None;
    for (interaction, edge) in &edges {
        if edge.version != nav.version {
            continue;
        }
        match interaction {
            Interaction::Pressed => pressed = Some(edge.index),
            Interaction::Hovered => active = Some(edge.index),
            Interaction::None => {}
        }
    }
    if let Some(index) =
        pressed.filter(|_| !editing.as_ref().is_some_and(|editing| editing.enabled))
        && (nav.selected.is_some() || nav.selected_edge != Some(index))
    {
        nav.selected = None;
        nav.selected_edge = Some(index);
    }
    let active = pressed.or(active);
    if hovered.0 != active {
        hovered.0 = active;
    }
    if hovered.1 != nav.version {
        hovered.1 = nav.version;
    }
}

fn sync_edge_selection(
    nav: Res<GraphNav>,
    hovered: Res<HoveredEdge>,
    visuals: Query<(Entity, Ref<GraphEdgeVisual>, Has<Text>)>,
    mut commands: Commands,
) {
    let active = match hovered.1 == nav.version {
        true => hovered.0.or(nav.selected_edge),
        false => nav.selected_edge,
    };
    for (entity, edge, text) in &visuals {
        if !nav.is_changed() && !hovered.is_changed() && !edge.is_added() {
            continue;
        }
        let token = match (active == Some(edge.index), edge.back) {
            (true, _) => theme::GRAPH_SELECTED_BORDER,
            (false, true) => theme::GRAPH_BACK,
            (false, false) => theme::GRAPH_BORDER,
        };
        match text {
            true => {
                commands.entity(entity).insert(ThemeTextColor(token));
            }
            false => {
                commands.entity(entity).insert(ThemeBackgroundColor(token));
            }
        }
    }
}

/// 浏览按下仅选中；下钻由完成的双击或详情按钮触发，避免拖动前误进入子图。
fn handle_node_press(
    nodes: Query<(Entity, &Interaction, &GraphNodeMarker), Changed<Interaction>>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    editor: Res<Editor>,
    editing: Option<Res<GraphEditing>>,
    guard: Option<Res<DocumentGuard>>,
    mut nav: ResMut<GraphNav>,
) {
    if editing.as_ref().is_some_and(|editing| editing.enabled)
        || guard.as_ref().is_some_and(|guard| guard.active())
    {
        return;
    }
    for (entity, state, marker) in &nodes {
        if *state != Interaction::Pressed
            || !current_scene(entity, nav.version, &parents, &versions)
        {
            continue;
        }
        let node = editor.data.as_ref().and_then(|d| {
            d.graph
                .subgraph_at(&nav.path)
                .and_then(|g| g.node(&marker.0))
        });
        let Some(_) = node else {
            continue;
        };
        nav.selected = Some(marker.0.clone());
        nav.selected_edge = None;
    }
}

/// 面包屑使用原生 UI Button，继续由 Interaction 驱动。
fn handle_nav_press(
    crumbs: Query<(Entity, &Interaction, &CrumbMarker), Changed<Interaction>>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    mut nav: ResMut<GraphNav>,
    guard: Option<Res<DocumentGuard>>,
) {
    if guard.as_ref().is_some_and(|guard| guard.active()) {
        return;
    }
    for (entity, state, crumb) in &crumbs {
        if *state == Interaction::Pressed && current_scene(entity, nav.version, &parents, &versions)
        {
            nav.go_to(crumb.0);
        }
    }
}

/// 详情中的下钻入口是 FeathersButton，鼠标和键盘统一通过 Activate 触发。
fn handle_drill_activate(
    event: On<Activate>,
    drills: Query<&DrillButton>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    editor: Res<Editor>,
    mut nav: ResMut<GraphNav>,
    guard: Option<Res<DocumentGuard>>,
) {
    if guard.as_ref().is_some_and(|guard| guard.active()) {
        return;
    }
    let Ok(drill) = drills.get(event.entity) else {
        return;
    };
    if !current_scene(event.entity, nav.version, &parents, &versions) {
        return;
    }
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

fn current_scene(
    entity: Entity,
    version: u64,
    parents: &Query<&ChildOf>,
    versions: &Query<&GraphSceneVersion>,
) -> bool {
    widgets::self_or_ancestor(entity, parents, |entity| versions.contains(entity))
        .and_then(|entity| versions.get(entity).ok())
        .is_some_and(|scene| scene.0 == version)
}

/// 独立插槽没有场景代次；若属于画布，则禁止已切走的旧层继续排队生成子场景。
pub(super) fn stale_scene(
    entity: Entity,
    version: u64,
    parents: &Query<&ChildOf>,
    versions: &Query<&GraphSceneVersion>,
) -> bool {
    widgets::self_or_ancestor(entity, parents, |entity| versions.contains(entity))
        .and_then(|entity| versions.get(entity).ok())
        .is_some_and(|scene| scene.0 != version)
}

fn graph_is_rendered(nav: Res<GraphNav>, rendered: Res<RenderedGraph>) -> bool {
    rendered.nav == Some(nav.version)
}

/// 把画布的水平滚动挪到内容中线
///
/// 分层布局把每一层都对齐到内容中线，而滚动位置默认停在最左：
/// 内容比视口宽时主干会被推到右边缘，右侧的回边通道整条看不见。
/// 布局尺寸要等 `ui_layout_system` 跑完才有，所以靠标记组件轮询，量到了就居中并摘掉标记。
#[allow(clippy::type_complexity)]
fn center_canvas(
    mut canvas: Query<
        (Entity, &ComputedNode, &mut ScrollPosition),
        (
            With<CanvasNeedsCenter>,
            Without<canvas_edit::CanvasViewport>,
        ),
    >,
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

fn sync_edit_panel_visibility(
    editing: Res<GraphEditing>,
    mut panels: Query<&mut Node, With<GraphEditPanelSlot>>,
) {
    let display = if editing.enabled {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in &mut panels {
        if node.display != display {
            node.display = display;
        }
    }
}

/// FeathersButton 没有旧 Interaction 组件，实际点击及键盘触发均发出 Activate。
fn handle_view_toggle(
    event: On<Activate>,
    toggles: Query<&ViewToggle>,
    mut variants: Query<(&ViewToggle, &mut ButtonVariant)>,
    mut mode: ResMut<ViewMode>,
    guard: Option<Res<DocumentGuard>>,
) {
    if guard.as_ref().is_some_and(|guard| guard.active()) {
        return;
    }
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

/// 属性面板必须在稳定身份、导航与选择修复完成之后读取当前图层。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GraphViewPrepared;

/// 子属性面板必须等待父画布替换及其延迟命令完成，再查询仍然存活的插槽。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GraphViewRebuilt;

impl Plugin for GraphViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewMode>()
            .init_resource::<GraphNav>()
            .init_resource::<RenderedGraph>()
            .init_resource::<HoveredEdge>()
            .add_observer(handle_view_toggle)
            .add_observer(handle_drill_activate)
            .add_systems(
                Update,
                (
                    handle_edge_interaction,
                    handle_node_press,
                    handle_nav_press,
                    canvas_edit::handle_input,
                )
                    .chain()
                    .in_set(UiSet::Input),
            )
            .add_systems(
                Update,
                (
                    reset_on_reload,
                    canvas_edit::prepare.in_set(GraphViewPrepared),
                    rebuild_graph.in_set(GraphViewRebuilt),
                    canvas_edit::sync_geometry.run_if(graph_is_rendered),
                    canvas_edit::redraw_edges,
                    canvas_edit::sync_controls,
                    sync_node_selection.run_if(graph_is_rendered),
                    sync_node_text.run_if(graph_is_rendered),
                    sync_edge_selection.run_if(graph_is_rendered),
                    rebuild_detail,
                    build_view_switch,
                    sync_view_switch,
                    sync_view_mode,
                    sync_edit_panel_visibility,
                    center_canvas,
                )
                    .chain()
                    .in_set(UiSet::Rebuild),
            );
        details::register(app);
        canvas_edit::register(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn test_editor() -> Editor {
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
                GraphSceneVersion(0),
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
    fn 再次点击复合节点不会在拖动前误下钻() {
        let mut app = App::new();
        app.insert_resource(test_editor())
            .init_resource::<GraphNav>()
            .add_systems(Update, handle_node_press);
        let card = app
            .world_mut()
            .spawn((
                GraphNodeMarker("step".into()),
                GraphSceneVersion(0),
                Interaction::Pressed,
            ))
            .id();
        app.update();
        assert_eq!(app.world().resource::<GraphNav>().version, 0);
        *app.world_mut().get_mut::<Interaction>(card).unwrap() = Interaction::None;
        app.update();
        *app.world_mut().get_mut::<Interaction>(card).unwrap() = Interaction::Pressed;
        app.update();
        let nav = app.world().resource::<GraphNav>();
        assert!(nav.path.is_empty());
        assert_eq!(nav.selected.as_deref(), Some("step"));
        assert_eq!(nav.version, 0);
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
    pub(super) fn graph_app(editor: Editor) -> App {
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

    pub(super) fn settle_scenes(app: &mut App) {
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
        app.world_mut().spawn((GraphSlot, Node::default()));
        settle_scenes(&mut app);
        app.world_mut().resource_mut::<GraphNav>().selected = Some("step".into());
        settle_scenes(&mut app);
        let button = app
            .world_mut()
            .query_filtered::<Entity, With<DrillButton>>()
            .single(app.world())
            .unwrap();
        assert!(app.world().get::<Interaction>(button).is_none());
        click_caption(&mut app, button);
        settle_scenes(&mut app);
        assert_eq!(app.world().resource::<GraphNav>().path, ["step"]);
        let root_crumb = app
            .world_mut()
            .query::<(Entity, &CrumbMarker)>()
            .iter(app.world())
            .find(|(_, crumb)| crumb.0 == 0)
            .unwrap()
            .0;
        *app.world_mut().get_mut::<Interaction>(root_crumb).unwrap() = Interaction::Pressed;
        app.update();
        assert!(app.world().resource::<GraphNav>().path.is_empty());
    }
}
