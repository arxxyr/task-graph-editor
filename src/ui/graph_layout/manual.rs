//! 手工位置的正交可见图走线。节点矩形作为障碍，端口之外不得穿入卡片。

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use super::{GraphLayout, LayoutError, NODE_H, NODE_W, PAD, SubGraph, Vec2};

const CLEARANCE: f32 = 8.0;

#[derive(Clone, Copy)]
struct Obstacle {
    min: Vec2,
    max: Vec2,
}

impl Obstacle {
    fn contains(self, p: Vec2) -> bool {
        p.x > self.min.x && p.x < self.max.x && p.y > self.min.y && p.y < self.max.y
    }

    fn blocks(self, a: Vec2, b: Vec2) -> bool {
        match a.x == b.x {
            true => {
                a.x > self.min.x
                    && a.x < self.max.x
                    && a.y.min(b.y) < self.max.y
                    && a.y.max(b.y) > self.min.y
            }
            false => {
                a.y > self.min.y
                    && a.y < self.max.y
                    && a.x.min(b.x) < self.max.x
                    && a.x.max(b.x) > self.min.x
            }
        }
    }
}

/// 没有手工位置时完全复用自动布局；有位置时重新计算所有边，绝不留下旧连线。
pub fn layout_with_positions(
    graph: &SubGraph,
    positions: &HashMap<String, Vec2>,
) -> Result<GraphLayout, LayoutError> {
    let mut placed = super::layout(graph)?;
    let mut changed = false;
    for node in &mut placed.nodes {
        if let Some(&position) = positions.get(&node.id)
            && position.is_finite()
            && position != node.pos
        {
            node.pos = position.max(Vec2::splat(PAD));
            changed = true;
        }
    }
    if !changed {
        return Ok(placed);
    }
    let lookup: HashMap<&str, Vec2> = placed
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.pos))
        .collect();
    let obstacles: Vec<Obstacle> = placed
        .nodes
        .iter()
        .map(|node| Obstacle {
            min: node.pos - Vec2::splat(CLEARANCE),
            max: node.pos + Vec2::new(NODE_W, NODE_H) + Vec2::splat(CLEARANCE),
        })
        .collect();
    let mut degree = HashMap::<(&str, bool), usize>::new();
    for edge in &graph.edges {
        *degree.entry((&edge.from, false)).or_default() += 1;
        *degree.entry((&edge.to, true)).or_default() += 1;
    }
    let mut used = HashMap::<(String, bool), usize>::new();
    let mut segments = Vec::new();
    for edge in &mut placed.edges {
        let source = &graph.edges[edge.edge_index];
        let mut port = |id: &str, incoming: bool| {
            let position = lookup[id];
            let count = degree[&(id, incoming)];
            let next = used.entry((id.to_owned(), incoming)).or_default();
            let spacing = 12.0_f32.min((NODE_W - 32.0) / count as f32);
            let x = position.x + NODE_W / 2.0 + (*next as f32 - (count - 1) as f32 / 2.0) * spacing;
            *next += 1;
            Vec2::new(x, position.y + if incoming { 0.0 } else { NODE_H })
        };
        let start = port(&source.from, false);
        let end = port(&source.to, true);
        let leave = start + Vec2::Y * CLEARANCE;
        let enter = end - Vec2::Y * CLEARANCE;
        let Some(middle) = route(leave, enter, &obstacles, &segments, edge.edge_index) else {
            return Err(LayoutError::Unroutable {
                edge_index: edge.edge_index,
            });
        };
        let mut points = vec![start];
        points.extend(middle);
        points.push(end);
        edge.points = simplify(points);
        segments.extend(edge.points.windows(2).map(|pair| (pair[0], pair[1])));
    }
    placed.size = placed
        .nodes
        .iter()
        .map(|node| node.pos + Vec2::new(NODE_W, NODE_H))
        .chain(
            placed
                .edges
                .iter()
                .flat_map(|edge| edge.points.iter().copied()),
        )
        .fold(Vec2::ZERO, Vec2::max)
        + Vec2::splat(PAD);
    Ok(placed)
}

