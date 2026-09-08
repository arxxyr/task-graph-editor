//! 画布视口与手势。坐标、选区、手工布局留在本机内存，流程命令统一经过输入刷新队列。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::ui::Val2;

use super::*;
use crate::model::graph_edit::{EdgeKey, GraphCommand, GraphKey, NodeKey};
use crate::ui::graph_edit::{
    GraphControl, GraphEditing, GraphIntent, GraphOperation, submit_command,
};
use crate::ui::worker_bridge::AppAction;

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 3.0;
const DRAG_THRESHOLD: f32 = 4.0;
const PORT_RADIUS: f32 = 11.0;

#[derive(Clone, Copy, PartialEq, Eq)]
struct ClickTarget {
    node: NodeKey,
    document: u64,
    scene: u64,
}

struct CardPress {
    target: ClickTarget,
    position: Vec2,
    dragged: bool,
}

#[derive(Resource, Default)]
struct NodeClicks {
    press: Option<CardPress>,
    last: Option<(ClickTarget, std::time::Instant)>,
}

/// 物理屏幕坐标先除目标 DPI 和全局 UI 缩放，再叠加滚动，最后除画布缩放。
#[derive(Clone, Copy, Debug)]
struct Coordinates {
    origin_physical: Vec2,
    inverse_scale: f32,
    scroll: Vec2,
    zoom: f32,
}

impl Coordinates {
    fn logical(self, physical: Vec2) -> Vec2 {
        (physical - self.origin_physical) * self.inverse_scale
    }

    fn graph(self, physical: Vec2) -> Vec2 {
        (self.logical(physical) + self.scroll) / self.zoom
    }

    #[cfg(test)]
    fn physical(self, graph: Vec2) -> Vec2 {
        (graph * self.zoom - self.scroll) / self.inverse_scale + self.origin_physical
    }
}

#[derive(Clone)]
struct LayerView {
    zoom: f32,
    scroll: Vec2,
    positions: BTreeMap<NodeKey, Vec2>,
    initialized: bool,
}

impl Default for LayerView {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            scroll: Vec2::ZERO,
            positions: BTreeMap::new(),
            initialized: false,
        }
    }
}

#[derive(Clone, Default)]
enum Gesture {
    #[default]
    Idle,
    Move {
        start: Vec2,
        original: BTreeMap<NodeKey, Vec2>,
        preview: BTreeMap<NodeKey, Vec2>,
        moved: bool,
    },
    Box {
        start: Vec2,
        end: Vec2,
        original: BTreeSet<NodeKey>,
        additive: bool,
    },
    Pan {
        start: Vec2,
        scroll: Vec2,
        button: MouseButton,
    },
    Connect {
        id: String,
        incoming: bool,
        cursor: Vec2,
    },
    Reconnect {
        edge: EdgeKey,
        from: String,
        to: String,
        incoming: bool,
        cursor: Vec2,
    },
}

#[derive(Resource, Default)]
pub(super) struct CanvasState {
    document: Option<u64>,
    topology: Option<u64>,
    pub(super) active: Option<GraphKey>,
    views: HashMap<GraphKey, LayerView>,
    pub(super) layout: GraphLayout,
    pub(super) layout_error: Option<String>,
    pub(super) geometry_revision: u64,
    gesture: Gesture,
    selected_node: Option<NodeKey>,
    selected_edge: Option<EdgeKey>,
    remembered_graphs: Vec<GraphKey>,
    fit_requested: bool,
    /// UI 自动居中/缩放等设置写回前，不能用旧实体的滚动覆盖新视口。
    apply_view: bool,
    computed_geometry: Option<(GraphKey, u64)>,
    viewport_size: Vec2,
}

impl CanvasState {
    fn view(&self) -> Option<&LayerView> {
        self.views.get(&self.active?)
    }
    fn view_mut(&mut self) -> Option<&mut LayerView> {
        self.views.get_mut(&self.active?)
    }

    pub(super) fn zoom(&self) -> f32 {
        self.view().map_or(1.0, |view| view.zoom)
    }

    fn cancel(&mut self) {
        if !matches!(self.gesture, Gesture::Idle) {
            if let Gesture::Pan { scroll, .. } = self.gesture {
                if let Some(view) = self.view_mut() {
                    view.scroll = scroll;
                }
                self.apply_view = true;
            }
            self.gesture = Gesture::Idle;
            self.geometry_revision += 1;
        }
    }
}

/// 框选集合与详情目标使用同一份选择；保留仍在集合中的焦点，否则选取一个有效节点。
fn sync_selection_focus(editor: &Editor, nav: &mut GraphNav, editing: &GraphEditing) {
    let Some(data) = &editor.data else {
        return;
    };
    let Some(graph) = data.graph_edit.graph_key_at(&nav.path) else {
        return;
    };
    let focused = nav
        .selected
        .as_deref()
        .and_then(|id| data.graph_edit.node_key(graph, id));
    if !focused.is_some_and(|key| editing.selected_nodes.contains(&key)) {
        nav.selected = editing.selected_nodes.iter().rev().find_map(|&key| {
            let id = data.graph_edit.node_id(key)?;
            (data.graph_edit.node_key(graph, id) == Some(key)).then(|| id.to_owned())
        });
    }
    nav.selected_edge = None;
}

fn cancel_gesture(
    state: &mut CanvasState,
    editor: &Editor,
    nav: &mut GraphNav,
    editing: &mut GraphEditing,
) {
    if let Gesture::Box { original, .. } = &state.gesture {
        editing.selected_nodes.clone_from(original);
        sync_selection_focus(editor, nav, editing);
    }
    state.cancel();
}

