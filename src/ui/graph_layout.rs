//! 任务图的分层布局
//!
//! 输入一层子图，输出每个节点的位置和每条边的折线点。纯计算，不碰 UI，
//! 便于单测。渲染在 [`super::graph_view`]。
//!
//! 算法是分层图布局的简化版：按最长路径给节点定层，同层水平居中排开，
//! 层间按水平区间分配通道并自动留出足够间距，端口与侧通道保留每条边的身份。
//! 边走正交折线。`loop` 节点的循环体带回边，直接拓扑排序会因为有环排不出层级，
//! 所以先用 DFS 把回边挑出来、定层时当它不存在，渲染时再用强调色标记——
//! 层次排得出来，循环关系也一眼看得见。

use bevy::math::Vec2;
use std::collections::{HashMap, HashSet};

use crate::model::{ENTRY_ID, EXIT_ID, GraphEdge, SubGraph};

mod manual;
pub use manual::layout_with_positions;

/// 节点框宽度
pub const NODE_W: f32 = 230.0;

/// 节点框高度
pub const NODE_H: f32 = 56.0;

/// 同层节点的水平间距
const GAP_X: f32 = 28.0;

/// 层与层之间的垂直间距
const GAP_Y: f32 = 48.0;

/// 画布四周留白
const PAD: f32 = 32.0;

/// 避障通道与所跨区域右边界的距离
const SIDE_BULGE: f32 = 36.0;

/// 纵向范围重叠的通道之间的距离
const LANE_GAP: f32 = 12.0;

/// 节点在图里的角色
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeSlot {
    /// 真实节点
    Real,
    /// 虚拟入口
    Entry,
    /// 虚拟出口
    Exit,
}

/// 排好位置的节点
#[derive(Debug, Clone)]
pub struct PlacedNode {
    /// 节点 id
    pub id: String,
    /// 左上角坐标
    pub pos: Vec2,
    /// 角色
    pub slot: NodeSlot,
}

/// 排好位置的边
#[derive(Debug, Clone)]
pub struct PlacedEdge {
    /// 在原始 `SubGraph::edges` 中的位置，过滤悬空边后也不改变身份
    pub edge_index: usize,
    /// 折线顶点，至少两个；相邻两点必定共享 x 或 y（正交）
    pub points: Vec<Vec2>,
    /// 是否为把流程绕回去的回边
    pub back: bool,
}

/// 一层图的布局结果
#[derive(Debug, Clone, Default)]
pub struct GraphLayout {
    /// 所有节点（含虚拟入口出口）
    pub nodes: Vec<PlacedNode>,
    /// 所有边
    pub edges: Vec<PlacedEdge>,
    /// 画布尺寸
    pub size: Vec2,
}

/// 节点身份不明确时不能安全地构造邻接关系或选择目标。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("节点 ID 为空或只有空白")]
    Empty { node_index: usize },
    #[error("节点使用了保留 ID：{id}")]
    Reserved { node_index: usize, id: String },
    #[error("节点 ID {id} 在本层重复")]
    Duplicate {
        first_index: usize,
        node_index: usize,
        id: String,
    },
    #[error("第 {} 条连接无法在当前手工位置之间走线，请恢复自动布局", edge_index + 1)]
    Unroutable { edge_index: usize },
}

