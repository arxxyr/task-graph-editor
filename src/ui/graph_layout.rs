//! 任务图的分层布局
//!
//! 输入一层子图，输出每个节点的位置和每条边的折线点。纯计算，不碰 UI，
//! 便于单测。渲染在 [`super::graph_view`]。
//!
//! 算法是分层图布局的简化版：按最长路径给节点定层，同层水平居中排开，
//! 边走正交折线。`loop` 节点的循环体带回边，直接拓扑排序会因为有环排不出层级，
//! 所以先用 DFS 把回边挑出来、定层时当它不存在，渲染时再单独画成虚线——
//! 层次排得出来，循环关系也一眼看得见。

use bevy::math::Vec2;
use std::collections::{HashMap, HashSet};

use crate::model::{ENTRY_ID, EXIT_ID, GraphEdge, SubGraph};

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

/// 回边向右绕出的基础距离
const BACK_BULGE: f32 = 36.0;

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
    if let Some(bottom) = rank.values().copied().max()
        && rank.contains_key(EXIT_ID)
    {
        rank.insert(EXIT_ID.to_string(), bottom + 1);
    }
    rank
}

/// 计算一层子图的布局
pub fn layout(graph: &SubGraph) -> GraphLayout {
    let mut ids: Vec<String> = vec![ENTRY_ID.to_string()];
    ids.extend(graph.nodes.iter().map(|n| n.id.clone()));
    ids.push(EXIT_ID.to_string());

    let known: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let edges: Vec<GraphEdge> = graph
        .edges
        .iter()
        .filter(|e| known.contains(e.from.as_str()) && known.contains(e.to.as_str()))
        .cloned()
        .collect();

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

    let mut pos: HashMap<&str, Vec2> = HashMap::new();
    for (&row, members) in &rows {
        let row_w =
            members.len() as f32 * NODE_W + (members.len().saturating_sub(1)) as f32 * GAP_X;
        let start_x = PAD + (content_w - row_w) / 2.0;
        for (i, id) in members.iter().enumerate() {
            pos.insert(
                id.as_str(),
                Vec2::new(
                    start_x + i as f32 * (NODE_W + GAP_X),
                    PAD + row as f32 * (NODE_H + GAP_Y),
                ),
            );
        }
    }

    let bottom_row = rows.keys().copied().max().unwrap_or(0);
    let nodes = ids
        .iter()
        .map(|id| PlacedNode {
            id: id.clone(),
            pos: pos[id.as_str()],
            slot: match id.as_str() {
                ENTRY_ID => NodeSlot::Entry,
                EXIT_ID => NodeSlot::Exit,
                _ => NodeSlot::Real,
            },
        })
        .collect();

    let mut right_edge = PAD + content_w;
    let placed_edges = edges
        .iter()
        .map(|e| {
            let a = pos[e.from.as_str()];
            let b = pos[e.to.as_str()];
            let is_back = back.contains(&(e.from.clone(), e.to.clone()));
            let points = match is_back {
                true => {
                    let lane = back_lane(&rows, rank[&e.from], rank[&e.to], content_w);
                    right_edge = right_edge.max(lane);
                    back_route(a, b, lane)
                }
                false => forward_route(a, b),
            };
            PlacedEdge {
                points,
                back: is_back,
            }
        })
        .collect();

    GraphLayout {
        nodes,
        edges: placed_edges,
        size: Vec2::new(
            right_edge + PAD,
            (bottom_row + 1) as f32 * (NODE_H + GAP_Y) - GAP_Y + PAD * 2.0,
        ),
    }
}

/// 回边通道的 x 坐标
///
/// 贴着这条边纵向跨过的那几层里最宽的一层，而不是全图最宽层——
/// 否则一条只穿过单列区域的回边会被甩到很远的右边，看着像断掉的线。
fn back_lane(
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
    PAD + (content_w + span_w) / 2.0 + BACK_BULGE
}

/// 顺行边：从上一个节点底边到下一个节点顶边，必要时中途横移
fn forward_route(a: Vec2, b: Vec2) -> Vec<Vec2> {
    let x1 = a.x + NODE_W / 2.0;
    let x2 = b.x + NODE_W / 2.0;
    let y1 = a.y + NODE_H;
    let y2 = b.y;
    if (x1 - x2).abs() < f32::EPSILON {
        return vec![Vec2::new(x1, y1), Vec2::new(x2, y2)];
    }
    let mid = (y1 + y2) / 2.0;
    vec![
        Vec2::new(x1, y1),
        Vec2::new(x1, mid),
        Vec2::new(x2, mid),
        Vec2::new(x2, y2),
    ]
}

/// 回边：从右侧绕出去再折回来，避免和主干重叠
///
/// `lane` 是通道的 x 坐标，由 [`back_lane`] 按这条边跨过的层算好。
fn back_route(a: Vec2, b: Vec2, lane: f32) -> Vec<Vec2> {
    let y1 = a.y + NODE_H / 2.0;
    let y2 = b.y + NODE_H / 2.0;
    vec![
        Vec2::new(a.x + NODE_W, y1),
        Vec2::new(lane, y1),
        Vec2::new(lane, y2),
        Vec2::new(b.x + NODE_W, y2),
    ]
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
        }
    }

    fn y_of(layout: &GraphLayout, id: &str) -> f32 {
        layout.nodes.iter().find(|n| n.id == id).unwrap().pos.y
    }

    #[test]
    fn 链式图每层一个节点且自上而下() {
        let out = layout(&chain(4));
        assert_eq!(out.nodes.len(), 6, "含虚拟入口出口");
        let ys: Vec<f32> = ["_entry", "n0", "n1", "n2", "n3", "_exit"]
            .iter()
            .map(|id| y_of(&out, id))
            .collect();
        assert!(ys.windows(2).all(|w| w[0] < w[1]), "层级应严格递增: {ys:?}");
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
        };
        let out = layout(&graph);
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
        };
        let out = layout(&graph);
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
        };
        let out = layout(&graph);
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
        };
        let out = layout(&graph);
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
        let out = layout(&chain(3));
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
        };
        let out = layout(&graph);
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
        let out = layout(&SubGraph::default());
        assert_eq!(out.nodes.len(), 2, "只剩入口与出口");
        assert!(out.size.x > 0.0 && out.size.y > 0.0);
    }

    #[test]
    fn 指向未知节点的边被忽略() {
        let graph = SubGraph {
            nodes: vec![node("a")],
            edges: vec![edge(ENTRY_ID, "a"), edge("a", "nowhere")],
        };
        let out = layout(&graph);
        assert_eq!(out.edges.len(), 1, "悬空的边不参与布局");
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
        };
        let out = layout(&graph);
        let back: Vec<&PlacedEdge> = out.edges.iter().filter(|e| e.back).collect();
        assert_eq!(back.len(), 1, "应当只有 c→b 一条回边");

        let lane = back[0].points[1].x;
        let widest_row_w = 3.0 * NODE_W + 2.0 * GAP_X;
        let full_lane = PAD + (widest_row_w + widest_row_w) / 2.0 + BACK_BULGE;
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
        };
        let out = layout(&graph);
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
}