#[derive(Clone, Copy)]
struct Visit {
    cost: f32,
    index: usize,
}

impl PartialEq for Visit {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost && self.index == other.index
    }
}
impl Eq for Visit {}
impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Visit {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.index.cmp(&self.index))
    }
}

/// 可见网格仅在障碍边界、端口与本条边的外侧轨道取坐标；Dijkstra 优先短路径和少转弯。
fn route(
    start: Vec2,
    end: Vec2,
    obstacles: &[Obstacle],
    occupied: &[(Vec2, Vec2)],
    edge_index: usize,
) -> Option<Vec<Vec2>> {
    // 链与相邻节点通常只需直线或两个转角，先验证有限候选，避免为每条短边建立整个网格。
    let middle_y = (start.y + end.y) / 2.0;
    let candidates = [
        vec![start, Vec2::new(start.x, end.y), end],
        vec![start, Vec2::new(end.x, start.y), end],
        vec![
            start,
            Vec2::new(start.x, middle_y),
            Vec2::new(end.x, middle_y),
            end,
        ],
    ];
    if let Some(candidate) = candidates.into_iter().map(simplify).find(|points| {
        points.windows(2).all(|pair| {
            !obstacles
                .iter()
                .any(|obstacle| obstacle.blocks(pair[0], pair[1]))
                && !occupied
                    .iter()
                    .any(|&(a, b)| collinear_overlap(pair[0], pair[1], a, b))
        })
    }) {
        return Some(candidate);
    }
    let lane = 2.0 + edge_index as f32 * 2.0;
    let mut xs = vec![start.x, end.x];
    let mut ys = vec![start.y, end.y];
    for obstacle in obstacles {
        xs.extend([
            obstacle.min.x,
            obstacle.max.x,
            (obstacle.min.x - lane).max(1.0),
            obstacle.max.x + lane,
        ]);
        ys.extend([
            obstacle.min.y,
            obstacle.max.y,
            (obstacle.min.y - lane).max(1.0),
            obstacle.max.y + lane,
        ]);
    }
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let width = xs.len();
    let points: Vec<Vec2> = ys
        .iter()
        .flat_map(|&y| xs.iter().map(move |&x| Vec2::new(x, y)))
        .collect();
    let valid: Vec<bool> = points
        .iter()
        .map(|&p| !obstacles.iter().any(|o| o.contains(p)))
        .collect();
    let source = points.iter().position(|&p| p == start)?;
    let target = points.iter().position(|&p| p == end)?;
    if !valid[source] || !valid[target] {
        return None;
    }
    // 每个格点保存横向到达和纵向到达两种状态，使转弯罚分具有明确意义。
    let mut distances = vec![f32::INFINITY; points.len() * 2];
    let mut previous = vec![None; distances.len()];
    let mut heap = BinaryHeap::new();
    for direction in 0..2 {
        distances[source * 2 + direction] = 0.0;
        heap.push(Visit {
            cost: 0.0,
            index: source * 2 + direction,
        });
    }
    let mut goal = None;
    while let Some(Visit { cost, index }) = heap.pop() {
        if cost > distances[index] {
            continue;
        }
        let point = index / 2;
        if point == target {
            goal = Some(index);
            break;
        }
        let (x, y) = (point % width, point / width);
        let adjacent = [
            (x.checked_sub(1).map(|x| y * width + x), 0),
            ((x + 1 < width).then_some(point + 1), 0),
            (y.checked_sub(1).map(|y| y * width + x), 1),
            ((y + 1 < ys.len()).then_some(point + width), 1),
        ];
        for (next, direction) in adjacent {
            let Some(next) = next else {
                continue;
            };
            let (a, b) = (points[point], points[next]);
            if !valid[next] || obstacles.iter().any(|o| o.blocks(a, b)) {
                continue;
            }
            let overlap = occupied.iter().any(|&(c, d)| collinear_overlap(a, b, c, d));
            let next_cost = cost
                + a.distance(b)
                + if direction == index % 2 { 0.0 } else { 20.0 }
                + if overlap {
                    20_000.0 + a.distance(b) * 20.0
                } else {
                    0.0
                };
            let state = next * 2 + direction;
            if next_cost < distances[state] {
                distances[state] = next_cost;
                previous[state] = Some(index);
                heap.push(Visit {
                    cost: next_cost,
                    index: state,
                });
            }
        }
    }
    let mut state = goal?;
    let mut path = vec![points[state / 2]];
    while let Some(parent) = previous[state] {
        state = parent;
        path.push(points[state / 2]);
    }
    path.reverse();
    Some(path)
}

