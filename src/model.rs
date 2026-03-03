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

/// 从 context 中提取的一个位姿字段
#[derive(Debug, Clone)]
pub struct PoseField {
    /// context 中的 key 名（如 "after_leak_point"、"mark_leak_point"）
    pub key: String,
    /// 解析后的机器人位姿
    pub pose: RobotPose,
}

/// GUI 编辑用的任务图数据
#[derive(Debug, Clone)]
pub struct TaskGraphData {
    pub map_id: String,
    pub task_id: String,
    /// 所有包含位姿信息的 context 字段（按 key 排序）
    pub pose_fields: Vec<PoseField>,
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

/// 从 JSON 字符串解析任务图数据
///
/// 提取 map_id、task_id，并遍历 config.context 中所有字符串值，
/// 尝试将其解析为 RobotPose（包含 chassis_pose/head_pose/waist_pose）。
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

    let mut pose_fields = Vec::new();

    // 遍历 config.context，找出所有位姿字符串字段
    if let Some(context) = raw
        .get("config")
        .and_then(|c| c.get("context"))
        .and_then(|c| c.as_object())
    {
        // 用 BTreeMap 排序保证顺序稳定
        let sorted: BTreeMap<_, _> = context.iter().collect();
        for (key, value) in sorted {
            if let Some(s) = value.as_str()
                && let Ok(pose) = serde_json::from_str::<RobotPose>(s)
            {
                pose_fields.push(PoseField {
                    key: key.clone(),
                    pose,
                });
            }
        }
    }

    Ok(TaskGraphData {
        map_id,
        task_id,
        pose_fields,
        raw_json: raw,
    })
}

/// 将编辑后的数据合并回原始 JSON 并输出格式化字符串
///
/// 修改 map_id、task_id，并将位姿字段重新序列化为字符串写回 context。
pub fn serialize_task_graph(data: &TaskGraphData) -> Result<String, serde_json::Error> {
    let mut json = data.raw_json.clone();

    // 更新顶层字段
    json["map_id"] = serde_json::Value::String(data.map_id.clone());
    json["task_id"] = serde_json::Value::String(data.task_id.clone());

    // 更新 context 中的位姿字段（重新序列化为字符串）
    if let Some(context) = json
        .get_mut("config")
        .and_then(|c| c.get_mut("context"))
        .and_then(|c| c.as_object_mut())
    {
        for field in &data.pose_fields {
            let pose_str = serde_json::to_string(&field.pose)?;
            context.insert(field.key.clone(), serde_json::Value::String(pose_str));
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
                "some_string": "not_a_pose"
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
    fn test_parse_extracts_pose_fields() {
        let data = parse_task_graph(TEST_JSON).unwrap();
        assert_eq!(data.pose_fields.len(), 1);
        assert_eq!(data.pose_fields[0].key, "point_a");
        assert!((data.pose_fields[0].pose.chassis_pose.position.x - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_roundtrip_preserves_data() {
        let mut data = parse_task_graph(TEST_JSON).unwrap();
        data.map_id = "new-map".into();
        data.pose_fields[0].pose.chassis_pose.position.x = 99.0;

        let output = serialize_task_graph(&data).unwrap();
        let reparsed = parse_task_graph(&output).unwrap();

        assert_eq!(reparsed.map_id, "new-map");
        assert!((reparsed.pose_fields[0].pose.chassis_pose.position.x - 99.0).abs() < f64::EPSILON);
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
}
