//! 任务图数据模型与 JSON 解析/序列化逻辑

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 三维位置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// 四元数姿态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Orientation {
    pub w: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// 单个部位的位姿（位置 + 姿态）
///
/// 新建位姿按字段声明顺序输出；已有字段未编辑时保留原始字符串，
/// 编辑时合并变动分量并保留扩展成员，避免无关数据发生变化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pose {
    pub position: Position,
    pub orientation: Orientation,
}

/// 机器人完整位姿：底盘 + 头部 + 腰部
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RobotPose {
    pub chassis_pose: Pose,
    pub head_pose: Pose,
    pub waist_pose: Pose,
}

impl Default for RobotPose {
    fn default() -> Self {
        let p = Pose {
            position: Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            orientation: Orientation {
                w: 1.0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
        };
        Self {
            chassis_pose: p.clone(),
            head_pose: p.clone(),
            waist_pose: p,
        }
    }
}

/// 轨迹点（关节轨迹的单个路径点）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryPoint {
    pub positions: Vec<f64>,
    pub time_from_start: f64,
}

/// context 字段值类型
#[derive(Debug, Clone, PartialEq)]
pub enum ContextValue {
    /// 位姿点（字符串化 JSON，含 chassis_pose/head_pose/waist_pose）
    Pose(RobotPose),
    /// 布尔值
    Bool(bool),
    /// 整数
    Integer(i64),
    /// 浮点数
    Float(f64),
    /// 一维数值数组
    ///
    /// 真实任务图里两种写法都有：偏移量之类是字符串化的 `"[0.01,0.17,0.34]"`，
    /// 也有直接写成 JSON 数组的。`stringified` 记住原样，序列化时照原样写回，
    /// 否则会悄悄改掉远程文件的数据格式。
    NumericArray {
        /// 元素
        values: Vec<f64>,
        /// 原文是否为字符串化形式
        stringified: bool,
    },
    /// 二维数值数组（如 `"[[0.30,0.34,0.45],...]"` 或原生 `[[...],...]`）
    NumericArray2D {
        /// 行
        rows: Vec<Vec<f64>>,
        /// 原文是否为字符串化形式
        stringified: bool,
    },
    /// 关节轨迹（原生 JSON 数组，每个元素含 positions 和 time_from_start）
    JointTrajectory(Vec<TrajectoryPoint>),
    /// 位姿数组（原生 JSON 数组，可以为空）
    PoseArray(Vec<RobotPose>),
    /// 嵌套分组（原生 JSON 对象，成员递归分类，如 station_profiles.station_1.*）
    NestedGroup(Vec<ContextField>),
    /// 普通字符串（无法解析为位姿或数组的字符串值）
    Text(String),
    /// JSON null
    Null,
    /// 无法归类的其他 JSON 值
    RawJson(serde_json::Value),
}

/// context 中的一个字段
#[derive(Debug, Clone, PartialEq)]
pub struct ContextField {
    /// context 中的 key 名
    pub key: String,
    /// 值
    pub value: ContextValue,
}

/// 任务图中的一个节点
///
/// 只读：本工具编辑的是 context 参数，流程本身由机器人端定义。
/// 序列化时 `config.nodes` / `config.edges` 原样从 `raw_json` 带回去。
#[derive(Debug, Clone)]
pub struct TaskNode {
    /// 节点 id，同层内唯一
    pub id: String,
    /// 节点类型，如 `sequence`、`ros2_action`、`condition`
    pub node_type: String,
    /// 输入参数，原样保留用于展示
    pub inputs: serde_json::Value,
    /// 是否为断点续跑的检查点
    pub checkpoint: bool,
    /// 复合节点的子图（`sequence`、`loop`、`parallel` 等会有）
    pub children: Option<SubGraph>,
}

/// 一层图：节点加它们之间的连接
#[derive(Debug, Clone, Default)]
pub struct SubGraph {
    /// 本层节点
    pub nodes: Vec<TaskNode>,
    /// 本层的边，端点可能是虚拟的 `_entry` / `_exit`
    pub edges: Vec<GraphEdge>,
}

/// 一条有向边
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    /// 起点节点 id
    pub from: String,
    /// 终点节点 id
    pub to: String,
}

/// 虚拟入口节点的 id
pub const ENTRY_ID: &str = "_entry";

/// 虚拟出口节点的 id
pub const EXIT_ID: &str = "_exit";

impl SubGraph {
    /// 按 id 找节点
    pub fn node(&self, id: &str) -> Option<&TaskNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// 递归统计节点总数
    pub fn total_nodes(&self) -> usize {
        self.nodes.len()
            + self
                .nodes
                .iter()
                .filter_map(|n| n.children.as_ref())
                .map(SubGraph::total_nodes)
                .sum::<usize>()
    }

    /// 递归求最大嵌套深度（本层为 1）
    pub fn depth(&self) -> usize {
        1 + self
            .nodes
            .iter()
            .filter_map(|n| n.children.as_ref())
            .map(SubGraph::depth)
            .max()
            .unwrap_or(0)
    }

    /// 沿 id 路径逐层下钻
    pub fn subgraph_at(&self, path: &[String]) -> Option<&SubGraph> {
        let Some((first, rest)) = path.split_first() else {
            return Some(self);
        };
        self.node(first)?.children.as_ref()?.subgraph_at(rest)
    }
}

/// GUI 编辑用的任务图数据
#[derive(Debug, Clone)]
pub struct TaskGraphData {
    pub map_id: String,
    pub task_id: String,
    /// 所有 context 字段（按 key 排序）
    pub context_fields: Vec<ContextField>,
    /// 任务流程图（只读展示）
    pub graph: SubGraph,
    /// 原始 JSON（用于合并回写时保留未编辑字段）
    pub raw_json: serde_json::Value,
}

/// 解析错误
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),
    #[error("缺少必要字段: {0}")]
    MissingField(String),
}

// ============================================================
// Context 值类型识别
// ============================================================

/// f64 控件只编辑能精确表示的整数；超出安全范围的整块结构保留原始 JSON。
fn numbers_are_lossless(value: &serde_json::Value) -> bool {
    const MAX_EXACT_INTEGER: u64 = 1_u64 << 53;
    match value {
        serde_json::Value::Number(number) => match (number.as_i64(), number.as_u64()) {
            (Some(integer), _) => integer.unsigned_abs() <= MAX_EXACT_INTEGER,
            (_, Some(integer)) => integer <= MAX_EXACT_INTEGER,
            _ => true,
        },
        serde_json::Value::Array(values) => values.iter().all(numbers_are_lossless),
        serde_json::Value::Object(fields) => fields.values().all(numbers_are_lossless),
        _ => true,
    }
}

/// 分类字符串类型的 context 值
fn classify_string_value(s: &str) -> ContextValue {
    // 大整数不能经过 f64 编辑；保留字符串原文供只读展示与无损保存。
    if serde_json::from_str::<serde_json::Value>(s).is_ok_and(|value| !numbers_are_lossless(&value))
    {
        return ContextValue::RawJson(serde_json::Value::String(s.to_owned()));
    }
    // 尝试解析为 RobotPose
    if let Ok(pose) = serde_json::from_str::<RobotPose>(s) {
        return ContextValue::Pose(pose);
    }
    // 尝试解析为二维数值数组（必须非空，避免空数组误判）
    if let Ok(arr2d) = serde_json::from_str::<Vec<Vec<f64>>>(s)
        && !arr2d.is_empty()
    {
        return ContextValue::NumericArray2D {
            rows: arr2d,
            stringified: true,
        };
    }
    // 尝试解析为一维数值数组
    if let Ok(arr) = serde_json::from_str::<Vec<f64>>(s) {
        return ContextValue::NumericArray {
            values: arr,
            stringified: true,
        };
    }
    ContextValue::Text(s.to_string())
}