fn validate_node_ids(graph: &SubGraph) -> Result<(), LayoutError> {
    let mut seen = HashMap::new();
    for (node_index, node) in graph.nodes.iter().enumerate() {
        match node.id.as_str() {
            id if id.trim().is_empty() => return Err(LayoutError::Empty { node_index }),
            ENTRY_ID | EXIT_ID => {
                return Err(LayoutError::Reserved {
                    node_index,
                    id: node.id.clone(),
                });
            }
            id => {
                if let Some(first_index) = seen.insert(id, node_index) {
                    return Err(LayoutError::Duplicate {
                        first_index,
                        node_index,
                        id: node.id.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// 找出所有回边
///
/// DFS 过程中指向仍在递归栈上的节点，就是把图绕回去的那条边。
fn back_edges(ids: &[String], adj: &HashMap<&str, Vec<&str>>) -> HashSet<(String, String)> {
    // 0 未访问，1 在栈上，2 已完成
    let mut state: HashMap<&str, u8> = ids.iter().map(|id| (id.as_str(), 0u8)).collect();
    let mut back = HashSet::new();
    // 显式栈，避免深图递归爆栈
    for start in ids {
        if state.get(start.as_str()) != Some(&0) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(start.as_str(), 0)];
        state.insert(start.as_str(), 1);
        while let Some(&mut (node, ref mut next)) = stack.last_mut() {
            let empty = Vec::new();
            let children = adj.get(node).unwrap_or(&empty);
            match children.get(*next) {
                Some(&to) => {
                    *next += 1;
                    match state.get(to) {
                        Some(1) => {
                            back.insert((node.to_string(), to.to_string()));
                        }
                        Some(0) => {
                            state.insert(to, 1);
                            stack.push((to, 0));
                        }
                        _ => {}
                    }
                }
                None => {
                    state.insert(node, 2);
                    stack.pop();
                }
            }
        }
    }
    back
}

/// 给每个节点定层：层号取从入口出发的最长路径
fn rank_nodes(
    ids: &[String],
    edges: &[GraphEdge],
    back: &HashSet<(String, String)>,
) -> HashMap<String, usize> {
    let mut succ: HashMap<&str, Vec<&str>> =
        ids.iter().map(|id| (id.as_str(), Vec::new())).collect();
    let mut indeg: HashMap<&str, usize> = ids.iter().map(|id| (id.as_str(), 0)).collect();
    for e in edges {
        if back.contains(&(e.from.clone(), e.to.clone())) {
            continue;
        }
        let (Some(s), Some(d)) = (succ.get_mut(e.from.as_str()), indeg.get_mut(e.to.as_str()))
        else {
            continue;
        };
        s.push(e.to.as_str());
        *d += 1;
    }

    let mut rank: HashMap<String, usize> = ids.iter().map(|id| (id.clone(), 0)).collect();
    let mut queue: Vec<&str> = ids
        .iter()
        .filter(|id| indeg[id.as_str()] == 0)
        .map(String::as_str)
        .collect();
    let mut head = 0;
    while head < queue.len() {
        let node = queue[head];
        head += 1;
        let here = rank[node];
        for &to in &succ[node] {
            let slot = rank.get_mut(to).expect("id 集合已覆盖全部端点");
            *slot = (*slot).max(here + 1);
            let deg = indeg.get_mut(to).expect("同上");
            *deg -= 1;
            if *deg == 0 {
                queue.push(to);
            }
        }
    }

    // 出口固定排在最底下，否则它会跟某个中间节点挤在同一层
    if let Some(bottom) = rank
        .iter()
        .filter(|(id, _)| id.as_str() != EXIT_ID)
        .map(|(_, rank)| *rank)
        .max()
        && rank.contains_key(EXIT_ID)
    {
        rank.insert(EXIT_ID.to_string(), bottom + 1);
    }
    rank
}

/// 计算一层子图的布局
pub fn layout(graph: &SubGraph) -> Result<GraphLayout, LayoutError> {
    validate_node_ids(graph)?;
    let mut ids: Vec<String> = vec![ENTRY_ID.to_string()];
    ids.extend(graph.nodes.iter().map(|n| n.id.clone()));
    ids.push(EXIT_ID.to_string());

    let known: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let (edge_indices, edges): (Vec<usize>, Vec<GraphEdge>) = graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| known.contains(e.from.as_str()) && known.contains(e.to.as_str()))
        .map(|(index, edge)| (index, edge.clone()))
        .unzip();

    let mut adj: HashMap<&str, Vec<&str>> =
        ids.iter().map(|id| (id.as_str(), Vec::new())).collect();
    for e in &edges {
        if let Some(list) = adj.get_mut(e.from.as_str()) {
            list.push(e.to.as_str());
        }
    }
    let back = back_edges(&ids, &adj);
    let rank = rank_nodes(&ids, &edges, &back);

    // 同层内按 ids 顺序排列，保持与文件里的书写顺序一致
    let mut rows: HashMap<usize, Vec<&String>> = HashMap::new();
    for id in &ids {
        rows.entry(rank[id]).or_default().push(id);
    }
    let widest = rows.values().map(Vec::len).max().unwrap_or(1);
    let content_w = widest as f32 * NODE_W + (widest.saturating_sub(1)) as f32 * GAP_X;

    let mut node_x: HashMap<&str, f32> = HashMap::new();
    for members in rows.values() {
        let row_w =
            members.len() as f32 * NODE_W + (members.len().saturating_sub(1)) as f32 * GAP_X;
        let start_x = PAD + (content_w - row_w) / 2.0;
        for (i, id) in members.iter().enumerate() {
            node_x.insert(id.as_str(), start_x + i as f32 * (NODE_W + GAP_X));
        }
    }

    let bottom_row = rows.keys().copied().max().unwrap_or(0);
    // 第 0 个间隙在第一层上方，第 r + 1 个间隙在第 r 层下方。
    let mut gap_ports: Vec<Vec<(f32, usize)>> = vec![Vec::new(); bottom_row + 2];
    let ports = edge_ports(&edges, &rank, &node_x, &mut gap_ports);
    let mut gaps = vec![GapTracks::default(); bottom_row + 2];
    let mut side_lanes: Vec<(usize, usize, f32)> = Vec::new();
    let mut right_edge = PAD + content_w;
    let plans: Vec<RoutePlan> = edges
        .iter()
        .zip(&ports)
        .map(|(edge, &(from_x, to_x))| {
            let from_row = rank[&edge.from];
            let to_row = rank[&edge.to];
            let is_back = back.contains(&(edge.from.clone(), edge.to.clone()));
            match !is_back && to_row == from_row + 1 {
                true => RoutePlan::Forward {
                    track: (from_x != to_x).then(|| gaps[to_row].reserve(from_x, to_x)),
                },
                false => {
                    let lo = from_row.min(to_row);
                    let hi = from_row.max(to_row) + 1;
                    let mut x = side_lane(&rows, from_row, to_row, content_w);
                    // 自环会伸入上下两个间隙，邻层通道也可能重叠，按间隙范围判定。
                    // 避开端口的 x，防止侧通道与邻层较宽节点的短竖线共线。
                    while side_lanes.iter().any(|&(other_lo, other_hi, other_x)| {
                        lo <= other_hi && hi >= other_lo && (x - other_x).abs() < LANE_GAP
                    }) || gap_ports[lo..=hi]
                        .iter()
                        .flatten()
                        .any(|&(port_x, _)| x == port_x)
                    {
                        x += LANE_GAP;
                    }
                    side_lanes.push((lo, hi, x));
                    right_edge = right_edge.max(x);
                    RoutePlan::Side {
                        x,
                        leave_track: gaps[from_row + 1].reserve(from_x, x),
                        enter_track: gaps[to_row].reserve(to_x, x),
                    }
                }
            }
        })
        .collect();

    let gap_heights: Vec<f32> = gaps
        .iter()
        .enumerate()
        .map(|(index, gap)| {
            let minimum = match index == 0 || index == bottom_row + 1 {
                true => PAD,
                false => GAP_Y,
            };
            minimum.max((gap.tracks.len() + 1) as f32 * LANE_GAP)
        })
        .collect();
    let mut gap_starts = vec![0.0; gaps.len()];
    let mut row_y = vec![0.0; bottom_row + 1];
    for row in 0..=bottom_row {
        row_y[row] = gap_starts[row] + gap_heights[row];
        gap_starts[row + 1] = row_y[row] + NODE_H;
    }
    let track_y = |gap: usize, track: usize| {
        gap_starts[gap]
            + gap_heights[gap] * (track + 1) as f32 / (gaps[gap].tracks.len() + 1) as f32
    };

    let nodes: Vec<PlacedNode> = ids
        .iter()
        .map(|id| PlacedNode {
            id: id.clone(),
            pos: Vec2::new(node_x[id.as_str()], row_y[rank[id]]),
            slot: match id.as_str() {
                ENTRY_ID => NodeSlot::Entry,
                EXIT_ID => NodeSlot::Exit,
                _ => NodeSlot::Real,
            },
        })
        .collect();

    let placed_edges = edges
        .iter()
        .zip(&ports)
        .zip(&plans)
        .zip(edge_indices)
        .map(|(((e, &(from_x, to_x)), plan), edge_index)| {
            let from_row = rank[&e.from];
            let to_row = rank[&e.to];
            let start = Vec2::new(from_x, row_y[from_row] + NODE_H);
            let end = Vec2::new(to_x, row_y[to_row]);
            let is_back = back.contains(&(e.from.clone(), e.to.clone()));
            let points = match *plan {
                RoutePlan::Forward { track: None } => vec![start, end],
                RoutePlan::Forward { track: Some(track) } => {
                    let y = track_y(to_row, track);
                    vec![start, Vec2::new(start.x, y), Vec2::new(end.x, y), end]
                }
                RoutePlan::Side {
                    x,
                    leave_track,
                    enter_track,
                } => {
                    let leave_y = track_y(from_row + 1, leave_track);
                    let enter_y = track_y(to_row, enter_track);
                    vec![
                        start,
                        Vec2::new(start.x, leave_y),
                        Vec2::new(x, leave_y),
                        Vec2::new(x, enter_y),
                        Vec2::new(end.x, enter_y),
                        end,
                    ]
                }
            };
            PlacedEdge {
                edge_index,
                points,
                back: is_back,
            }
        })
        .collect();

    Ok(GraphLayout {
        nodes,
        edges: placed_edges,
        size: Vec2::new(
            right_edge + PAD,
            gap_starts[bottom_row + 1] + gap_heights[bottom_row + 1],
        ),
    })
}

#[derive(Clone, Copy)]
enum RoutePlan {
    Forward {
        track: Option<usize>,
    },
    Side {
        x: f32,
        leave_track: usize,
        enter_track: usize,
    },
}

/// 不相交的水平区间可以复用高度，有正长度重叠的区间必须占不同轨道。
#[derive(Clone, Default)]
struct GapTracks {
    tracks: Vec<Vec<(f32, f32)>>,
}

impl GapTracks {
    fn reserve(&mut self, a: f32, b: f32) -> usize {
        let (lo, hi) = (a.min(b), a.max(b));
        for (index, track) in self.tracks.iter_mut().enumerate() {
            if track
                .iter()
                .all(|&(other_lo, other_hi)| lo >= other_hi || hi <= other_lo)
            {
                track.push((lo, hi));
                return index;
            }
        }
        self.tracks.push(vec![(lo, hi)]);
        self.tracks.len() - 1
    }
}

/// 同一节点的多条边使用独立端口，间隙内不同边的短竖线也不得互相覆盖。
fn edge_ports<'a>(
    edges: &'a [GraphEdge],
    rank: &HashMap<String, usize>,
    node_x: &HashMap<&str, f32>,
    gaps: &mut [Vec<(f32, usize)>],
) -> Vec<(f32, f32)> {
    let mut degree: HashMap<(&str, bool), usize> = HashMap::new();
    for edge in edges {
        *degree.entry((&edge.from, false)).or_default() += 1;
        *degree.entry((&edge.to, true)).or_default() += 1;
    }
    let mut used: HashMap<(&str, bool), usize> = HashMap::new();
    edges
        .iter()
        .enumerate()
        .map(|(edge_index, edge)| {
            let mut port = |id: &'a str, incoming: bool, gap: usize| {
                let count = degree[&(id, incoming)];
                let index = used.entry((id, incoming)).or_default();
                let spacing = LANE_GAP.min((NODE_W - 32.0) / count as f32);
                let preferred = node_x[id]
                    + NODE_W / 2.0
                    + (*index as f32 - (count - 1) as f32 / 2.0) * spacing;
                *index += 1;
                reserve_port(&mut gaps[gap], edge_index, preferred, node_x[id])
            };
            (
                port(&edge.from, false, rank[&edge.from] + 1),
                port(&edge.to, true, rank[&edge.to]),
            )
        })
        .collect()
}

fn reserve_port(
    ports: &mut Vec<(f32, usize)>,
    edge_index: usize,
    preferred: f32,
    node_x: f32,
) -> f32 {
    let occupied = |candidate| {
        ports
            .iter()
            .any(|&(x, other_edge)| x == candidate && other_edge != edge_index)
    };
    let x = match occupied(preferred) {
        false => preferred,
        true => {
            let lo = node_x + 8.0;
            let hi = node_x + NODE_W - 8.0;
            let spacing = (LANE_GAP / 4.0).min((hi - lo) / (ports.len() + 2) as f32);
            // 由近到远找空位；候选数多于已有端口数，且始终留在节点内部。
            (1..=ports.len() + 2)
                .flat_map(|offset| {
                    [
                        preferred + offset as f32 * spacing,
                        preferred - offset as f32 * spacing,
                    ]
                })
                .find(|&candidate| candidate >= lo && candidate <= hi && !occupied(candidate))
                .expect("节点内部的候选端口数多于已占用端口数")
        }
    };
    ports.push((x, edge_index));
    x
}

/// 避障通道的 x 坐标
///
/// 贴着这条边纵向跨过的那几层里最宽的一层，而不是全图最宽层——
/// 否则一条只穿过单列区域的回边会被甩到很远的右边，看着像断掉的线。
fn side_lane(
    rows: &HashMap<usize, Vec<&String>>,
    from_row: usize,
    to_row: usize,
    content_w: f32,
) -> f32 {
    let (lo, hi) = match from_row <= to_row {
        true => (from_row, to_row),
        false => (to_row, from_row),
    };
    let widest = (lo..=hi)
        .filter_map(|r| rows.get(&r))
        .map(Vec::len)
        .max()
        .unwrap_or(1);
    let span_w = widest as f32 * NODE_W + (widest.saturating_sub(1)) as f32 * GAP_X;
    // 每层都对齐到内容中线，所以这段区域的右边界在中线右侧 span_w/2 处
    PAD + (content_w + span_w) / 2.0 + SIDE_BULGE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TaskNode;

    fn node(id: &str) -> TaskNode {
        TaskNode {
            id: id.into(),
            node_type: "log".into(),
            inputs: serde_json::Value::Null,
            checkpoint: false,
            children: None,
        }
    }

    fn edge(from: &str, to: &str) -> GraphEdge {
        GraphEdge {
            from: from.into(),
            to: to.into(),
        }
    }

    fn chain(n: usize) -> SubGraph {
        let ids: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();
        let mut edges = vec![edge(ENTRY_ID, &ids[0])];
        for pair in ids.windows(2) {
            edges.push(edge(&pair[0], &pair[1]));
        }
        edges.push(edge(ids.last().unwrap(), EXIT_ID));
        SubGraph {
            nodes: ids.iter().map(|id| node(id)).collect(),
            edges,
            ..Default::default()
        }
    }

    fn y_of(layout: &GraphLayout, id: &str) -> f32 {
        layout.nodes.iter().find(|n| n.id == id).unwrap().pos.y
    }

    #[test]
    fn 链式图每层一个节点且自上而下() {
        let out = layout(&chain(4)).unwrap();
        assert_eq!(out.nodes.len(), 6, "含虚拟入口出口");
        let ys: Vec<f32> = ["_entry", "n0", "n1", "n2", "n3", "_exit"]
            .iter()
            .map(|id| y_of(&out, id))
            .collect();
        assert!(ys.windows(2).all(|w| w[0] < w[1]), "层级应严格递增: {ys:?}");
        assert!(
            ys.windows(2).all(|w| w[1] - w[0] == NODE_H + GAP_Y),
            "出口不应产生额外空层：{ys:?}"
        );
        assert!(
            out.edges.iter().all(|edge| edge.points.len() == 2),
            "普通单链保留竖直走线"
        );
    }

    #[test]
    fn 重复节点不能进入拓扑排序() {
        let graph = SubGraph {
            nodes: vec![node("a"), node("a"), node("b")],
            edges: vec![edge("a", "b")],
            ..Default::default()
        };
        assert_eq!(
            layout(&graph).unwrap_err(),
            LayoutError::Duplicate {
                first_index: 0,
                node_index: 1,
                id: "a".into(),
            }
        );
    }

    #[test]
    fn 保留和空白节点身份必须拒绝() {
        for id in [ENTRY_ID, EXIT_ID] {
            let graph = SubGraph {
                nodes: vec![node(id)],
                ..Default::default()
            };
            assert_eq!(
                layout(&graph).unwrap_err(),
                LayoutError::Reserved {
                    node_index: 0,
                    id: id.into(),
                }
            );
        }
        for id in ["", " ", "\t\n", "　"] {
            let graph = SubGraph {
                nodes: vec![node(id)],
                ..Default::default()
            };
            assert_eq!(
                layout(&graph).unwrap_err(),
                LayoutError::Empty { node_index: 0 }
            );
        }
    }

    #[test]
    fn 并行分支排在同一层() {
        let graph = SubGraph {
            nodes: vec![node("a"), node("b"), node("join")],
            edges: vec![
                edge(ENTRY_ID, "a"),
                edge(ENTRY_ID, "b"),
                edge("a", "join"),
                edge("b", "join"),
                edge("join", EXIT_ID),
            ],
            ..Default::default()
        };
        let out = layout(&graph).unwrap();
        assert_eq!(y_of(&out, "a"), y_of(&out, "b"), "两个分支应同层");
        assert!(y_of(&out, "join") > y_of(&out, "a"), "汇合点在分支之下");
    }

    #[test]
    fn 汇合点取最长路径而非最短() {
        // entry → a → b → join，同时 entry → join；join 必须排在 b 之下
        let graph = SubGraph {
            nodes: vec![node("a"), node("b"), node("join")],
            edges: vec![
                edge(ENTRY_ID, "a"),
                edge("a", "b"),
                edge("b", "join"),
                edge(ENTRY_ID, "join"),
                edge("join", EXIT_ID),
            ],
            ..Default::default()
        };
        let out = layout(&graph).unwrap();
        assert!(y_of(&out, "join") > y_of(&out, "b"));
    }

    #[test]
    fn 有环时仍能分层且回边被标出() {
        // loop 体：a → b → a
        let graph = SubGraph {
            nodes: vec![node("a"), node("b")],
            edges: vec![
                edge(ENTRY_ID, "a"),
                edge("a", "b"),
                edge("b", "a"),
                edge("b", EXIT_ID),
            ],
            ..Default::default()
        };
        let out = layout(&graph).unwrap();
        assert!(y_of(&out, "b") > y_of(&out, "a"), "断掉回边后仍分得出层级");
        let backs: Vec<&PlacedEdge> = out.edges.iter().filter(|e| e.back).collect();
        assert_eq!(backs.len(), 1, "恰好一条回边");
    }

    #[test]
    fn 回边走右侧通道不穿过主干() {
        let graph = SubGraph {
            nodes: vec![node("a"), node("b")],
            edges: vec![
                edge(ENTRY_ID, "a"),
                edge("a", "b"),
                edge("b", "a"),
                edge("b", EXIT_ID),
            ],
            ..Default::default()
        };
        let out = layout(&graph).unwrap();
        let back = out.edges.iter().find(|e| e.back).unwrap();
        let rightmost = out
            .nodes
            .iter()
            .map(|n| n.pos.x + NODE_W)
            .fold(f32::MIN, f32::max);
        assert!(
            back.points.iter().any(|p| p.x > rightmost),
            "回边应绕到所有节点右侧"
        );
    }

    #[test]
    fn 折线相邻点保持正交() {
        let out = layout(&chain(3)).unwrap();
        for e in &out.edges {
            for pair in e.points.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                assert!(
                    (a.x - b.x).abs() < 0.01 || (a.y - b.y).abs() < 0.01,
                    "线段既不水平也不垂直: {a:?} → {b:?}"
                );
            }
        }
    }

    #[test]
    fn 出口始终在最底层() {
        // 有一个节点不连到出口，出口仍应排在最下面
        let graph = SubGraph {
            nodes: vec![node("a"), node("dangling")],
            edges: vec![edge(ENTRY_ID, "a"), edge("a", EXIT_ID)],
            ..Default::default()
        };
        let out = layout(&graph).unwrap();
        let exit_y = y_of(&out, "_exit");
        assert!(
            out.nodes
                .iter()
                .filter(|n| n.slot != NodeSlot::Exit)
                .all(|n| n.pos.y < exit_y)
        );
    }

    #[test]
    fn 空图也能布局() {
        let out = layout(&SubGraph::default()).unwrap();
        assert_eq!(out.nodes.len(), 2, "只剩入口与出口");
        assert!(out.size.x > 0.0 && out.size.y > 0.0);
    }

    #[test]
    fn 指向未知节点的边被忽略() {
        let graph = SubGraph {
            nodes: vec![node("a")],
            edges: vec![
                edge("a", "nowhere"),
                edge(ENTRY_ID, "a"),
                edge("a", "nowhere"),
                edge("a", EXIT_ID),
            ],
            ..Default::default()
        };
        let out = layout(&graph).unwrap();
        assert_eq!(out.edges.len(), 2, "悬空的边不参与布局");
        assert_eq!(
            out.edges
                .iter()
                .map(|edge| edge.edge_index)
                .collect::<Vec<_>>(),
            [1, 3],
            "过滤不改变原始边身份"
        );
    }

    #[test]
    fn 回边通道贴紧它跨过的层而非全图最宽层() {
        // wide1..wide3 在同一层撑宽整张图，回边 c→b 只穿过下面的单列区域
        let graph = SubGraph {
            nodes: vec![
                node("wide1"),
                node("wide2"),
                node("wide3"),
                node("a"),
                node("b"),
                node("c"),
            ],
            edges: vec![
                edge(ENTRY_ID, "wide1"),
                edge(ENTRY_ID, "wide2"),
                edge(ENTRY_ID, "wide3"),
                edge("wide1", "a"),
                edge("wide2", "a"),
                edge("wide3", "a"),
                edge("a", "b"),
                edge("b", "c"),
                edge("c", "b"),
                edge("c", EXIT_ID),
            ],
            ..Default::default()
        };
        let out = layout(&graph).unwrap();
        let back: Vec<&PlacedEdge> = out.edges.iter().filter(|e| e.back).collect();
        assert_eq!(back.len(), 1, "应当只有 c→b 一条回边");

        let lane = back[0].points.iter().map(|p| p.x).fold(0.0, f32::max);
        let widest_row_w = 3.0 * NODE_W + 2.0 * GAP_X;
        let full_lane = PAD + (widest_row_w + widest_row_w) / 2.0 + SIDE_BULGE;
        assert!(
            lane < full_lane,
            "回边只穿过单列区域，通道不该贴到全图最宽层：lane={lane} full={full_lane}"
        );

        // 但仍要绕开它自己那一列的节点
        let col_right = out
            .nodes
            .iter()
            .find(|n| n.id == "b")
            .map(|n| n.pos.x + NODE_W)
            .unwrap();
        assert!(lane > col_right, "回边通道压到主干上了");
    }

    #[test]
    fn 画布尺寸能容纳所有节点与回边() {
        let graph = SubGraph {
            nodes: vec![node("a"), node("b")],
            edges: vec![
                edge(ENTRY_ID, "a"),
                edge("a", "b"),
                edge("b", "a"),
                edge("b", EXIT_ID),
            ],
            ..Default::default()
        };
        let out = layout(&graph).unwrap();
        for n in &out.nodes {
            assert!(n.pos.x + NODE_W <= out.size.x, "节点超出画布宽度");
            assert!(n.pos.y + NODE_H <= out.size.y, "节点超出画布高度");
        }
        for e in &out.edges {
            for p in &e.points {
                assert!(p.x <= out.size.x, "折线超出画布: {p:?}");
            }
        }
    }

    /// 独立检查可见性约束：每段折线正交、留在画布内，且避开所有非端点卡片。
    fn assert_clear_routes(graph: &SubGraph) {
        let out = layout(graph).unwrap();
        assert_eq!(out.edges.len(), graph.edges.len());
        for placed in &out.edges {
            let source = &graph.edges[placed.edge_index];
            assert!(placed.points.len() >= 2);
            for pair in placed.points.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                assert!(a.x == b.x || a.y == b.y, "非正交线段：{pair:?}");
                for point in [a, b] {
                    assert!(point.x >= 0.0 && point.x <= out.size.x);
                    assert!(point.y >= 0.0 && point.y <= out.size.y);
                }
                for node in &out.nodes {
                    if node.id == source.from || node.id == source.to {
                        continue;
                    }
                    let separated = a.x.max(b.x) < node.pos.x
                        || a.x.min(b.x) > node.pos.x + NODE_W
                        || a.y.max(b.y) < node.pos.y
                        || a.y.min(b.y) > node.pos.y + NODE_H;
                    assert!(
                        separated,
                        "{} → {} 的线段 {pair:?} 遮挡了 {}",
                        source.from, source.to, node.id
                    );
                }
            }
        }
    }

    /// 交点允许存在；有正长度的共线重叠会掩盖路径身份，必须由通道分配消除。
    fn assert_distinct_routes(out: &GraphLayout) {
        for (index, first) in out.edges.iter().enumerate() {
            for second in &out.edges[index + 1..] {
                for a in first.points.windows(2) {
                    for b in second.points.windows(2) {
                        let horizontal = a[0].y == a[1].y
                            && b[0].y == b[1].y
                            && a[0].y == b[0].y
                            && a[0].x.min(a[1].x).max(b[0].x.min(b[1].x))
                                < a[0].x.max(a[1].x).min(b[0].x.max(b[1].x));
                        let vertical = a[0].x == a[1].x
                            && b[0].x == b[1].x
                            && a[0].x == b[0].x
                            && a[0].y.min(a[1].y).max(b[0].y.min(b[1].y))
                                < a[0].y.max(a[1].y).min(b[0].y.max(b[1].y));
                        assert!(
                            !horizontal && !vertical,
                            "边 {} 和 {} 共线重叠：{a:?} 与 {b:?}",
                            first.edge_index,
                            second.edge_index
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn 交叉分支使用不同的水平通道() {
        let graph = SubGraph {
            nodes: vec![node("a"), node("b"), node("c"), node("d")],
            edges: vec![
                edge(ENTRY_ID, "a"),
                edge(ENTRY_ID, "b"),
                edge("a", "d"),
                edge("b", "c"),
                edge("c", EXIT_ID),
                edge("d", EXIT_ID),
            ],
            ..Default::default()
        };
        assert_clear_routes(&graph);
        assert_distinct_routes(&layout(&graph).unwrap());
    }

    #[test]
    fn 同端点平行边自环与回边各自保留身份和路径() {
        let mut graph = chain(2);
        graph.edges.extend([
            edge("n0", "n1"),
            edge("n0", "n1"),
            edge("n1", "n0"),
            edge("n1", "n0"),
            edge("n1", "n1"),
            edge("n1", "n1"),
        ]);
        assert_clear_routes(&graph);
        let out = layout(&graph).unwrap();
        assert_eq!(
            out.edges
                .iter()
                .map(|edge| edge.edge_index)
                .collect::<Vec<_>>(),
            (0..graph.edges.len()).collect::<Vec<_>>()
        );
        assert_distinct_routes(&out);
    }

    #[test]
    fn 密集层间通道自动增加间距而不侵入卡片() {
        let mut graph = chain(2);
        graph.edges.extend((0..12).map(|_| edge("n1", "n0")));
        assert_clear_routes(&graph);
        let out = layout(&graph).unwrap();
        assert!(y_of(&out, "n0") - y_of(&out, ENTRY_ID) > NODE_H + GAP_Y);
        assert_distinct_routes(&out);
    }

    #[test]
    fn 跨层直达边绕过中间卡片且保留独立通道() {
        let mut graph = chain(4);
        graph.edges.push(edge(ENTRY_ID, "n3"));
        graph.edges.push(edge("n0", "n3"));
        assert_clear_routes(&graph);
        let out = layout(&graph).unwrap();
        let lanes: Vec<f32> = out
            .edges
            .iter()
            .rev()
            .take(2)
            .map(|edge| edge.points.iter().map(|point| point.x).fold(0.0, f32::max))
            .collect();
        assert!((lanes[0] - lanes[1]).abs() >= LANE_GAP);
    }

    #[test]
    fn 回边避开端点同行的其他卡片() {
        let graph = SubGraph {
            nodes: vec![node("a"), node("b"), node("c"), node("d")],
            edges: vec![
                edge(ENTRY_ID, "a"),
                edge(ENTRY_ID, "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "a"),
                edge("d", EXIT_ID),
            ],
            ..Default::default()
        };
        assert_clear_routes(&graph);
    }

    #[test]
    fn 自环有可见高度且完整落在画布内() {
        let graph = SubGraph {
            nodes: vec![node("a")],
            edges: vec![edge(ENTRY_ID, "a"), edge("a", "a"), edge("a", EXIT_ID)],
            ..Default::default()
        };
        assert_clear_routes(&graph);
        let out = layout(&graph).unwrap();
        let loop_edge = &out.edges[1];
        assert!(loop_edge.back);
        assert!(loop_edge.points.iter().any(|p| p.y < y_of(&out, "a")));
        assert!(
            loop_edge
                .points
                .iter()
                .any(|p| p.y > y_of(&out, "a") + NODE_H)
        );
    }

    #[test]
    fn 多种分支与闭环组合均不遮挡非端点节点() {
        // 固定种子的组合覆盖不同层宽、回边、自环、跨层汇合，不依赖随机源或时序。
        let mut seed = 17_u64;
        for _ in 0..256 {
            let mut graph = chain(6);
            for from in 0..6 {
                for to in 0..6 {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    if (seed >> 32).is_multiple_of(5) && to != from + 1 {
                        graph
                            .edges
                            .push(edge(&format!("n{from}"), &format!("n{to}")));
                    }
                }
            }
            assert_clear_routes(&graph);
            assert_distinct_routes(&layout(&graph).unwrap());
        }
    }

    #[test]
    fn 不同层宽孤立节点及虚拟端点闭环保持路径不变量() {
        let mut seed = 29_u64;
        for _ in 0..256 {
            let mut graph = SubGraph {
                nodes: (0..8).map(|i| node(&format!("n{i}"))).collect(),
                ..Default::default()
            };
            let mut ids = vec![ENTRY_ID.to_string()];
            ids.extend(graph.nodes.iter().map(|node| node.id.clone()));
            ids.push(EXIT_ID.to_string());
            for from in &ids {
                for to in &ids {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    if (seed >> 32).is_multiple_of(9) {
                        graph.edges.push(edge(from, to));
                    }
                }
            }
            assert_clear_routes(&graph);
            assert_distinct_routes(&layout(&graph).unwrap());
        }
    }
}