#[derive(Component, Clone, Copy, Default)]
pub(super) struct CanvasViewport(pub u64);
#[derive(Component, Clone, Copy, Default)]
pub(super) struct CanvasExtent;
#[derive(Component, Clone, Copy, Default)]
pub(super) struct CanvasContent;
#[derive(Component, Clone, Default)]
pub(super) struct CanvasItem(pub String);
#[derive(Component, Clone, Default)]
pub(super) struct CanvasPort {
    id: String,
    incoming: bool,
}
#[derive(Component, Clone, Copy, Default)]
pub(super) struct CanvasEdges {
    rendered: Option<(u64, u64)>,
}
#[derive(Component, Clone, Copy, Default)]
struct CanvasOverlay;
#[derive(Component, Clone, Copy, Default)]
pub(super) struct CanvasZoomLabel;
#[derive(Component, Clone, Copy, Default)]
pub(super) struct CanvasLayoutWarning;

#[derive(Component, Clone, Copy, Default)]
pub(super) enum CanvasAction {
    #[default]
    ZoomIn,
    ZoomOut,
    Actual,
    Fit,
    Auto,
    ReconnectFrom,
    ReconnectTo,
}

fn canvas_control(label: &'static str, action: CanvasAction) -> impl Scene {
    widgets::button_gated(
        label,
        ButtonVariant::Normal,
        widgets::ButtonGate::Managed,
        action,
    )
}

pub(super) fn controls() -> impl Scene {
    bsn! {
        Node {
            width: percent(100), flex_shrink: 0.0,
            flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap,
            align_items: AlignItems::Center, column_gap: px(5), row_gap: px(4),
            padding: {UiRect::axes(px(theme::PAD), px(5))},
        }
        Children [
            (canvas_control("−", CanvasAction::ZoomOut)),
            (Text("100%") CanvasZoomLabel TextFont { font_size: px(12) } ThemeTextColor({theme::FIELD_LABEL})),
            (canvas_control("＋", CanvasAction::ZoomIn)),
            (canvas_control("100%", CanvasAction::Actual)),
            (canvas_control("适配画布", CanvasAction::Fit)),
            (canvas_control("自动布局", CanvasAction::Auto)),
            (canvas_control("重连起点", CanvasAction::ReconnectFrom)),
            (canvas_control("重连终点", CanvasAction::ReconnectTo)),
            (widgets::hint("双击复合节点进入子图 · Ctrl/⌘＋滚轮缩放 · 中键/空格平移 · 编辑时框选/拖动 · Esc 取消")),
            (Text("") CanvasLayoutWarning TextFont { font_size: px(12) } ThemeTextColor({theme::FIELD_LABEL}))
        ]
    }
}

pub(super) fn port_scene(id: &str, incoming: bool, position: Vec2) -> BoxedScene {
    boxed(bsn! {
        Node {
            position_type: PositionType::Absolute,
            left: {px(position.x - 7.0)}, top: {px(position.y - 7.0)},
            width: px(14), height: px(14), border: {UiRect::all(px(2))},
            border_radius: {BorderRadius::all(px(7))}, display: Display::None,
        }
        template_value(CanvasPort { id: id.to_owned(), incoming })
        ThemeBackgroundColor({theme::CARD_BG})
        ThemeBorderColor({theme::GRAPH_SELECTED_BORDER})
    })
}