fn collinear_overlap(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> bool {
    match (a.x == b.x, c.x == d.x) {
        (true, true) => a.x == c.x && a.y.min(b.y) < c.y.max(d.y) && c.y.min(d.y) < a.y.max(b.y),
        (false, false) => a.y == c.y && a.x.min(b.x) < c.x.max(d.x) && c.x.min(d.x) < a.x.max(b.x),
        _ => false,
    }
}

fn simplify(points: Vec<Vec2>) -> Vec<Vec2> {
    let mut result: Vec<Vec2> = Vec::new();
    for p in points {
        if result.last() == Some(&p) {
            continue;
        }
        while result.len() >= 2 {
            let (a, b) = (result[result.len() - 2], result[result.len() - 1]);
            if (a.x == b.x && b.x == p.x) || (a.y == b.y && b.y == p.y) {
                result.pop();
            } else {
                break;
            }
        }
        result.push(p);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse_task_graph;

    #[test]
    fn 手工移动后循环平行边仍正交且避开全部卡片() {
        let data = parse_task_graph(r#"{"map_id":"m","task_id":"t","config":{"context":{},"nodes":[{"id":"a","type":"log"},{"id":"b","type":"log"},{"id":"c","type":"log"}],"edges":[{"from":"_entry","to":"a"},{"from":"a","to":"b"},{"from":"a","to":"b"},{"from":"b","to":"c"},{"from":"c","to":"a"},{"from":"a","to":"a"},{"from":"b","to":"_exit"}]}}"#).unwrap();
        let positions = HashMap::from([
            ("a".into(), Vec2::new(600.0, 200.0)),
            ("b".into(), Vec2::new(50.0, 300.0)),
            ("c".into(), Vec2::new(350.0, 420.0)),
        ]);
        let placed = layout_with_positions(&data.graph, &positions).unwrap();
        assert_eq!(placed.edges.len(), data.graph.edges.len());
        for edge in &placed.edges {
            for pair in edge.points.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                assert!(a.x == b.x || a.y == b.y);
                for node in &placed.nodes {
                    let obstacle = Obstacle {
                        min: node.pos,
                        max: node.pos + Vec2::new(NODE_W, NODE_H),
                    };
                    assert!(
                        !obstacle.blocks(a, b),
                        "边 {} 穿过 {}",
                        edge.edge_index,
                        node.id
                    );
                }
                assert!(a.cmpge(Vec2::ZERO).all() && a.cmple(placed.size).all());
            }
        }
        assert_ne!(placed.edges[1].points, placed.edges[2].points);
    }

    #[test]
    fn 空手工位置与自动布局完全相同() {
        let graph = SubGraph::default();
        let automatic = super::super::layout(&graph).unwrap();
        let manual = layout_with_positions(&graph, &HashMap::new()).unwrap();
        assert_eq!(automatic.size, manual.size);
        assert_eq!(
            automatic.nodes.iter().map(|n| n.pos).collect::<Vec<_>>(),
            manual.nodes.iter().map(|n| n.pos).collect::<Vec<_>>()
        );
    }
}
