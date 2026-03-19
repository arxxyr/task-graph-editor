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
#[derive(Debug, Clone)]
pub enum ContextValue {
    /// 位姿点（字符串化 JSON，含 chassis_pose/head_pose/waist_pose）
    Pose(RobotPose),
    /// 布尔值
    Bool(bool),
    /// 整数
    Integer(i64),
    /// 浮点数
    Float(f64),
    /// 字符串化的数值数组（如 "[0.01,0.17,0.34]"、"[4,3]"）
    NumericArray(Vec<f64>),
    /// 字符串化的二维数值数组（如 "[[0.30,0.34,0.45],...]"）
    NumericArray2D(Vec<Vec<f64>>),
    /// 关节轨迹（原生 JSON 数组，每个元素含 positions 和 time_from_start）
    JointTrajectory(Vec<TrajectoryPoint>),
    /// 位姿数组（原生 JSON 数组，可以为空）
    PoseArray(Vec<RobotPose>),
    /// 普通字符串（无法解析为位姿或数组的字符串值）
    Text(String),
    /// JSON null
    Null,
    /// 无法归类的其他 JSON 值
    RawJson(serde_json::Value),
}

/// context 中的一个字段
#[derive(Debug, Clone)]
pub struct ContextField {
    /// context 中的 key 名
    pub key: String,
    /// 值
    pub value: ContextValue,
}

/// GUI 编辑用的任务图数据
#[derive(Debug, Clone)]
pub struct TaskGraphData {
    pub map_id: String,
    pub task_id: String,
    /// 所有 context 字段（按 key 排序）
    pub context_fields: Vec<ContextField>,
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

/// 分类字符串类型的 context 值
fn classify_string_value(s: &str) -> ContextValue {
    // 尝试解析为 RobotPose
    if let Ok(pose) = serde_json::from_str::<RobotPose>(s) {
        return ContextValue::Pose(pose);
    }
    // 尝试解析为二维数值数组（必须非空，避免空数组误判）
    if let Ok(arr2d) = serde_json::from_str::<Vec<Vec<f64>>>(s)
        && !arr2d.is_empty()
    {
        return ContextValue::NumericArray2D(arr2d);
    }
    // 尝试解析为一维数值数组
    if let Ok(arr) = serde_json::from_str::<Vec<f64>>(s) {
        return ContextValue::NumericArray(arr);
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
    // 尝试解析为轨迹点数组
    if let Ok(traj) = serde_json::from_value::<Vec<TrajectoryPoint>>(value.clone()) {
        return ContextValue::JointTrajectory(traj);
    }
    // 尝试解析为位姿数组
    if let Ok(poses) = serde_json::from_value::<Vec<RobotPose>>(value.clone()) {
        return ContextValue::PoseArray(poses);
    }
    ContextValue::RawJson(value.clone())
}

/// 识别 context 中一个值的类型
fn classify_context_value(key: &str, value: &serde_json::Value) -> ContextValue {
    match value {
        serde_json::Value::Null => ContextValue::Null,
        serde_json::Value::Bool(b) => ContextValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ContextValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                ContextValue::Float(f)
            } else {
                ContextValue::RawJson(value.clone())
            }
        }
        serde_json::Value::String(s) => classify_string_value(s),
        serde_json::Value::Array(arr) => classify_array_value(key, value, arr),
        _ => ContextValue::RawJson(value.clone()),
    }
}

// ============================================================
// 解析与序列化
// ============================================================

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

    Ok(TaskGraphData {
        map_id,
        task_id,
        context_fields,
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

/// 将 ContextValue 序列化为 JSON 值
fn serialize_context_value(value: &ContextValue) -> Result<serde_json::Value, serde_json::Error> {
    Ok(match value {
        ContextValue::Pose(pose) => serde_json::Value::String(serde_json::to_string(pose)?),
        ContextValue::Bool(b) => serde_json::Value::Bool(*b),
        ContextValue::Integer(i) => serde_json::json!(*i),
        ContextValue::Float(f) => serde_json::json!(*f),
        ContextValue::NumericArray(arr) => {
            let json_arr: Vec<serde_json::Value> =
                arr.iter().map(|&v| numeric_to_json_value(v)).collect();
            serde_json::Value::String(serde_json::to_string(&json_arr)?)
        }
        ContextValue::NumericArray2D(arr2d) => {
            let json_arr: Vec<Vec<serde_json::Value>> = arr2d
                .iter()
                .map(|row| row.iter().map(|&v| numeric_to_json_value(v)).collect())
                .collect();
            serde_json::Value::String(serde_json::to_string(&json_arr)?)
        }
        ContextValue::JointTrajectory(traj) => serde_json::to_value(traj)?,
        ContextValue::PoseArray(poses) => serde_json::to_value(poses)?,
        ContextValue::Text(s) => serde_json::Value::String(s.clone()),
        ContextValue::Null => serde_json::Value::Null,
        ContextValue::RawJson(v) => v.clone(),
    })
}

/// 将编辑后的数据合并回原始 JSON 并输出格式化字符串
///
/// 修改 map_id、task_id，并将所有 context 字段重新序列化写回。
pub fn serialize_task_graph(data: &TaskGraphData) -> Result<String, serde_json::Error> {
    let mut json = data.raw_json.clone();

    // 更新顶层字段
    json["map_id"] = serde_json::Value::String(data.map_id.clone());
    json["task_id"] = serde_json::Value::String(data.task_id.clone());

    // 更新 context 中的所有字段
    if let Some(context) = json
        .get_mut("config")
        .and_then(|c| c.get_mut("context"))
        .and_then(|c| c.as_object_mut())
    {
        for field in &data.context_fields {
            let value = serialize_context_value(&field.value)?;
            context.insert(field.key.clone(), value);
        }
    }

    serde_json::to_string_pretty(&json)
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
            ros_domain_id: "11".into(),
            remote_dir: default_remote_dir(),
        }
    }
}

/// 配置文件路径: ~/.config/task-graph-editor/login.json
fn config_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
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
                ]
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
        // angles_2d, heights, jt1_traj, null_val, pick_poses,
        // point_a, some_bool, some_float, some_number, some_string
        assert_eq!(data.context_fields.len(), 10);
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
            ContextValue::NumericArray(arr) => {
                assert_eq!(arr.len(), 3);
                assert!((arr[0] - 0.01).abs() < f64::EPSILON);
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
            ContextValue::NumericArray2D(arr) => {
                assert_eq!(arr.len(), 2);
                assert_eq!(arr[0].len(), 2);
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