/// 在图重建前同步稳定身份。改名不会丢掉子图位置，删除当前子图回到最近有效祖先。
pub(super) fn prepare(
    editor: Res<Editor>,
    mut nav: ResMut<GraphNav>,
    mut state: ResMut<CanvasState>,
    mut editing: ResMut<GraphEditing>,
    guard: Option<Res<DocumentGuard>>,
) {
    // 模式切换和确认层可能在同帧 Update 才生效，重建前取消，不能留下上一帧编辑预览。
    let guarded = guard.as_ref().is_some_and(|guard| guard.active());
    if guarded
        || (!editing.enabled && !matches!(state.gesture, Gesture::Idle | Gesture::Pan { .. }))
    {
        cancel_gesture(&mut state, &editor, &mut nav, &mut editing);
    }
    if guarded {
        state.fit_requested = false;
    }
    if state.document != Some(editor.document_version) {
        *state = CanvasState {
            document: Some(editor.document_version),
            ..default()
        };
        editing.selected_nodes.clear();
    }
    let Some(data) = &editor.data else {
        return;
    };
    let topology = data.graph_edit.topology_revision();
    if state.topology != Some(topology) {
        let had_topology = state.topology.is_some();
        if let Some(active) = state.active {
            let surviving = std::iter::once(active)
                .chain(state.remembered_graphs.iter().rev().copied())
                .find_map(|key| data.graph_edit.graph_path(key));
            if let Some(path) = surviving {
                nav.path = path;
            }
        }
        if let Some(key) = state.selected_node {
            nav.selected = data.graph_edit.node_id(key).map(str::to_owned);
        }
        if let Some(key) = state.selected_edge
            && let Some(graph) = data.graph.subgraph_at(&nav.path)
            && let Some(graph_key) = data.graph_edit.graph_key_at(&nav.path)
        {
            nav.selected_edge = (0..graph.edges.len())
                .find(|&index| data.graph_edit.edge_key_at(graph_key, index) == Some(key));
        }
        editing
            .selected_nodes
            .retain(|&key| data.graph_edit.node_id(key).is_some());
        state.topology = Some(topology);
        state.cancel();
        state.geometry_revision += 1;
        if had_topology {
            nav.version += 1;
        }
        state.apply_view = true;
    }
    let Some(key) = data.graph_edit.graph_key_at(&nav.path) else {
        return;
    };
    if state.active != Some(key) {
        state.cancel();
        state.active = Some(key);
        state.views.entry(key).or_default();
        state.remembered_graphs = (0..nav.path.len())
            .filter_map(|depth| data.graph_edit.graph_key_at(&nav.path[..depth]))
            .collect();
        editing.selected_nodes.clear();
        state.geometry_revision += 1;
        state.apply_view = true;
    }
    let selected_node = nav
        .selected
        .as_deref()
        .and_then(|id| data.graph_edit.node_key(key, id));
    let selected_edge = nav
        .selected_edge
        .and_then(|index| data.graph_edit.edge_key_at(key, index));
    if state.selected_node != selected_node {
        state.selected_node = selected_node;
    }
    if state.selected_edge != selected_edge {
        state.selected_edge = selected_edge;
    }
    let Some(current) = data.graph.subgraph_at(&nav.path) else {
        return;
    };
    // 仅在图或手势位置变化时计算，普通属性输入不会重新路由。
    let revision = (key, state.geometry_revision);
    if state.computed_geometry == Some(revision) {
        return;
    }
    state.computed_geometry = Some(revision);
    let mut positions = state
        .view()
        .map(|view| view.positions.clone())
        .unwrap_or_default();
    if let Gesture::Move {
        preview,
        moved: true,
        ..
    } = &state.gesture
    {
        positions.extend(preview.iter().map(|(&key, &value)| (key, value)));
    }
    let positions = positions
        .into_iter()
        .filter_map(|(key, value)| {
            data.graph_edit
                .node_id(key)
                .map(|id| (id.to_owned(), value))
        })
        .collect();
    match graph_layout::layout_with_positions(current, &positions) {
        Ok(layout) => {
            state.layout = layout;
            state.layout_error = None;
        }
        Err(error) => {
            state.layout = graph_layout::layout(current).unwrap_or_default();
            state.layout_error = Some(error.to_string());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn canvas_action(
    event: On<Activate>,
    actions: Query<&CanvasAction>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    editor: Res<Editor>,
    mut nav: ResMut<GraphNav>,
    mut editing: ResMut<GraphEditing>,
    mut state: ResMut<CanvasState>,
    viewports: Query<&ComputedNode, With<CanvasViewport>>,
    guard: Option<Res<DocumentGuard>>,
) {
    if guard.as_ref().is_some_and(|guard| guard.active()) {
        cancel_gesture(&mut state, &editor, &mut nav, &mut editing);
        return;
    }
    let Ok(action) = actions.get(event.entity) else {
        return;
    };
    if !current_scene(event.entity, nav.version, &parents, &versions) {
        return;
    }
    let size = viewports
        .single()
        .map(|node| node.size() * node.inverse_scale_factor)
        .unwrap_or(Vec2::ZERO);
    match *action {
        CanvasAction::Fit => state.fit_requested = true,
        CanvasAction::Auto => {
            state.cancel();
            if let Some(view) = state.view_mut() {
                view.positions.clear();
            }
            state.geometry_revision += 1;
        }
        CanvasAction::ZoomIn | CanvasAction::ZoomOut | CanvasAction::Actual => {
            let factor = match action {
                CanvasAction::ZoomIn => 1.2,
                CanvasAction::ZoomOut => 1.0 / 1.2,
                _ => 1.0 / state.zoom(),
            };
            if let Some(view) = state.view_mut() {
                zoom_at(view, size / 2.0, factor);
            }
            state.apply_view = true;
        }
        CanvasAction::ReconnectFrom | CanvasAction::ReconnectTo => {
            if !editing.enabled {
                return;
            }
            let Some(data) = &editor.data else {
                return;
            };
            let Some(graph) = data.graph.subgraph_at(&nav.path) else {
                return;
            };
            let Some(index) = nav.selected_edge else {
                return;
            };
            let Some(edge) = graph.edges.get(index) else {
                return;
            };
            let Some(key) = state
                .active
                .and_then(|key| data.graph_edit.edge_key_at(key, index))
            else {
                return;
            };
            state.gesture = Gesture::Reconnect {
                edge: key,
                from: edge.from.clone(),
                to: edge.to.clone(),
                incoming: matches!(action, CanvasAction::ReconnectTo),
                cursor: Vec2::ZERO,
            };
            state.geometry_revision += 1;
        }
    }
}

fn zoom_at(view: &mut LayerView, anchor: Vec2, factor: f32) {
    let graph_anchor = (anchor + view.scroll) / view.zoom;
    view.zoom = (view.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    view.scroll = (graph_anchor * view.zoom - anchor).max(Vec2::ZERO);
}

/// 使用拾取后端的最终悬停结果，避免菜单、弹出层覆盖画布时仍按底层几何接收输入。
fn pointer_targets_canvas(
    hovered: Option<&HoverMap>,
    viewport: Entity,
    parents: &Query<&ChildOf>,
    cameras: &Query<&Camera>,
) -> bool {
    // 无拾取插件的纯几何测试仍能独立执行；正式 UI 始终提供 HoverMap。
    let Some(hovered) = hovered else {
        return true;
    };
    let Some(hits) = hovered.get(&PointerId::Mouse) else {
        return false;
    };
    let top = hits.iter().min_by(|(_, a), (_, b)| {
        let a_order = cameras.get(a.camera).map_or(0, |camera| camera.order);
        let b_order = cameras.get(b.camera).map_or(0, |camera| camera.order);
        b_order
            .cmp(&a_order)
            .then_with(|| a.depth.total_cmp(&b.depth))
    });
    top.is_some_and(|(&entity, _)| {
        widgets::self_or_ancestor(entity, parents, |entity| entity == viewport).is_some()
    })
}

/// 输入只在当前可见画布矩形内开始；捕获后的拖动可以越界继续，松开或 Esc 必定结束。
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_input(
    windows: Query<&Window>,
    viewports: Query<(
        Entity,
        &CanvasViewport,
        &ComputedNode,
        &UiGlobalTransform,
        &ScrollPosition,
    )>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    mode: Res<ViewMode>,
    editor: Res<Editor>,
    mut nav: ResMut<GraphNav>,
    mut editing: ResMut<GraphEditing>,
    mut state: ResMut<CanvasState>,
    mut actions: MessageWriter<AppAction>,
    guard: Option<Res<DocumentGuard>>,
    hovered: Option<Res<HoverMap>>,
    parents: Query<&ChildOf>,
    cameras: Query<&Camera>,
) {
    if guard.as_ref().is_some_and(|guard| guard.active()) {
        cancel_gesture(&mut state, &editor, &mut nav, &mut editing);
        wheel.clear();
        return;
    }
    if *mode != ViewMode::Graph {
        cancel_gesture(&mut state, &editor, &mut nav, &mut editing);
        wheel.clear();
        return;
    }
    if !editing.enabled && !matches!(state.gesture, Gesture::Idle | Gesture::Pan { .. }) {
        cancel_gesture(&mut state, &editor, &mut nav, &mut editing);
    }
    let Ok(window) = windows.single() else {
        wheel.clear();
        return;
    };
    let Ok((viewport_entity, viewport, computed, transform, scroll)) = viewports.single() else {
        wheel.clear();
        return;
    };
    if viewport.0 != nav.version || state.document != Some(editor.document_version) {
        cancel_gesture(&mut state, &editor, &mut nav, &mut editing);
        wheel.clear();
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || !window.focused {
        cancel_gesture(&mut state, &editor, &mut nav, &mut editing);
        wheel.clear();
        return;
    }
    let Some(physical) = window.physical_cursor_position() else {
        let released = match &state.gesture {
            Gesture::Move { .. } | Gesture::Box { .. } | Gesture::Connect { .. } => {
                !mouse.pressed(MouseButton::Left)
            }
            Gesture::Pan { button, .. } => !mouse.pressed(*button),
            _ => false,
        };
        if released {
            cancel_gesture(&mut state, &editor, &mut nav, &mut editing);
        }
        wheel.clear();
        return;
    };
    let coordinates = Coordinates {
        origin_physical: transform.affine().translation - computed.size() / 2.0,
        inverse_scale: computed.inverse_scale_factor,
        scroll: scroll.0,
        zoom: state.zoom(),
    };
    let logical = coordinates.logical(physical);
    let point = coordinates.graph(physical);
    let size = computed.size() * computed.inverse_scale_factor;
    let inside = logical.cmpge(Vec2::ZERO).all() && logical.cmplt(size).all();
    let targets_canvas =
        inside && pointer_targets_canvas(hovered.as_deref(), viewport_entity, &parents, &cameras);
    if !state.apply_view
        && let Some(view) = state.view_mut()
    {
        view.scroll = scroll.0;
    }
    let modifier = keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    for event in wheel.read() {
        if !targets_canvas || shift || !matches!(state.gesture, Gesture::Idle) {
            continue;
        }
        if let Some(view) = state.view_mut() {
            match modifier {
                true => zoom_at(view, logical, (event.y.clamp(-20.0, 20.0) * 0.08).exp()),
                false => {
                    let scale = match event.unit {
                        MouseScrollUnit::Line => 36.0,
                        MouseScrollUnit::Pixel => 1.0,
                    };
                    view.scroll =
                        (view.scroll - Vec2::new(event.x, event.y) * scale).max(Vec2::ZERO);
                }
            }
            state.apply_view = true;
        }
    }
    let Some(data) = &editor.data else {
        return;
    };
    let Some(graph_key) = state.active else {
        return;
    };
    let pressed = mouse.just_pressed(MouseButton::Left);
    // 中键和空格始终优先平移，包括编辑模式下的节点区域。
    if targets_canvas
        && (mouse.just_pressed(MouseButton::Middle) || (pressed && keys.pressed(KeyCode::Space)))
    {
        state.gesture = Gesture::Pan {
            start: logical,
            scroll: scroll.0,
            button: if mouse.just_pressed(MouseButton::Middle) {
                MouseButton::Middle
            } else {
                MouseButton::Left
            },
        };
    } else if targets_canvas && pressed && matches!(state.gesture, Gesture::Idle) && editing.enabled
    {
        if let Some((id, incoming)) = hit_port(&state.layout, point, state.zoom()) {
            state.gesture = Gesture::Connect {
                id,
                incoming,
                cursor: point,
            };
        } else if let Some(node) =
            hit_node(&state.layout, point).filter(|node| node.slot == NodeSlot::Real)
        {
            let id = node.id.clone();
            if let Some(key) = data.graph_edit.node_key(graph_key, &id) {
                match modifier || shift {
                    true => {
                        if !editing.selected_nodes.insert(key) {
                            editing.selected_nodes.remove(&key);
                        }
                    }
                    false if !editing.selected_nodes.contains(&key) => {
                        editing.selected_nodes.clear();
                        editing.selected_nodes.insert(key);
                    }
                    false => {}
                }
                nav.selected = match editing.selected_nodes.contains(&key) {
                    true => Some(id),
                    false => editing
                        .selected_nodes
                        .iter()
                        .next_back()
                        .and_then(|&key| data.graph_edit.node_id(key))
                        .map(str::to_owned),
                };
                nav.selected_edge = None;
                let original: BTreeMap<NodeKey, Vec2> = state
                    .layout
                    .nodes
                    .iter()
                    .filter_map(|node| {
                        let key = data.graph_edit.node_key(graph_key, &node.id)?;
                        editing
                            .selected_nodes
                            .contains(&key)
                            .then_some((key, node.pos))
                    })
                    .collect();
                state.gesture = Gesture::Move {
                    start: point,
                    preview: original.clone(),
                    original,
                    moved: false,
                };
            }
        } else if let Some(index) = hit_edge(&state.layout, point, state.zoom()) {
            editing.selected_nodes.clear();
            nav.selected = None;
            nav.selected_edge = Some(index);
        } else {
            let original = editing.selected_nodes.clone();
            if !modifier && !shift {
                editing.selected_nodes.clear();
            }
            nav.selected = None;
            nav.selected_edge = None;
            state.gesture = Gesture::Box {
                start: point,
                end: point,
                original,
                additive: modifier || shift,
            };
        }
        state.geometry_revision += 1;
    }
    let mut gesture = std::mem::take(&mut state.gesture);
    let mut keep = true;
    match &mut gesture {
        Gesture::Move {
            start,
            original,
            preview,
            moved,
        } => {
            let delta = point - *start;
            if delta.length() * state.zoom() >= DRAG_THRESHOLD {
                *moved = true;
            }
            if *moved {
                let minimum = original
                    .values()
                    .copied()
                    .fold(Vec2::splat(f32::INFINITY), Vec2::min);
                let delta = delta.max(Vec2::splat(32.0) - minimum);
                let candidate: BTreeMap<NodeKey, Vec2> = original
                    .iter()
                    .map(|(&key, &pos)| (key, pos + delta))
                    .collect();
                if !collides(&candidate, &state.layout, |id| {
                    data.graph_edit.node_key(graph_key, id)
                }) && *preview != candidate
                {
                    *preview = candidate;
                    state.geometry_revision += 1;
                }
            }
            if mouse.just_released(MouseButton::Left) {
                if *moved && let Some(view) = state.view_mut() {
                    view.positions
                        .extend(preview.iter().map(|(&key, &pos)| (key, pos)));
                }
                keep = false;
            }
        }
        Gesture::Box {
            start,
            end,
            original,
            additive,
        } => {
            *end = point;
            let bounds = Rect::from_corners(*start, *end);
            editing.selected_nodes = if *additive {
                original.clone()
            } else {
                BTreeSet::new()
            };
            for node in &state.layout.nodes {
                if bounds.contains(node.pos)
                    && bounds.contains(node.pos + Vec2::new(NODE_W, NODE_H))
                    && let Some(key) = data.graph_edit.node_key(graph_key, &node.id)
                {
                    editing.selected_nodes.insert(key);
                }
            }
            state.geometry_revision += 1;
            if mouse.just_released(MouseButton::Left) {
                sync_selection_focus(&editor, &mut nav, &editing);
                keep = false;
            }
        }
        Gesture::Pan {
            start,
            scroll,
            button,
        } => {
            if let Some(view) = state.view_mut() {
                view.scroll = (*scroll - (logical - *start)).max(Vec2::ZERO);
            }
            state.apply_view = true;
            if mouse.just_released(*button) {
                keep = false;
            }
        }
        Gesture::Connect {
            id,
            incoming,
            cursor,
        } => {
            *cursor = point;
            state.geometry_revision += 1;
            if mouse.just_released(MouseButton::Left) {
                if targets_canvas
                    && let Some((other, other_incoming)) =
                        hit_port(&state.layout, point, state.zoom())
                    && *incoming != other_incoming
                {
                    let (from, to) = if *incoming {
                        (other, id.clone())
                    } else {
                        (id.clone(), other)
                    };
                    submit_command(
                        &editor,
                        GraphCommand::AddEdge {
                            graph: graph_key,
                            value: serde_json::json!({"from":from,"to":to}),
                        },
                        &mut actions,
                    );
                }
                keep = false;
            }
        }
        Gesture::Reconnect {
            edge,
            from,
            to,
            incoming,
            cursor,
        } => {
            *cursor = point;
            state.geometry_revision += 1;
            if targets_canvas
                && pressed
                && let Some((id, port_incoming)) = hit_port(&state.layout, point, state.zoom())
                && port_incoming == *incoming
            {
                let (from, to) = if *incoming {
                    (from.clone(), id)
                } else {
                    (id, to.clone())
                };
                submit_command(
                    &editor,
                    GraphCommand::ReconnectEdge {
                        edge: *edge,
                        from,
                        to,
                    },
                    &mut actions,
                );
                keep = false;
            }
        }
        Gesture::Idle => {}
    }
    if keep {
        state.gesture = gesture;
    } else {
        state.geometry_revision += 1;
    }
}

fn hit_node(layout: &GraphLayout, point: Vec2) -> Option<&graph_layout::PlacedNode> {
    layout.nodes.iter().rev().find(|node| {
        Rect::from_corners(node.pos, node.pos + Vec2::new(NODE_W, NODE_H)).contains(point)
    })
}

fn hit_port(layout: &GraphLayout, point: Vec2, zoom: f32) -> Option<(String, bool)> {
    layout
        .nodes
        .iter()
        .flat_map(|node| {
            [false, true].into_iter().filter_map(move |incoming| {
                if (node.slot == NodeSlot::Entry && incoming)
                    || (node.slot == NodeSlot::Exit && !incoming)
                {
                    return None;
                }
                let position =
                    node.pos + Vec2::new(NODE_W / 2.0, if incoming { 0.0 } else { NODE_H });
                Some((node.id.clone(), incoming, position.distance(point)))
            })
        })
        .filter(|(_, _, distance)| *distance <= PORT_RADIUS / zoom.min(1.0))
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(id, incoming, _)| (id, incoming))
}

fn hit_edge(layout: &GraphLayout, point: Vec2, zoom: f32) -> Option<usize> {
    layout
        .edges
        .iter()
        .flat_map(|edge| {
            edge.points.windows(2).map(move |pair| {
                let delta = pair[1] - pair[0];
                let t = ((point - pair[0]).dot(delta) / delta.length_squared()).clamp(0.0, 1.0);
                (edge.edge_index, point.distance(pair[0] + delta * t))
            })
        })
        .filter(|(_, distance)| *distance <= EDGE_HIT_W / 2.0 / zoom.min(1.0))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

fn collides(
    positions: &BTreeMap<NodeKey, Vec2>,
    layout: &GraphLayout,
    key_for: impl Fn(&str) -> Option<NodeKey>,
) -> bool {
    positions.values().any(|&position| {
        layout.nodes.iter().any(|node| {
            if key_for(&node.id).is_some_and(|key| positions.contains_key(&key)) {
                return false;
            }
            let overlap = (position - node.pos).abs();
            overlap.x < NODE_W + 24.0 && overlap.y < NODE_H + 24.0
        })
    })
}

/// 更新节点、端口、缩放与滚动组件，不销毁画布根、详情或正在编辑的表单。
#[allow(clippy::type_complexity)]
pub(super) fn sync_geometry(
    mut state: ResMut<CanvasState>,
    editing: Res<GraphEditing>,
    nav: Res<GraphNav>,
    mut queries: ParamSet<(
        Query<(&CanvasViewport, &ComputedNode, &mut ScrollPosition)>,
        Query<&mut Node, With<CanvasExtent>>,
        Query<(&mut Node, &mut UiTransform), With<CanvasContent>>,
        Query<(&CanvasItem, &mut Node)>,
        Query<(&CanvasPort, &mut Node)>,
    )>,
    mut labels: Query<
        (&mut Text, Has<CanvasZoomLabel>),
        Or<(With<CanvasZoomLabel>, With<CanvasLayoutWarning>)>,
    >,
) {
    let zoom = state.zoom();
    for (mut label, zoom_label) in &mut labels {
        let wanted = match zoom_label {
            true => format!("{:.0}%", zoom * 100.0),
            false => state
                .layout_error
                .as_ref()
                .map(|error| format!("{error}；当前显示自动布局。"))
                .unwrap_or_default(),
        };
        if label.0 != wanted {
            label.0 = wanted;
        }
    }
    for (viewport, computed, mut scroll) in &mut queries.p0() {
        if viewport.0 != nav.version {
            continue;
        }
        let size = computed.size() * computed.inverse_scale_factor;
        if size.min_element() <= 0.0 {
            continue;
        }
        let layout_size = state.layout.size;
        if state.viewport_size != size {
            state.viewport_size = size;
        }
        if state.fit_requested {
            state.cancel();
            if let Some(view) = state.view_mut() {
                view.zoom = ((size.x - 32.0) / layout_size.x)
                    .min((size.y - 32.0) / layout_size.y)
                    .clamp(MIN_ZOOM, MAX_ZOOM);
                view.scroll = Vec2::ZERO;
                view.initialized = true;
            }
            state.fit_requested = false;
            state.apply_view = true;
        }
        if let Some(view) = state.view_mut()
            && !view.initialized
        {
            view.scroll.x = ((layout_size.x * view.zoom - size.x) / 2.0).max(0.0);
            view.initialized = true;
            state.apply_view = true;
        }
        if state.apply_view {
            if let Some(view) = state.view_mut() {
                view.scroll = view
                    .scroll
                    .clamp(Vec2::ZERO, (layout_size * view.zoom - size).max(Vec2::ZERO));
                scroll.0 = view.scroll;
            }
            state.apply_view = false;
        }
    }
    let zoom = state.zoom();
    let size = state.layout.size;
    let extent = (size * zoom).max(state.viewport_size);
    for mut node in &mut queries.p1() {
        if node.width != px(extent.x) {
            node.width = px(extent.x);
        }
        if node.height != px(extent.y) {
            node.height = px(extent.y);
        }
    }
    for (mut node, mut transform) in &mut queries.p2() {
        if node.width != px(size.x) {
            node.width = px(size.x);
        }
        if node.height != px(size.y) {
            node.height = px(size.y);
        }
        if node.left != px(0) {
            node.left = px(0);
        }
        if node.top != px(0) {
            node.top = px(0);
        }
        if transform.scale != Vec2::splat(zoom) {
            transform.scale = Vec2::splat(zoom);
        }
        let translation = Val2::px(size.x * (zoom - 1.0) / 2.0, size.y * (zoom - 1.0) / 2.0);
        if transform.translation != translation {
            transform.translation = translation;
        }
    }
    for (item, mut node) in &mut queries.p3() {
        if let Some(placed) = state.layout.nodes.iter().find(|placed| placed.id == item.0) {
            if node.left != px(placed.pos.x) {
                node.left = px(placed.pos.x);
            }
            if node.top != px(placed.pos.y) {
                node.top = px(placed.pos.y);
            }
        }
    }
    for (port, mut node) in &mut queries.p4() {
        let display = if editing.enabled {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        if let Some(placed) = state
            .layout
            .nodes
            .iter()
            .find(|placed| placed.id == port.id)
        {
            node.left = px(placed.pos.x + NODE_W / 2.0 - 7.0);
            node.top = px(placed.pos.y + if port.incoming { 0.0 } else { NODE_H } - 7.0);
        }
    }
}

/// 线段使用普通 ECS 子实体立即替换，避免每次拖动都排入跨帧 BSN 场景队列。
pub(super) fn redraw_edges(
    state: Res<CanvasState>,
    nav: Res<GraphNav>,
    mut slots: Query<(Entity, &mut CanvasEdges)>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    mut commands: Commands,
) {
    for (slot, mut marker) in &mut slots {
        if !current_scene(slot, nav.version, &parents, &versions) {
            continue;
        }
        let revision = (nav.version, state.geometry_revision);
        if marker.rendered == Some(revision) {
            continue;
        }
        marker.rendered = Some(revision);
        commands.entity(slot).despawn_related::<Children>();
        for edge in &state.layout.edges {
            let token = if edge.back {
                theme::GRAPH_BACK
            } else {
                theme::GRAPH_BORDER
            };
            for pair in edge.points.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                let entity = commands
                    .spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(a.x.min(b.x) - EDGE_HIT_W / 2.0),
                            top: px(a.y.min(b.y) - EDGE_HIT_W / 2.0),
                            width: px((a.x - b.x).abs() + EDGE_HIT_W),
                            height: px((a.y - b.y).abs() + EDGE_HIT_W),
                            ..default()
                        },
                        Button,
                        GraphEdgeMarker {
                            index: edge.edge_index,
                            version: nav.version,
                        },
                        ChildOf(slot),
                    ))
                    .id();
                commands.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px((EDGE_HIT_W - EDGE_W) / 2.0),
                        top: px((EDGE_HIT_W - EDGE_W) / 2.0),
                        width: px((a.x - b.x).abs().max(EDGE_W)),
                        height: px((a.y - b.y).abs().max(EDGE_W)),
                        ..default()
                    },
                    GraphEdgeVisual {
                        index: edge.edge_index,
                        back: edge.back,
                    },
                    ThemeBackgroundColor(token.clone()),
                    ChildOf(entity),
                ));
            }
            if let Some(end) = edge.points.last()
                && let Some(prev) = edge.points.iter().rev().nth(1)
            {
                let (glyph, offset) = match (
                    (end.y - prev.y).abs() < f32::EPSILON,
                    end.x < prev.x,
                    end.y < prev.y,
                ) {
                    (true, true, _) => ("◀", Vec2::new(-9.0, -6.0)),
                    (true, false, _) => ("▶", Vec2::new(-3.0, -6.0)),
                    (false, _, true) => ("▲", Vec2::new(-5.0, -9.0)),
                    (false, _, false) => ("▼", Vec2::new(-5.0, -4.0)),
                };
                commands.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(end.x + offset.x),
                        top: px(end.y + offset.y),
                        ..default()
                    },
                    Text::new(glyph),
                    TextFont {
                        font_size: px(9).into(),
                        ..default()
                    },
                    GraphEdgeVisual {
                        index: edge.edge_index,
                        back: edge.back,
                    },
                    ThemeTextColor(token),
                    ChildOf(slot),
                ));
            }
        }
        let mut preview_line = |a: Vec2, b: Vec2| {
            commands.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: px(a.x.min(b.x)),
                    top: px(a.y.min(b.y)),
                    width: px((a.x - b.x).abs().max(2.0)),
                    height: px((a.y - b.y).abs().max(2.0)),
                    ..default()
                },
                CanvasOverlay,
                ThemeBackgroundColor(theme::GRAPH_SELECTED_BORDER),
                Pickable::IGNORE,
                ChildOf(slot),
            ));
        };
        let connection = match &state.gesture {
            Gesture::Connect {
                id,
                incoming,
                cursor,
            } => Some((id, *incoming, *cursor)),
            Gesture::Reconnect {
                from,
                incoming: true,
                cursor,
                ..
            } => Some((from, false, *cursor)),
            Gesture::Reconnect {
                to,
                incoming: false,
                cursor,
                ..
            } => Some((to, true, *cursor)),
            _ => None,
        };
        if let Some((id, incoming, cursor)) = connection
            && let Some(node) = state.layout.nodes.iter().find(|node| &node.id == id)
        {
            let start = node.pos + Vec2::new(NODE_W / 2.0, if incoming { 0.0 } else { NODE_H });
            let middle = Vec2::new(start.x, cursor.y);
            preview_line(start, middle);
            preview_line(middle, cursor);
        }
        if let Gesture::Box { start, end, .. } = &state.gesture {
            let min = start.min(*end);
            let max = start.max(*end);
            preview_line(min, Vec2::new(max.x, min.y));
            preview_line(Vec2::new(max.x, min.y), max);
            preview_line(max, Vec2::new(min.x, max.y));
            preview_line(Vec2::new(min.x, max.y), min);
        }
    }
}