/// 分类数组类型的 context 值
fn classify_array_value(
    key: &str,
    value: &serde_json::Value,
    arr: &[serde_json::Value],
) -> ContextValue {
    if arr.is_empty() {
        // 空数组：根据 key 名推断类型
        if key.contains("traj") {
            return ContextValue::JointTrajectory(Vec::new());
        }
        if key.contains("pose") {
            return ContextValue::PoseArray(Vec::new());
        }
        return ContextValue::RawJson(value.clone());
    }
    if !numbers_are_lossless(value) {
        return ContextValue::RawJson(value.clone());
    }
    // 尝试解析为轨迹点数组
    if let Ok(traj) = serde_json::from_value::<Vec<TrajectoryPoint>>(value.clone()) {
        return ContextValue::JointTrajectory(traj);
    }
    // 尝试解析为位姿数组
    if let Ok(poses) = serde_json::from_value::<Vec<RobotPose>>(value.clone()) {
        return ContextValue::PoseArray(poses);
    }
    // 原生 JSON 数值数组（不是字符串化的），如偏移配置 [[0.0,0.05,0.25],...]
    if let Ok(rows) = serde_json::from_value::<Vec<Vec<f64>>>(value.clone()) {
        return ContextValue::NumericArray2D {
            rows,
            stringified: false,
        };
    }
    if let Ok(values) = serde_json::from_value::<Vec<f64>>(value.clone()) {
        return ContextValue::NumericArray {
            values,
            stringified: false,
        };
    }
    ContextValue::RawJson(value.clone())
}

/// 分类对象类型的 context 值：递归分类每个成员，形成嵌套分组
///
/// 适配 station_profiles 之类的嵌套模式：
/// `{"station_1": {"_box_slot_1_pose": "{...}", "enabled": true, ...}, ...}`
fn classify_object_value(map: &serde_json::Map<String, serde_json::Value>) -> ContextValue {
    // BTreeMap 保证按 key 排序，与顶层 context 行为一致
    let sorted: BTreeMap<_, _> = map.iter().collect();
    let fields = sorted
        .into_iter()
        .map(|(key, value)| ContextField {
            key: key.clone(),
            value: classify_context_value(key, value),
        })
        .collect();
    ContextValue::NestedGroup(fields)
}

/// 识别 context 中一个值的类型
fn classify_context_value(key: &str, value: &serde_json::Value) -> ContextValue {
    match value {
        serde_json::Value::Null => ContextValue::Null,
        serde_json::Value::Bool(b) => ContextValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ContextValue::Integer(i)
            } else if n.is_f64()
                && let Some(f) = n.as_f64()
            {
                ContextValue::Float(f)
            } else {
                ContextValue::RawJson(value.clone())
            }
        }
        serde_json::Value::String(s) => classify_string_value(s),
        serde_json::Value::Array(arr) => classify_array_value(key, value, arr),
        serde_json::Value::Object(map) => classify_object_value(map),
    }
}

// ============================================================
// 解析与序列化
// ============================================================

/// 解析一层图的节点与边
fn parse_subgraph(container: &serde_json::Value) -> SubGraph {
    let nodes = container
        .get("nodes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_node).collect())
        .unwrap_or_default();
    let edges = container
        .get("edges")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    Some(GraphEdge {
                        from: e.get("from")?.as_str()?.to_string(),
                        to: e.get("to")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    SubGraph { nodes, edges }
}

/// 解析单个节点；没有 id 的条目直接跳过
fn parse_node(value: &serde_json::Value) -> Option<TaskNode> {
    let id = value.get("id")?.as_str()?.to_string();
    let children = value.get("nodes").is_some().then(|| parse_subgraph(value));
    Some(TaskNode {
        id,
        node_type: value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        inputs: value
            .get("inputs")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        checkpoint: value
            .get("checkpoint")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        children,
    })
}

/// 从 JSON 字符串解析任务图数据
///
/// 提取 map_id、task_id，并遍历 config.context 中所有字段，
/// 自动识别类型（位姿、轨迹、数组、标量、布尔等）。
pub fn parse_task_graph(json_str: &str) -> Result<TaskGraphData, ParseError> {
    let raw: serde_json::Value = serde_json::from_str(json_str)?;

    let map_id = raw
        .get("map_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ParseError::MissingField("map_id".into()))?
        .to_string();

    let task_id = raw
        .get("task_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ParseError::MissingField("task_id".into()))?
        .to_string();

    let mut context_fields = Vec::new();

    if let Some(context) = raw
        .get("config")
        .and_then(|c| c.get("context"))
        .and_then(|c| c.as_object())
    {
        // BTreeMap 保证按 key 排序
        let sorted: BTreeMap<_, _> = context.iter().collect();
        for (key, value) in sorted {
            context_fields.push(ContextField {
                key: key.clone(),
                value: classify_context_value(key, value),
            });
        }
    }

    let graph = raw.get("config").map(parse_subgraph).unwrap_or_default();

    Ok(TaskGraphData {
        map_id,
        task_id,
        context_fields,
        graph,
        raw_json: raw,
    })
}

/// 将 f64 值转为 JSON 值，整数输出为整数格式
fn numeric_to_json_value(v: f64) -> serde_json::Value {
    if v.fract() == 0.0 && v.abs() < i64::MAX as f64 {
        serde_json::Value::Number(serde_json::Number::from(v as i64))
    } else {
        serde_json::json!(v)
    }
}

/// 保存的最后一道数值防线：serde_json 会把非有限数静默转换为 null，必须提前拒绝。
fn validate_finite(value: &ContextValue) -> Result<(), serde_json::Error> {
    fn pose_is_finite(pose: &RobotPose) -> bool {
        [&pose.chassis_pose, &pose.head_pose, &pose.waist_pose]
            .into_iter()
            .all(|part| {
                let p = &part.position;
                let q = &part.orientation;
                [p.x, p.y, p.z, q.w, q.x, q.y, q.z]
                    .into_iter()
                    .all(f64::is_finite)
            })
    }
    let finite = match value {
        ContextValue::Pose(pose) => pose_is_finite(pose),
        ContextValue::Float(number) => number.is_finite(),
        ContextValue::NumericArray { values, .. } => values.iter().all(|v| v.is_finite()),
        ContextValue::NumericArray2D { rows, .. } => rows.iter().flatten().all(|v| v.is_finite()),
        ContextValue::JointTrajectory(points) => points.iter().all(|point| {
            point.time_from_start.is_finite() && point.positions.iter().all(|v| v.is_finite())
        }),
        ContextValue::PoseArray(poses) => poses.iter().all(pose_is_finite),
        ContextValue::NestedGroup(fields) => {
            for field in fields {
                validate_finite(&field.value)?;
            }
            true
        }
        _ => true,
    };
    match finite {
        true => Ok(()),
        false => Err(<serde_json::Error as serde::ser::Error>::custom(
            "不能保存非有限数值（NaN 或无穷大），请修正对应数值",
        )),
    }
}

/// 将 ContextValue 序列化为 JSON 值
fn serialize_context_value(value: &ContextValue) -> Result<serde_json::Value, serde_json::Error> {
    validate_finite(value)?;
    Ok(match value {
        ContextValue::Pose(pose) => serde_json::Value::String(serde_json::to_string(pose)?),
        ContextValue::Bool(b) => serde_json::Value::Bool(*b),
        ContextValue::Integer(i) => serde_json::json!(*i),
        ContextValue::Float(f) => serde_json::json!(*f),
        ContextValue::NumericArray {
            values,
            stringified,
        } => match stringified {
            // 字符串化的数组沿用既有规则：整数值写成整数，"[4,3]" 不会变成 "[4.0,3.0]"
            true => {
                let json_arr: Vec<serde_json::Value> =
                    values.iter().map(|&v| numeric_to_json_value(v)).collect();
                serde_json::Value::String(serde_json::to_string(&json_arr)?)
            }
            // 原生数组照 f64 写回：这里再做整数收敛会把原文的 0.0 改成 0，
            // JSON 类型跟着变，属于悄悄改数据
            false => serde_json::to_value(values)?,
        },
        ContextValue::NumericArray2D { rows, stringified } => match stringified {
            true => {
                let json_arr: Vec<Vec<serde_json::Value>> = rows
                    .iter()
                    .map(|row| row.iter().map(|&v| numeric_to_json_value(v)).collect())
                    .collect();
                serde_json::Value::String(serde_json::to_string(&json_arr)?)
            }
            false => serde_json::to_value(rows)?,
        },
        ContextValue::JointTrajectory(traj) => serde_json::to_value(traj)?,
        ContextValue::PoseArray(poses) => serde_json::to_value(poses)?,
        ContextValue::NestedGroup(fields) => {
            let mut map = serde_json::Map::with_capacity(fields.len());
            for field in fields {
                map.insert(field.key.clone(), serialize_context_value(&field.value)?);
            }
            serde_json::Value::Object(map)
        }
        ContextValue::Text(s) => serde_json::Value::String(s.clone()),
        ContextValue::Null => serde_json::Value::Null,
        ContextValue::RawJson(v) => v.clone(),
    })
}

/// 在原始结构上只覆盖变动的叶子；模型未识别的扩展成员以及未改数值的整数/浮点形式均保留。
fn merge_edited_json(
    original: &serde_json::Value,
    before: &serde_json::Value,
    after: &serde_json::Value,
) -> serde_json::Value {
    if before == after {
        return original.clone();
    }
    match (original, before, after) {
        (
            serde_json::Value::Object(raw),
            serde_json::Value::Object(old),
            serde_json::Value::Object(new),
        ) => {
            let mut merged = raw.clone();
            for (key, value) in new {
                let value = match (raw.get(key), old.get(key)) {
                    (Some(original), Some(before)) => merge_edited_json(original, before, value),
                    _ => value.clone(),
                };
                merged.insert(key.clone(), value);
            }
            serde_json::Value::Object(merged)
        }
        (
            serde_json::Value::Array(raw),
            serde_json::Value::Array(old),
            serde_json::Value::Array(new),
        ) => serde_json::Value::Array(
            new.iter()
                .enumerate()
                .map(|(index, value)| match (raw.get(index), old.get(index)) {
                    (Some(original), Some(before)) => merge_edited_json(original, before, value),
                    _ => value.clone(),
                })
                .collect(),
        ),
        (serde_json::Value::Number(raw), _, serde_json::Value::Number(new)) => match new.as_f64() {
            Some(number) if (raw.is_i64() || raw.is_u64()) && number.fract() == 0.0 => {
                numeric_to_json_value(number)
            }
            _ => after.clone(),
        },
        _ => after.clone(),
    }
}

/// 只对位姿的已知成员指定顺序，扩展成员仍完整输出。
/// serde_json::Value 的对象是 BTreeMap，直接写出会把 orientation 排到 position 前面。
struct OrderedPoseJson<'a> {
    value: &'a serde_json::Value,
    kind: PoseJsonKind,
}

#[derive(Clone, Copy)]
enum PoseJsonKind {
    Robot,
    Part,
    Position,
    Orientation,
    Extension,
}

impl Serialize for OrderedPoseJson<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let Some(object) = self.value.as_object() else {
            return self.value.serialize(serializer);
        };
        let keys: &[&str] = match self.kind {
            PoseJsonKind::Robot => &["chassis_pose", "head_pose", "waist_pose"],
            PoseJsonKind::Part => &["position", "orientation"],
            PoseJsonKind::Position => &["x", "y", "z"],
            PoseJsonKind::Orientation => &["w", "x", "y", "z"],
            PoseJsonKind::Extension => &[],
        };
        let mut map = serializer.serialize_map(Some(object.len()))?;
        for key in keys {
            if let Some(value) = object.get(*key) {
                let kind = match (self.kind, *key) {
                    (PoseJsonKind::Robot, _) => PoseJsonKind::Part,
                    (PoseJsonKind::Part, "position") => PoseJsonKind::Position,
                    (PoseJsonKind::Part, "orientation") => PoseJsonKind::Orientation,
                    _ => PoseJsonKind::Extension,
                };
                map.serialize_entry(key, &OrderedPoseJson { value, kind })?;
            }
        }
        for (key, value) in object {
            if !keys.contains(&key.as_str()) {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

fn merge_context_value(
    key: &str,
    value: &ContextValue,
    original: Option<&serde_json::Value>,
) -> Result<serde_json::Value, serde_json::Error> {
    validate_finite(value)?;
    let Some(original) = original else {
        return serialize_context_value(value);
    };
    let before = classify_context_value(key, original);
    if &before == value {
        return Ok(original.clone());
    }
    if let ContextValue::NestedGroup(fields) = value {
        let mut merged = original.as_object().cloned().unwrap_or_default();
        for field in fields {
            let next = merge_context_value(&field.key, &field.value, merged.get(&field.key))?;
            merged.insert(field.key.clone(), next);
        }
        return Ok(serde_json::Value::Object(merged));
    }
    // 类型切换（如 null 转位姿）没有同形的旧模型，直接序列化新值。
    if std::mem::discriminant(&before) != std::mem::discriminant(value) {
        return serialize_context_value(value);
    }
    let old = serialize_context_value(&before)?;
    let new = serialize_context_value(value)?;
    match (original.as_str(), old.as_str(), new.as_str()) {
        (Some(raw), Some(old), Some(new))
            if matches!(
                value,
                ContextValue::Pose(_)
                    | ContextValue::NumericArray {
                        stringified: true,
                        ..
                    }
                    | ContextValue::NumericArray2D {
                        stringified: true,
                        ..
                    }
            ) =>
        {
            let merged = merge_edited_json(
                &serde_json::from_str(raw)?,
                &serde_json::from_str(old)?,
                &serde_json::from_str(new)?,
            );
            let text = match value {
                ContextValue::Pose(_) => serde_json::to_string(&OrderedPoseJson {
                    value: &merged,
                    kind: PoseJsonKind::Robot,
                })?,
                _ => serde_json::to_string(&merged)?,
            };
            Ok(serde_json::Value::String(text))
        }
        _ => Ok(merge_edited_json(original, &old, &new)),
    }
}

/// 将编辑后的数据合并回原始 JSON；未修改的 context 保留原始值及字符串文本。
pub fn serialize_task_graph(data: &TaskGraphData) -> Result<String, serde_json::Error> {
    for field in &data.context_fields {
        validate_finite(&field.value)?;
    }
    let mut json = data.raw_json.clone();
    json["map_id"] = serde_json::Value::String(data.map_id.clone());
    json["task_id"] = serde_json::Value::String(data.task_id.clone());
    if let Some(context) = json
        .get_mut("config")
        .and_then(|c| c.get_mut("context"))
        .and_then(|c| c.as_object_mut())
    {
        for field in &data.context_fields {
            let value = merge_context_value(&field.key, &field.value, context.get(&field.key))?;
            context.insert(field.key.clone(), value);
        }
    }
    serde_json::to_string_pretty(&json)
}

// ============================================================
// 索引路径查找（支持嵌套分组下钻）
// ============================================================

/// 沿索引路径查找字段（不可变）
///
/// 路径首元素索引顶层 `context_fields`，后续元素依次下钻嵌套分组。
/// 路径为空、索引越界、或中间节点不是嵌套分组时返回 `None`。
pub fn field_at_path<'a>(fields: &'a [ContextField], path: &[usize]) -> Option<&'a ContextField> {
    let (&first, rest) = path.split_first()?;
    let field = fields.get(first)?;
    if rest.is_empty() {
        return Some(field);
    }
    match &field.value {
        ContextValue::NestedGroup(children) => field_at_path(children, rest),
        _ => None,
    }
}

/// 沿索引路径查找字段（可变）
pub fn field_at_path_mut<'a>(
    fields: &'a mut [ContextField],
    path: &[usize],
) -> Option<&'a mut ContextField> {
    let (&first, rest) = path.split_first()?;
    let field = fields.get_mut(first)?;
    if rest.is_empty() {
        return Some(field);
    }
    match &mut field.value {
        ContextValue::NestedGroup(children) => field_at_path_mut(children, rest),
        _ => None,
    }
}

/// 将索引路径转换为 key 路径字符串（如 `station_profiles.station_2._box_slot_1_pose`）
pub fn key_path_string(fields: &[ContextField], path: &[usize]) -> String {
    let mut parts = Vec::with_capacity(path.len());
    let mut current = fields;
    for &idx in path {
        let Some(field) = current.get(idx) else { break };
        parts.push(field.key.as_str());
        current = match &field.value {
            ContextValue::NestedGroup(children) => children,
            _ => &[],
        };
    }
    parts.join(".")
}

// ============================================================
// 登录配置持久化
// ============================================================

/// 登录配置（持久化到本地，下次启动自动加载）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginConfig {
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    /// 私钥文件路径（密码为空时使用；空 = ssh-agent / 默认私钥）
    #[serde(default)]
    pub identity_file: String,
    /// ROS_DOMAIN_ID，不同机器人可能不同
    #[serde(default)]
    pub ros_domain_id: String,
    /// 远程任务图文件目录
    #[serde(default = "default_remote_dir")]
    pub remote_dir: String,
}

fn default_remote_dir() -> String {
    "/home/linux/Workspace/task_graphs".into()
}

impl Default for LoginConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: "22".into(),
            username: "linux".into(),
            password: String::new(),
            identity_file: String::new(),
            ros_domain_id: "11".into(),
            remote_dir: default_remote_dir(),
        }
    }
}