fn click_target(
    entity: Entity,
    cards: &Query<&GraphNodeMarker>,
    parents: &Query<&ChildOf>,
    versions: &Query<&GraphSceneVersion>,
    editor: &Editor,
    nav: &GraphNav,
) -> Option<ClickTarget> {
    let card = widgets::self_or_ancestor(entity, parents, |entity| cards.contains(entity))?;
    if !current_scene(card, nav.version, parents, versions) {
        return None;
    }
    let marker = cards.get(card).ok()?;
    let data = editor.data.as_ref()?;
    let graph = data.graph_edit.graph_key_at(&nav.path)?;
    Some(ClickTarget {
        node: data.graph_edit.node_key(graph, &marker.0)?,
        document: editor.document_version,
        scene: nav.version,
    })
}

fn click_has_modifier(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::Space,
    ])
}

#[allow(clippy::too_many_arguments)]
fn track_node_press(
    mut event: On<Pointer<Press>>,
    cards: Query<&GraphNodeMarker>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    editor: Res<Editor>,
    nav: Res<GraphNav>,
    mut clicks: ResMut<NodeClicks>,
) {
    if event.pointer_id != PointerId::Mouse
        || event.button != bevy::picking::pointer::PointerButton::Primary
    {
        return;
    }
    let Some(target) = click_target(event.entity, &cards, &parents, &versions, &editor, &nav)
    else {
        clicks.press = None;
        clicks.last = None;
        return;
    };
    event.propagate(false);
    clicks.press = Some(CardPress {
        target,
        position: event.pointer_location.position,
        dragged: false,
    });
}

fn track_node_drag(
    event: On<Pointer<Drag>>,
    scale: Option<Res<UiScale>>,
    mut clicks: ResMut<NodeClicks>,
) {
    if event.pointer_id == PointerId::Mouse
        && event.button == bevy::picking::pointer::PointerButton::Primary
        && event.distance.length() / scale.as_ref().map_or(1.0, |scale| scale.0) >= DRAG_THRESHOLD
        && let Some(press) = &mut clicks.press
    {
        press.dragged = true;
    }
}

/// 原生Click按最内层实体计数；这里按稳定节点身份归并相邻点击，文字与卡片空白可以组合双击。
#[allow(clippy::too_many_arguments)]
fn enter_node_on_double_click(
    mut event: On<Pointer<Click>>,
    cards: Query<&GraphNodeMarker>,
    parents: Query<&ChildOf>,
    versions: Query<&GraphSceneVersion>,
    editor: Res<Editor>,
    nav: Res<GraphNav>,
    mode: Res<ViewMode>,
    state: Res<CanvasState>,
    keys: Res<ButtonInput<KeyCode>>,
    scale: Option<Res<UiScale>>,
    settings: Option<Res<bevy::picking::PickingSettings>>,
    guard: Option<Res<DocumentGuard>>,
    mut clicks: ResMut<NodeClicks>,
    mut actions: MessageWriter<AppAction>,
) {
    if event.pointer_id != PointerId::Mouse
        || event.button != bevy::picking::pointer::PointerButton::Primary
    {
        return;
    }
    let Some(target) = click_target(event.entity, &cards, &parents, &versions, &editor, &nav)
    else {
        clicks.last = None;
        return;
    };
    event.propagate(false);
    let press = clicks.press.take();
    let previous = clicks.last.take();
    if *mode != ViewMode::Graph
        || guard.as_ref().is_some_and(|guard| guard.active())
        || click_has_modifier(&keys)
        || !matches!(
            state.gesture,
            Gesture::Idle | Gesture::Move { moved: false, .. }
        )
        || !press.is_some_and(|press| {
            press.target == target
                && !press.dragged
                && press.position.distance(event.pointer_location.position)
                    / scale.as_ref().map_or(1.0, |scale| scale.0)
                    < DRAG_THRESHOLD
        })
    {
        return;
    }
    let Some(data) = &editor.data else {
        return;
    };
    let Some(id) = data.graph_edit.node_id(target.node) else {
        return;
    };
    if !data
        .graph
        .subgraph_at(&nav.path)
        .and_then(|graph| graph.node(id))
        .is_some_and(|node| node.children.is_some())
    {
        return;
    }
    let now = std::time::Instant::now();
    let interval = settings.as_ref().map_or_else(
        || bevy::picking::PickingSettings::default().multi_click_interval,
        |settings| settings.multi_click_interval,
    );
    if !previous.is_some_and(|(last, when)| last == target && now.duration_since(when) <= interval)
    {
        clicks.last = Some((target, now));
        return;
    }
    actions.write(AppAction::GraphEdit(GraphIntent {
        document_version: editor.document_version,
        revision: data.graph_edit.revision(),
        operation: GraphOperation::Control(GraphControl::EnterSubgraph(target.node)),
    }));
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<CanvasState>()
        .init_resource::<NodeClicks>()
        .init_resource::<GraphEditing>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .add_message::<MouseWheel>()
        .add_message::<AppAction>()
        .add_observer(canvas_action)
        .add_observer(track_node_press)
        .add_observer(track_node_drag)
        .add_observer(enter_node_on_double_click);
}

pub(super) fn sync_controls(
    editor: Res<Editor>,
    editing: Res<GraphEditing>,
    nav: Res<GraphNav>,
    state: Res<CanvasState>,
    actions: Query<(Entity, &CanvasAction, Has<bevy::ui::InteractionDisabled>)>,
    mut commands: Commands,
    guard: Option<Res<DocumentGuard>>,
) {
    for (entity, action, disabled) in &actions {
        let allowed = !guard.as_ref().is_some_and(|guard| guard.active())
            && match action {
                CanvasAction::ReconnectFrom | CanvasAction::ReconnectTo => {
                    editing.enabled && nav.selected_edge.is_some()
                }
                CanvasAction::Auto => state.view().is_some_and(|view| !view.positions.is_empty()),
                _ => editor.data.is_some() && state.layout_error.is_none(),
            };
        match (allowed, disabled) {
            (true, true) => {
                commands
                    .entity(entity)
                    .remove::<bevy::ui::InteractionDisabled>();
            }
            (false, false) => {
                commands
                    .entity(entity)
                    .insert(bevy::ui::InteractionDisabled);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