/// 配置文件路径: ~/.config/task-graph-editor/login.json
fn config_path() -> PathBuf {
    crate::ssh_config::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("task-graph-editor")
        .join("login.json")
}

/// 加载上次保存的登录配置
pub fn load_login_config() -> LoginConfig {
    let path = config_path();
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 保存登录配置到本地
pub fn save_login_config(config: &LoginConfig) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(config) {
        let _ = std::fs::write(path, json);
    }
}

// ============================================================
// ROS2 tracked_pose 输出解析
// ============================================================

/// 解析 `ros2 topic echo /tracked_pose --once` 的 YAML 输出，
/// 提取 pose.position 和 pose.orientation 填入 chassis_pose
pub fn parse_tracked_pose(output: &str) -> Option<Pose> {
    // 状态机: 0=初始, 1=在pose下, 2=在position下, 3=在orientation下
    let mut state = 0u8;
    let mut pos = Position {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    let mut ori = Orientation {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    let mut found = false;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed == "---" || trimmed.is_empty() {
            continue;
        }

        match trimmed {
            "pose:" => state = 1,
            "position:" if state >= 1 => state = 2,
            "orientation:" if state >= 1 => state = 3,
            _ => {
                if (state == 2 || state == 3)
                    && let Some((key, val_str)) = trimmed.split_once(':')
                    && let Ok(val) = val_str.trim().parse::<f64>()
                {
                    let key = key.trim();
                    match (state, key) {
                        (2, "x") => {
                            pos.x = val;
                            found = true;
                        }
                        (2, "y") => pos.y = val,
                        (2, "z") => pos.z = val,
                        (3, "w") => ori.w = val,
                        (3, "x") => ori.x = val,
                        (3, "y") => ori.y = val,
                        (3, "z") => ori.z = val,
                        _ => {}
                    }
                }
            }
        }
    }

    if found {
        Some(Pose {
            position: pos,
            orientation: ori,
        })
    } else {
        None
    }
}

// ============================================================
// ROS2 joint_states 输出解析
// ============================================================

/// 关节角数据（从 /joint_states 话题获取）
#[derive(Debug, Clone, PartialEq)]
pub struct JointAngles {
    /// head_joint_1 → head_pose.position.x
    pub head_joint_1: f64,
    /// head_joint_2 → head_pose.position.y
    pub head_joint_2: f64,
    /// body_joint_1 → waist_pose.position.x
    pub body_joint_1: f64,
    /// body_joint_2 → waist_pose.position.y
    pub body_joint_2: f64,
}

/// 解析 joint_states Python 脚本的输出
///
/// 输入格式: `head_joint_1=-0.314158499 head_joint_2=0.000042716 body_joint_1=0.679999937 body_joint_2=0.299999416`
pub fn parse_joint_states(output: &str) -> Option<JointAngles> {
    let mut angles = JointAngles {
        head_joint_1: 0.0,
        head_joint_2: 0.0,
        body_joint_1: 0.0,
        body_joint_2: 0.0,
    };
    let mut found = 0u8;

    for token in output.split_whitespace() {
        if let Some((key, val_str)) = token.split_once('=')
            && let Ok(val) = val_str.parse::<f64>()
        {
            match key {
                "head_joint_1" => {
                    angles.head_joint_1 = val;
                    found |= 1;
                }
                "head_joint_2" => {
                    angles.head_joint_2 = val;
                    found |= 2;
                }
                "body_joint_1" => {
                    angles.body_joint_1 = val;
                    found |= 4;
                }
                "body_joint_2" => {
                    angles.body_joint_2 = val;
                    found |= 8;
                }
                _ => {}
            }
        }
    }

    // 四个字段全部找到才算解析成功
    if found == 0b1111 { Some(angles) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_JSON: &str = r#"{
        "config": {
            "task_meta": { "id": "test", "name": "测试", "version": "1.0", "description": "测试" },
            "checkpoint_policy": { "enabled": true, "auto_save_interval": 0, "max_checkpoints": 10 },
            "context": {
                "point_a": "{\"chassis_pose\":{\"position\":{\"x\":1.0,\"y\":2.0,\"z\":0.0},\"orientation\":{\"w\":1.0,\"x\":0.0,\"y\":0.0,\"z\":0.0}},\"head_pose\":{\"position\":{\"x\":0.0,\"y\":0.0,\"z\":0.0},\"orientation\":{\"w\":1.0,\"x\":0.0,\"y\":0.0,\"z\":0.0}},\"waist_pose\":{\"position\":{\"x\":0.5,\"y\":0.3,\"z\":0.0},\"orientation\":{\"w\":1.0,\"x\":0.0,\"y\":0.0,\"z\":0.0}}}",
                "some_number": 42,
                "some_string": "not_a_pose",
                "some_bool": true,
                "some_float": 1.234,
                "heights": "[0.01,0.17,0.34]",
                "angles_2d": "[[0.1,0.2],[0.3,0.4]]",
                "null_val": null,
                "pick_poses": [],
                "jt1_traj": [
                    {"positions": [1.0, 2.0, 3.0], "time_from_start": 0.5},
                    {"positions": [4.0, 5.0, 6.0], "time_from_start": 1.0}
                ],
                "active_profile": {},
                "station_profiles": {
                    "station_1": {
                        "slot_pose": "{\"chassis_pose\":{\"position\":{\"x\":-3.8,\"y\":0.34,\"z\":0.0},\"orientation\":{\"w\":1.0,\"x\":0.0,\"y\":0.0,\"z\":0.0}},\"head_pose\":{\"position\":{\"x\":0.0,\"y\":-0.12,\"z\":0.0},\"orientation\":{\"w\":1.0,\"x\":0.0,\"y\":0.0,\"z\":0.0}},\"waist_pose\":{\"position\":{\"x\":0.68,\"y\":0.3,\"z\":0.0},\"orientation\":{\"w\":1.0,\"x\":0.0,\"y\":0.0,\"z\":0.0}}}",
                        "enabled": true,
                        "capacity": 16,
                        "pick_height": 0.5,
                        "put_heights": "[ 0.04, 0.21, 0.40]",
                        "warning": "示教点位提醒"
                    },
                    "station_2": {
                        "enabled": false,
                        "capacity": 8
                    }
                }
            },
            "nodes": [],
            "edges": []
        },
        "created_at": { "sec": 1000, "nanosec": 0 },
        "map_id": "test-map-id",
        "task_id": "test-task"
    }"#;

    #[test]
    fn test_parse_extracts_metadata() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        assert_eq!(data.map_id, "test-map-id");
        assert_eq!(data.task_id, "test-task");
    }

    #[test]
    fn test_parse_extracts_all_context_fields() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        // active_profile, angles_2d, heights, jt1_traj, null_val, pick_poses,
        // point_a, some_bool, some_float, some_number, some_string, station_profiles
        assert_eq!(data.context_fields.len(), 12);
    }

    #[test]
    fn test_parse_pose_field() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "point_a")
            .unwrap();
        match &field.value {
            ContextValue::Pose(pose) => {
                assert!((pose.chassis_pose.position.x - 1.0).abs() < f64::EPSILON);
            }
            other => panic!("Expected Pose, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_integer_field() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "some_number")
            .unwrap();
        assert!(matches!(field.value, ContextValue::Integer(42)));
    }

    #[test]
    fn test_parse_float_field() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "some_float")
            .unwrap();
        match &field.value {
            ContextValue::Float(f) => assert!((*f - 1.234).abs() < f64::EPSILON),
            other => panic!("Expected Float, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_bool_field() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "some_bool")
            .unwrap();
        assert!(matches!(field.value, ContextValue::Bool(true)));
    }

    #[test]
    fn test_parse_numeric_array() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "heights")
            .unwrap();
        match &field.value {
            ContextValue::NumericArray {
                values,
                stringified,
            } => {
                assert_eq!(values.len(), 3);
                assert!((values[0] - 0.01).abs() < f64::EPSILON);
                assert!(stringified, "原文是字符串化的数组");
            }
            other => panic!("Expected NumericArray, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_numeric_array_2d() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "angles_2d")
            .unwrap();
        match &field.value {
            ContextValue::NumericArray2D { rows, stringified } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
                assert!(stringified, "原文是字符串化的数组");
            }
            other => panic!("Expected NumericArray2D, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_null_field() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "null_val")
            .unwrap();
        assert!(matches!(field.value, ContextValue::Null));
    }

    #[test]
    fn test_parse_pose_array() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "pick_poses")
            .unwrap();
        match &field.value {
            ContextValue::PoseArray(poses) => assert!(poses.is_empty()),
            other => panic!("Expected PoseArray, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_trajectory() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "jt1_traj")
            .unwrap();
        match &field.value {
            ContextValue::JointTrajectory(traj) => {
                assert_eq!(traj.len(), 2);
                assert_eq!(traj[0].positions.len(), 3);
                assert!((traj[0].time_from_start - 0.5).abs() < f64::EPSILON);
            }
            other => panic!("Expected JointTrajectory, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_text_field() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let field = data
            .context_fields
            .iter()
            .find(|f| f.key == "some_string")
            .unwrap();
        match &field.value {
            ContextValue::Text(s) => assert_eq!(s, "not_a_pose"),
            other => panic!("Expected Text, got {other:?}"),
        }
    }

    /// 从字段列表中按 key 查找嵌套分组的成员列表
    fn nested_fields<'a>(fields: &'a [ContextField], key: &str) -> &'a [ContextField] {
        let field = fields.iter().find(|f| f.key == key).unwrap();
        match &field.value {
            ContextValue::NestedGroup(children) => children,
            other => panic!("Expected NestedGroup, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_empty_nested_group() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        assert!(nested_fields(&data.context_fields, "active_profile").is_empty());
    }

    #[test]
    fn test_parse_nested_group_classifies_members() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let stations = nested_fields(&data.context_fields, "station_profiles");
        assert_eq!(stations.len(), 2);

        // station_1 内部字段递归分类：位姿、布尔、整数、浮点、字符串化数组、文本
        let station_1 = nested_fields(stations, "station_1");
        assert_eq!(station_1.len(), 6);

        let slot_pose = station_1.iter().find(|f| f.key == "slot_pose").unwrap();
        match &slot_pose.value {
            ContextValue::Pose(pose) => {
                assert!((pose.chassis_pose.position.x - (-3.8)).abs() < f64::EPSILON);
                assert!((pose.waist_pose.position.x - 0.68).abs() < f64::EPSILON);
            }
            other => panic!("Expected Pose, got {other:?}"),
        }

        let enabled = station_1.iter().find(|f| f.key == "enabled").unwrap();
        assert!(matches!(enabled.value, ContextValue::Bool(true)));

        let capacity = station_1.iter().find(|f| f.key == "capacity").unwrap();
        assert!(matches!(capacity.value, ContextValue::Integer(16)));

        let heights = station_1.iter().find(|f| f.key == "put_heights").unwrap();
        match &heights.value {
            ContextValue::NumericArray { values, .. } => assert_eq!(values.len(), 3),
            other => panic!("Expected NumericArray, got {other:?}"),
        }

        let warning = station_1.iter().find(|f| f.key == "warning").unwrap();
        match &warning.value {
            ContextValue::Text(s) => assert_eq!(s, "示教点位提醒"),
            other => panic!("Expected Text, got {other:?}"),
        }
    }

    #[test]
    fn test_nested_roundtrip_preserves_edit() {
        let mut data = parse_task_graph(TEST_JSON).unwrap();

        // 定位 station_profiles → station_1 → slot_pose 并修改底盘 x
        let sp_idx = data
            .context_fields
            .iter()
            .position(|f| f.key == "station_profiles")
            .unwrap();
        let path = {
            let stations = nested_fields(&data.context_fields, "station_profiles");
            let st_idx = stations.iter().position(|f| f.key == "station_1").unwrap();
            let station_1 = nested_fields(stations, "station_1");
            let pose_idx = station_1.iter().position(|f| f.key == "slot_pose").unwrap();
            vec![sp_idx, st_idx, pose_idx]
        };

        let field = field_at_path_mut(&mut data.context_fields, &path).unwrap();
        match &mut field.value {
            ContextValue::Pose(pose) => pose.chassis_pose.position.x = 77.5,
            other => panic!("Expected Pose, got {other:?}"),
        }

        let output = serialize_task_graph(&data).unwrap();
        let reparsed = parse_task_graph(&output).unwrap();

        // 修改后的位姿保留；整数容量保持整数格式；布尔保持
        let stations = nested_fields(&reparsed.context_fields, "station_profiles");
        let station_1 = nested_fields(stations, "station_1");
        let slot_pose = station_1.iter().find(|f| f.key == "slot_pose").unwrap();
        match &slot_pose.value {
            ContextValue::Pose(pose) => {
                assert!((pose.chassis_pose.position.x - 77.5).abs() < f64::EPSILON);
            }
            other => panic!("Expected Pose, got {other:?}"),
        }
        let capacity = station_1.iter().find(|f| f.key == "capacity").unwrap();
        assert!(matches!(capacity.value, ContextValue::Integer(16)));

        // 嵌套整数在 JSON 文本中保持整数字面量
        assert!(output.contains("\"capacity\": 16"));

        // 空分组序列化后仍是空对象
        assert!(nested_fields(&reparsed.context_fields, "active_profile").is_empty());
    }

    #[test]
    fn test_field_at_path_lookup() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let sp_idx = data
            .context_fields
            .iter()
            .position(|f| f.key == "station_profiles")
            .unwrap();

        // 顶层单元素路径
        let field = field_at_path(&data.context_fields, &[sp_idx]).unwrap();
        assert_eq!(field.key, "station_profiles");

        // 两级下钻（嵌套分组按 key 排序，station_1 在前）
        let field = field_at_path(&data.context_fields, &[sp_idx, 0]).unwrap();
        assert_eq!(field.key, "station_1");

        // 空路径 / 越界 / 中间节点非嵌套分组均返回 None
        assert!(field_at_path(&data.context_fields, &[]).is_none());
        assert!(field_at_path(&data.context_fields, &[999]).is_none());
        assert!(field_at_path(&data.context_fields, &[sp_idx, 0, 999]).is_none());
        let point_a_idx = data
            .context_fields
            .iter()
            .position(|f| f.key == "point_a")
            .unwrap();
        assert!(field_at_path(&data.context_fields, &[point_a_idx, 0]).is_none());
    }

    #[test]
    fn test_key_path_string() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let sp_idx = data
            .context_fields
            .iter()
            .position(|f| f.key == "station_profiles")
            .unwrap();
        assert_eq!(
            key_path_string(&data.context_fields, &[sp_idx]),
            "station_profiles"
        );
        assert_eq!(
            key_path_string(&data.context_fields, &[sp_idx, 0]),
            "station_profiles.station_1"
        );
    }

    #[test]
    fn test_roundtrip_preserves_data() {
        let mut data = parse_task_graph(TEST_JSON).unwrap();
        data.map_id = "new-map".into();
        for field in &mut data.context_fields {
            if field.key == "point_a"
                && let ContextValue::Pose(ref mut pose) = field.value
            {
                pose.chassis_pose.position.x = 99.0;
            }
        }

        let output = serialize_task_graph(&data).unwrap();
        let reparsed = parse_task_graph(&output).unwrap();

        assert_eq!(reparsed.map_id, "new-map");
        let field = reparsed
            .context_fields
            .iter()
            .find(|f| f.key == "point_a")
            .unwrap();
        match &field.value {
            ContextValue::Pose(pose) => {
                assert!((pose.chassis_pose.position.x - 99.0).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Pose"),
        }
    }

    #[test]
    fn test_roundtrip_preserves_all_types() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        let output = serialize_task_graph(&data).unwrap();
        let reparsed = parse_task_graph(&output).unwrap();

        assert_eq!(data.context_fields.len(), reparsed.context_fields.len());

        // 验证整数保持为整数
        let field = reparsed
            .context_fields
            .iter()
            .find(|f| f.key == "some_number")
            .unwrap();
        assert!(matches!(field.value, ContextValue::Integer(42)));

        // 验证浮点保持为浮点
        let field = reparsed
            .context_fields
            .iter()
            .find(|f| f.key == "some_float")
            .unwrap();
        assert!(matches!(field.value, ContextValue::Float(_)));

        // 验证轨迹保持完整
        let field = reparsed
            .context_fields
            .iter()
            .find(|f| f.key == "jt1_traj")
            .unwrap();
        match &field.value {
            ContextValue::JointTrajectory(traj) => assert_eq!(traj.len(), 2),
            other => panic!("Expected JointTrajectory, got {other:?}"),
        }
    }

    #[test]
    fn test_missing_map_id() {
        let json = r#"{"config":{"context":{}},"task_id":"t"}"#;
        assert!(parse_task_graph(json).is_err());
    }

    #[test]
    fn test_parse_tracked_pose() {
        let output = "\
header:
  stamp:
    sec: 1772506352
    nanosec: 313712300
  frame_id: map
pose:
  position:
    x: 0.7197834644638161
    y: 0.18714868614716318
    z: 0.0
  orientation:
    x: 0.0
    y: 0.0
    z: 0.037550545079943314
    w: 0.9992947295789162
---";
        let pose = parse_tracked_pose(output).unwrap();
        assert!((pose.position.x - 0.7197834644638161).abs() < f64::EPSILON);
        assert!((pose.position.y - 0.18714868614716318).abs() < f64::EPSILON);
        assert!((pose.position.z - 0.0).abs() < f64::EPSILON);
        assert!((pose.orientation.w - 0.9992947295789162).abs() < f64::EPSILON);
        assert!((pose.orientation.z - 0.037550545079943314).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_tracked_pose_empty() {
        assert!(parse_tracked_pose("").is_none());
        assert!(parse_tracked_pose("random text").is_none());
    }

    #[test]
    fn test_parse_joint_states() {
        let output = "head_joint_1=-0.314158499 head_joint_2=0.000042716 body_joint_1=0.679999937 body_joint_2=0.299999416";
        let angles = parse_joint_states(output).unwrap();
        assert!((angles.head_joint_1 - (-0.314158499)).abs() < f64::EPSILON);
        assert!((angles.head_joint_2 - 0.000042716).abs() < f64::EPSILON);
        assert!((angles.body_joint_1 - 0.679999937).abs() < f64::EPSILON);
        assert!((angles.body_joint_2 - 0.299999416).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_joint_states_with_trailing_newline() {
        let output = "head_joint_1=1.0 head_joint_2=2.0 body_joint_1=3.0 body_joint_2=4.0\n";
        let angles = parse_joint_states(output).unwrap();
        assert!((angles.head_joint_1 - 1.0).abs() < f64::EPSILON);
        assert!((angles.body_joint_2 - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_joint_states_incomplete() {
        // 缺少 body_joint_2
        assert!(parse_joint_states("head_joint_1=1.0 head_joint_2=2.0 body_joint_1=3.0").is_none());
        assert!(parse_joint_states("").is_none());
        assert!(parse_joint_states("random text").is_none());
    }

    /// 语义标准化：字符串化 JSON（对象/数组）解析为原生结构后递归处理，
    /// 用于对比 roundtrip 前后 context 是否语义等价
    fn normalize_json(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::String(s) => {
                if let Ok(inner) = serde_json::from_str::<serde_json::Value>(s)
                    && (inner.is_object() || inner.is_array())
                {
                    return normalize_json(&inner);
                }
                v.clone()
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(normalize_json).collect())
            }
            serde_json::Value::Object(map) => serde_json::Value::Object(
                map.iter()
                    .map(|(k, val)| (k.clone(), normalize_json(val)))
                    .collect(),
            ),
            _ => v.clone(),
        }
    }

    /// 递归定位两个 JSON 值第一处差异的路径（用于失败时输出可读诊断）
    fn find_first_diff(a: &serde_json::Value, b: &serde_json::Value, path: &str) -> Option<String> {
        match (a, b) {
            (serde_json::Value::Object(ma), serde_json::Value::Object(mb)) => {
                for (k, va) in ma {
                    match mb.get(k) {
                        Some(vb) => {
                            if let Some(diff) = find_first_diff(va, vb, &format!("{path}.{k}")) {
                                return Some(diff);
                            }
                        }
                        None => return Some(format!("{path}.{k}: 仅左侧存在")),
                    }
                }
                for k in mb.keys() {
                    if !ma.contains_key(k) {
                        return Some(format!("{path}.{k}: 仅右侧存在"));
                    }
                }
                None
            }
            (serde_json::Value::Array(aa), serde_json::Value::Array(ab)) => {
                if aa.len() != ab.len() {
                    return Some(format!("{path}: 数组长度 {} != {}", aa.len(), ab.len()));
                }
                for (i, (va, vb)) in aa.iter().zip(ab).enumerate() {
                    if let Some(diff) = find_first_diff(va, vb, &format!("{path}[{i}]")) {
                        return Some(diff);
                    }
                }
                None
            }
            _ => (a != b).then(|| format!("{path}: {a} != {b}")),
        }
    }

    /// 真实任务图文件 roundtrip 验证（本地手动执行，CI 跳过）：
    /// `TASK_GRAPH_REAL_FILE=/path/to/file.json cargo test -- --ignored`
    #[test]
    #[ignore = "依赖本地真实文件，通过 TASK_GRAPH_REAL_FILE 环境变量指定路径"]
    fn test_real_file_roundtrip() {
        let path =
            std::env::var("TASK_GRAPH_REAL_FILE").expect("请设置 TASK_GRAPH_REAL_FILE 环境变量");
        let content = std::fs::read_to_string(&path).expect("读取真实文件失败");

        let data = parse_task_graph(&content).expect("解析真实文件失败");
        let output = serialize_task_graph(&data).expect("序列化失败");
        let reparsed = parse_task_graph(&output).expect("重新解析失败");
        assert_eq!(data.context_fields.len(), reparsed.context_fields.len());

        // 语义等价对比：原始 context vs 输出 context（字符串化 JSON 展开后逐项对比）
        let original: serde_json::Value = serde_json::from_str(&content).unwrap();
        let rewritten: serde_json::Value = serde_json::from_str(&output).unwrap();
        let orig_ctx = normalize_json(&original["config"]["context"]);
        let new_ctx = normalize_json(&rewritten["config"]["context"]);
        if let Some(diff) = find_first_diff(&orig_ctx, &new_ctx, "context") {
            panic!("roundtrip 后 context 语义不等价，首个差异: {diff}");
        }
    }

    #[test]
    fn test_numeric_to_json_value_integer() {
        let v = numeric_to_json_value(4.0);
        assert_eq!(v, serde_json::json!(4));
    }

    #[test]
    fn test_numeric_to_json_value_float() {
        let v = numeric_to_json_value(0.01);
        assert_eq!(v, serde_json::json!(0.01));
    }
}

#[cfg(test)]
mod 数组形式保真 {
    use super::*;

    /// 原生 JSON 数组与字符串化数组混在同一份 context 里
    const 混合数组: &str = r#"{
        "map_id": "m",
        "task_id": "t",
        "config": {
            "context": {
                "原生二维": [[0.0, 0.05, 0.25], [0.2, 0.05, 0.25]],
                "原生一维": [1.5, 2.5],
                "字符串二维": "[[0.3,0.34],[0.5,0.54]]",
                "字符串一维": "[0.01,0.17,0.34]",
                "字符串整数": "[4,3]"
            }
        }
    }"#;

    fn 取(data: &TaskGraphData, key: &str) -> ContextValue {
        data.context_fields
            .iter()
            .find(|f| f.key == key)
            .unwrap_or_else(|| panic!("没有字段 {key}"))
            .value
            .clone()
    }

    #[test]
    fn 原生数组被识别且标记为非字符串化() {
        let data = parse_task_graph(混合数组).unwrap();
        match 取(&data, "原生二维") {
            ContextValue::NumericArray2D { rows, stringified } => {
                assert_eq!(rows.len(), 2);
                assert!(!stringified);
            }
            other => panic!("应为 NumericArray2D，实际 {other:?}"),
        }
        match 取(&data, "原生一维") {
            ContextValue::NumericArray {
                values,
                stringified,
            } => {
                assert_eq!(values, vec![1.5, 2.5]);
                assert!(!stringified);
            }
            other => panic!("应为 NumericArray，实际 {other:?}"),
        }
    }

    #[test]
    fn 字符串化数组仍标记为字符串化() {
        let data = parse_task_graph(混合数组).unwrap();
        assert!(matches!(
            取(&data, "字符串二维"),
            ContextValue::NumericArray2D {
                stringified: true,
                ..
            }
        ));
        assert!(matches!(
            取(&data, "字符串一维"),
            ContextValue::NumericArray {
                stringified: true,
                ..
            }
        ));
    }

    #[test]
    fn 序列化按原样写回各自的形式() {
        let data = parse_task_graph(混合数组).unwrap();
        let out: serde_json::Value =
            serde_json::from_str(&serialize_task_graph(&data).unwrap()).unwrap();
        let ctx = &out["config"]["context"];

        // 原生的仍是数组，不能变成字符串
        assert!(ctx["原生二维"].is_array(), "原生二维应保持数组");
        assert!(ctx["原生一维"].is_array(), "原生一维应保持数组");
        // 字符串化的仍是字符串
        assert!(ctx["字符串二维"].is_string(), "字符串二维应保持字符串");
        assert!(ctx["字符串一维"].is_string(), "字符串一维应保持字符串");
    }

    #[test]
    fn 原生数组里的零点零不会被写成整数() {
        // 真实任务图里的 pose_offset_configs 首元素就是 0.0，
        // 收敛成 0 会把 JSON 类型从 float 改成 int
        let data = parse_task_graph(混合数组).unwrap();
        let out: serde_json::Value =
            serde_json::from_str(&serialize_task_graph(&data).unwrap()).unwrap();
        let first = &out["config"]["context"]["原生二维"][0][0];
        assert!(first.is_f64(), "原生数组的 0.0 应保持浮点，实际 {first}");
    }

    #[test]
    fn 字符串化数组的整数仍保持整数写法() {
        // 既有行为：位姿尺寸之类的 "[4,3]" 不应变成 "[4.0,3.0]"
        let data = parse_task_graph(混合数组).unwrap();
        let out: serde_json::Value =
            serde_json::from_str(&serialize_task_graph(&data).unwrap()).unwrap();
        assert_eq!(out["config"]["context"]["字符串整数"], "[4,3]");
    }

    #[test]
    fn 未修改字符串化数组的文本完整保留() {
        // 未编辑字段不重新序列化，尾随零与空白也保留。
        let json = r#"{
            "map_id": "m", "task_id": "t",
            "config": { "context": { "带尾零": "[0.30,0.50]" } }
        }"#;
        let data = parse_task_graph(json).unwrap();
        let out: serde_json::Value =
            serde_json::from_str(&serialize_task_graph(&data).unwrap()).unwrap();
        assert_eq!(out["config"]["context"]["带尾零"], "[0.30,0.50]");
    }

    #[test]
    fn 混合数组往返后语义不变() {
        let data = parse_task_graph(混合数组).unwrap();
        let out = serialize_task_graph(&data).unwrap();
        let a: serde_json::Value = serde_json::from_str(混合数组).unwrap();
        let b: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(a, b, "往返后 JSON 应完全一致");
    }
}

/// 真实任务图的解析覆盖率诊断
///
/// 不进常规测试（`#[ignore]`），需要时手工跑：
/// ```bash
/// TGE_ANALYZE=/path/to/task.json cargo test 分析 -- --ignored --nocapture
/// ```
/// 输出每种类型的字段数、未能识别的字段（RawJson），并把往返序列化结果写到
/// `<文件>.roundtrip.json`，便于和原文件比对是否丢数据。
#[cfg(test)]
mod 真实文件分析 {
    use super::*;

    /// 递归统计每种 ContextValue 变体，并记录可疑字段
    fn 统计(
        fields: &[ContextField],
        prefix: &str,
        counts: &mut BTreeMap<&'static str, usize>,
        raw: &mut Vec<String>,
        text: &mut Vec<(String, String)>,
    ) {
        for f in fields {
            let path = match prefix.is_empty() {
                true => f.key.clone(),
                false => format!("{prefix}.{}", f.key),
            };
            let name = match &f.value {
                ContextValue::Pose(_) => "Pose",
                ContextValue::Bool(_) => "Bool",
                ContextValue::Integer(_) => "Integer",
                ContextValue::Float(_) => "Float",
                ContextValue::NumericArray { .. } => "NumericArray",
                ContextValue::NumericArray2D { .. } => "NumericArray2D",
                ContextValue::JointTrajectory(_) => "JointTrajectory",
                ContextValue::PoseArray(_) => "PoseArray",
                ContextValue::NestedGroup(_) => "NestedGroup",
                ContextValue::Text(_) => "Text",
                ContextValue::Null => "Null",
                ContextValue::RawJson(_) => "RawJson",
            };
            *counts.entry(name).or_insert(0) += 1;
            match &f.value {
                ContextValue::RawJson(v) => {
                    let s = v.to_string();
                    let head: String = s.chars().take(70).collect();
                    raw.push(format!("{path}  =>  {head}"));
                }
                ContextValue::Text(t) => {
                    let head: String = t.chars().take(50).collect();
                    text.push((path.clone(), head));
                }
                ContextValue::NestedGroup(children) => {
                    统计(children, &path, counts, raw, text);
                }
                _ => {}
            }
        }
    }

    #[test]
    #[ignore = "需要用 TGE_ANALYZE 指定真实文件"]
    fn 分析() {
        let path = std::env::var("TGE_ANALYZE").expect("需要 TGE_ANALYZE");
        let text_in = std::fs::read_to_string(&path).expect("读取失败");
        let data = parse_task_graph(&text_in).expect("解析失败");

        let mut counts = BTreeMap::new();
        let mut raw = Vec::new();
        let mut texts = Vec::new();
        统计(&data.context_fields, "", &mut counts, &mut raw, &mut texts);

        println!(
            "\n===== 顶层 context 字段：{} =====",
            data.context_fields.len()
        );
        println!("map_id = {}  task_id = {}", data.map_id, data.task_id);
        println!("\n----- 递归分类统计（含嵌套） -----");
        for (k, v) in &counts {
            println!("{k:<16} {v}");
        }

        println!("\n----- RawJson（未能识别的类型）：{} -----", raw.len());
        for line in &raw {
            println!("  {line}");
        }

        println!("\n----- Text（按纯文本处理）：{} -----", texts.len());
        for (k, v) in texts.iter().take(30) {
            println!("  {k}  =>  {v:?}");
        }

        // 往返一致性：解析后再序列化，与原文件做语义比对
        let out = serialize_task_graph(&data).expect("序列化失败");
        std::fs::write(format!("{path}.roundtrip.json"), &out).expect("写出失败");
        println!("\n往返结果已写到 {path}.roundtrip.json");
    }
}

#[cfg(test)]
mod 保存完整性测试 {
    use super::*;
    use serde_json::{Value, json};

    fn document(context: Value) -> TaskGraphData {
        parse_task_graph(
            &json!({"map_id": "m", "task_id": "t", "config": {"context": context}}).to_string(),
        )
        .unwrap()
    }

    fn saved_context(data: &TaskGraphData) -> Value {
        let saved: Value = serde_json::from_str(&serialize_task_graph(data).unwrap()).unwrap();
        saved["config"]["context"].clone()
    }

    fn field_mut<'a>(data: &'a mut TaskGraphData, key: &str) -> &'a mut ContextValue {
        &mut data
            .context_fields
            .iter_mut()
            .find(|field| field.key == key)
            .unwrap()
            .value
    }

    #[test]
    fn 未编辑和修改无关字段都保留轨迹扩展及字符串原文() {
        let context = json!({
            "trajectory": [{"positions": [1, 2.0], "time_from_start": 0.5, "velocities": [2.0], "vendor": {"mode": "smooth"}}],
            "numbers": "[ 0.30, 1e-20 ]", "enabled": false
        });
        let mut data = document(context.clone());
        assert_eq!(saved_context(&data), context);
        *field_mut(&mut data, "enabled") = ContextValue::Bool(true);
        let out = saved_context(&data);
        assert_eq!(out["trajectory"], context["trajectory"]);
        assert_eq!(out["numbers"], context["numbers"]);
        assert_eq!(out["enabled"], true);
    }

    #[test]
    fn 只改轨迹关节仍保留未知成员和其他数字类型() {
        let mut data = document(
            json!({"trajectory": [{"positions": [1, 2.0], "time_from_start": 0.5, "velocities": [3], "vendor": {"keep": true}}]}),
        );
        let ContextValue::JointTrajectory(points) = field_mut(&mut data, "trajectory") else {
            panic!("未识别轨迹")
        };
        points[0].positions[1] = 4.5;
        let out = saved_context(&data);
        assert_eq!(
            out["trajectory"][0],
            json!({"positions": [1, 4.5], "time_from_start": 0.5, "velocities": [3], "vendor": {"keep": true}})
        );
        assert!(out["trajectory"][0]["positions"][0].is_i64());
    }

    #[test]
    fn 修改嵌套位姿分量不删除所有层级的扩展成员() {
        let mut raw = serde_json::to_value(RobotPose::default()).unwrap();
        raw["tag"] = json!("calibrated");
        raw["chassis_pose"]["frame_id"] = json!("map");
        raw["chassis_pose"]["position"]["accuracy"] = json!(0.01);
        let mut data = document(json!({"group": {"pose": raw.to_string(), "other": " [1, 2] "}}));
        let ContextValue::NestedGroup(fields) = field_mut(&mut data, "group") else {
            panic!("未识别分组")
        };
        let ContextValue::Pose(pose) = &mut fields
            .iter_mut()
            .find(|field| field.key == "pose")
            .unwrap()
            .value
        else {
            panic!("未识别位姿")
        };
        pose.chassis_pose.position.x = 42.5;
        let out = saved_context(&data);
        let text = out["group"]["pose"].as_str().unwrap();
        assert!(text.starts_with("{\"chassis_pose\":{\"position\":"));
        for part in ["chassis_pose", "head_pose", "waist_pose"] {
            assert!(text.contains(&format!("\"{part}\":{{\"position\":")));
        }
        assert!(text.contains("\"orientation\":{\"w\":"));
        let pose: Value = serde_json::from_str(text).unwrap();
        raw["chassis_pose"]["position"]["x"] = json!(42.5);
        assert_eq!(pose, raw);
        assert_eq!(out["group"]["other"], " [1, 2] ");
    }

    #[test]
    fn 大整数不经过浮点分类并完整往返() {
        let context = json!({"unsigned": u64::MAX, "native": [9007199254740993_u64],
            "string": "[9007199254740993]", "matrix": [[9007199254740993_u64]],
            "trajectory": [{"positions": [9007199254740993_u64], "time_from_start": 0.0}]});
        let data = document(context.clone());
        assert!(
            data.context_fields
                .iter()
                .all(|field| matches!(field.value, ContextValue::RawJson(_)))
        );
        assert_eq!(saved_context(&data), context);
    }

    #[test]
    fn 数组中修改一个数不改变其他整数浮点形式() {
        let mut data = document(json!({"array": [1, 2.0, 3], "string": "[1,2.0,3]"}));
        for key in ["array", "string"] {
            let ContextValue::NumericArray { values, .. } = field_mut(&mut data, key) else {
                panic!("未识别数值数组")
            };
            values[2] = 4.0;
        }
        let out = saved_context(&data);
        assert_eq!(out["array"], json!([1, 2.0, 4]));
        assert_eq!(out["string"], "[1,2.0,4]");
    }

    #[test]
    fn 保存拒绝每一种浮点容器的非有限数值() {
        let mut pose = RobotPose::default();
        pose.head_pose.orientation.w = f64::INFINITY;
        let values = vec![
            ContextValue::Float(f64::INFINITY),
            ContextValue::Pose(pose.clone()),
            ContextValue::PoseArray(vec![pose]),
            ContextValue::NumericArray {
                values: vec![f64::NAN],
                stringified: false,
            },
            ContextValue::NumericArray2D {
                rows: vec![vec![f64::NEG_INFINITY]],
                stringified: true,
            },
            ContextValue::JointTrajectory(vec![TrajectoryPoint {
                positions: vec![0.0],
                time_from_start: f64::NAN,
            }]),
            ContextValue::NestedGroup(vec![ContextField {
                key: "nested".into(),
                value: ContextValue::Float(f64::INFINITY),
            }]),
        ];
        for value in values {
            let mut data = document(json!({"invalid": null}));
            *field_mut(&mut data, "invalid") = value;
            assert!(
                serialize_task_graph(&data).is_err(),
                "必须拒绝 {}",
                data.context_fields[0].key
            );
        }
    }
}
