//! 数值控件与 `TaskGraphData` 的双向绑定
//!
//! Bevy 是保留模式 UI：控件是实体，不像 egui 那样直接持有 `&mut f64`。
//! 每个数值控件挂一个 [`ValueBinding`]，描述"我编辑的是数据里的哪一个数"，
//! 控件发出 `ValueChange` 时由统一的 observer 按绑定回写。
//!
//! 本模块只做纯数据寻址，不依赖任何 UI 类型，便于单元测试。

use bevy::prelude::*;

use crate::model::{
    ContextValue, Pose, RobotPose, TaskGraphData, field_at_path, field_at_path_mut,
};

/// 一个可选中、可从 ROS2 回填的位姿，支持独立字段与位姿数组元素。
///
/// 字段路径只描述嵌套分组，数组下标单独保存，不能混入字段路径。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoseTarget {
    pub field_path: Vec<usize>,
    pub array_index: Option<usize>,
}

impl PoseTarget {
    /// 独立位姿字段。
    pub fn field(field_path: Vec<usize>) -> Self {
        Self {
            field_path,
            array_index: None,
        }
    }

    /// 位姿数组中的一个元素。
    pub fn array_element(field_path: Vec<usize>, array_index: usize) -> Self {
        Self {
            field_path,
            array_index: Some(array_index),
        }
    }

    /// 类型不匹配、字段不存在或数组越界时拒绝定位。
    pub fn pose<'a>(&self, data: &'a TaskGraphData) -> Option<&'a RobotPose> {
        let field = field_at_path(&data.context_fields, &self.field_path)?;
        match (&field.value, self.array_index) {
            (ContextValue::Pose(pose), None) => Some(pose),
            (ContextValue::PoseArray(poses), Some(index)) => poses.get(index),
            _ => None,
        }
    }

    /// 使用与只读定位相同的规则，避免选中验证与回填目标不一致。
    pub fn pose_mut<'a>(&self, data: &'a mut TaskGraphData) -> Option<&'a mut RobotPose> {
        let field = field_at_path_mut(&mut data.context_fields, &self.field_path)?;
        match (&mut field.value, self.array_index) {
            (ContextValue::Pose(pose), None) => Some(pose),
            (ContextValue::PoseArray(poses), Some(index)) => poses.get_mut(index),
            _ => None,
        }
    }

    /// 已验证目标的可读路径，数组目标保留具体下标。
    pub fn display_path(&self, data: &TaskGraphData) -> String {
        let path = crate::model::key_path_string(&data.context_fields, &self.field_path);
        match self.array_index {
            Some(index) => format!("{path}[{index}]"),
            None => path,
        }
    }
}

/// 机器人部位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PosePart {
    /// 底盘
    #[default]
    Chassis,
    /// 头部
    Head,
    /// 腰部
    Waist,
}

impl PosePart {
    /// 中文显示名（含英文原名，与旧版 UI 一致）
    pub fn label(self) -> &'static str {
        match self {
            PosePart::Chassis => "底盘 (chassis)",
            PosePart::Head => "头部 (head)",
            PosePart::Waist => "腰部 (waist)",
        }
    }

    /// 短名（位姿数组等紧凑场景使用）
    pub fn short_label(self) -> &'static str {
        match self {
            PosePart::Chassis => "底盘",
            PosePart::Head => "头部",
            PosePart::Waist => "腰部",
        }
    }

    /// 取出对应部位的可变引用
    pub fn get_mut(self, pose: &mut RobotPose) -> &mut Pose {
        match self {
            PosePart::Chassis => &mut pose.chassis_pose,
            PosePart::Head => &mut pose.head_pose,
            PosePart::Waist => &mut pose.waist_pose,
        }
    }

    /// 取出对应部位的只读引用
    pub fn get(self, pose: &RobotPose) -> &Pose {
        match self {
            PosePart::Chassis => &pose.chassis_pose,
            PosePart::Head => &pose.head_pose,
            PosePart::Waist => &pose.waist_pose,
        }
    }

    /// 三个部位的固定顺序
    pub const ALL: [PosePart; 3] = [PosePart::Chassis, PosePart::Head, PosePart::Waist];
}

/// 位姿的单个数值分量
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PoseComp {
    /// 位置 x
    #[default]
    PosX,
    /// 位置 y
    PosY,
    /// 位置 z
    PosZ,
    /// 姿态 w
    OriW,
    /// 姿态 x
    OriX,
    /// 姿态 y
    OriY,
    /// 姿态 z
    OriZ,
}

impl PoseComp {
    /// 控件标签
    pub fn label(self) -> &'static str {
        match self {
            PoseComp::PosX | PoseComp::OriX => "x",
            PoseComp::PosY | PoseComp::OriY => "y",
            PoseComp::PosZ | PoseComp::OriZ => "z",
            PoseComp::OriW => "w",
        }
    }

    /// 取出分量的可变引用
    pub fn get_mut(self, pose: &mut Pose) -> &mut f64 {
        match self {
            PoseComp::PosX => &mut pose.position.x,
            PoseComp::PosY => &mut pose.position.y,
            PoseComp::PosZ => &mut pose.position.z,
            PoseComp::OriW => &mut pose.orientation.w,
            PoseComp::OriX => &mut pose.orientation.x,
            PoseComp::OriY => &mut pose.orientation.y,
            PoseComp::OriZ => &mut pose.orientation.z,
        }
    }

    /// 取出分量的值
    pub fn get(self, pose: &Pose) -> f64 {
        match self {
            PoseComp::PosX => pose.position.x,
            PoseComp::PosY => pose.position.y,
            PoseComp::PosZ => pose.position.z,
            PoseComp::OriW => pose.orientation.w,
            PoseComp::OriX => pose.orientation.x,
            PoseComp::OriY => pose.orientation.y,
            PoseComp::OriZ => pose.orientation.z,
        }
    }

    /// 位置三分量
    pub const POSITION: [PoseComp; 3] = [PoseComp::PosX, PoseComp::PosY, PoseComp::PosZ];

    /// 姿态四分量（w 在前，与 JSON 字段顺序一致）
    pub const ORIENTATION: [PoseComp; 4] = [
        PoseComp::OriW,
        PoseComp::OriX,
        PoseComp::OriY,
        PoseComp::OriZ,
    ];
}

/// 字段内部的具体数值槽位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ValueSlot {
    /// 标量字段本身（Integer / Float）
    #[default]
    Scalar,
    /// 位姿字段的某个部位分量
    Pose(PosePart, PoseComp),
    /// 一维数值数组的第 n 个元素
    Array1D(usize),
    /// 二维数值数组的 `[row][col]`
    Array2D(usize, usize),
    /// 关节轨迹第 n 个点的时间戳
    TrajTime(usize),
    /// 关节轨迹第 n 个点的第 j 个关节
    TrajJoint(usize, usize),
    /// 位姿数组第 n 个位姿的部位分量
    PoseArray(usize, PosePart, PoseComp),
}

/// 数值控件到数据位置的绑定
///
/// `field_path` 是索引路径：首元素索引顶层 `context_fields`，
/// 后续元素依次下钻嵌套分组（见 [`crate::model::field_at_path`]）。
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub struct ValueBinding {
    /// context 字段索引路径
    pub field_path: Vec<usize>,
    /// 字段内部槽位
    pub slot: ValueSlot,
}

impl ValueBinding {
    /// 构造绑定
    pub fn new(field_path: Vec<usize>, slot: ValueSlot) -> Self {
        Self { field_path, slot }
    }
}

/// 布尔控件绑定（Bool 字段专用，无需槽位）
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub struct BoolBinding {
    /// context 字段索引路径
    pub field_path: Vec<usize>,
}

/// 文本控件绑定（Text 字段专用）
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub struct TextBinding {
    /// context 字段索引路径
    pub field_path: Vec<usize>,
}

/// 按槽位在 `ContextValue` 中定位可变的 f64
///
/// 槽位与字段类型不匹配、或索引越界时返回 `None`。
fn slot_in_value_mut(value: &mut ContextValue, slot: ValueSlot) -> Option<&mut f64> {
    match (value, slot) {
        (ContextValue::Float(f), ValueSlot::Scalar) => Some(f),
        (ContextValue::Pose(pose), ValueSlot::Pose(part, comp)) => {
            Some(comp.get_mut(part.get_mut(pose)))
        }
        (ContextValue::NumericArray { values, .. }, ValueSlot::Array1D(i)) => values.get_mut(i),
        (ContextValue::NumericArray2D { rows, .. }, ValueSlot::Array2D(row, col)) => {
            rows.get_mut(row)?.get_mut(col)
        }
        (ContextValue::JointTrajectory(traj), ValueSlot::TrajTime(i)) => {
            Some(&mut traj.get_mut(i)?.time_from_start)
        }
        (ContextValue::JointTrajectory(traj), ValueSlot::TrajJoint(i, j)) => {
            traj.get_mut(i)?.positions.get_mut(j)
        }
        (ContextValue::PoseArray(poses), ValueSlot::PoseArray(i, part, comp)) => {
            Some(comp.get_mut(part.get_mut(poses.get_mut(i)?)))
        }
        _ => None,
    }
}

/// 按槽位在 `ContextValue` 中读取 f64
fn slot_in_value(value: &ContextValue, slot: ValueSlot) -> Option<f64> {
    match (value, slot) {
        (ContextValue::Float(f), ValueSlot::Scalar) => Some(*f),
        (ContextValue::Pose(pose), ValueSlot::Pose(part, comp)) => Some(comp.get(part.get(pose))),
        (ContextValue::NumericArray { values, .. }, ValueSlot::Array1D(i)) => {
            values.get(i).copied()
        }
        (ContextValue::NumericArray2D { rows, .. }, ValueSlot::Array2D(row, col)) => {
            rows.get(row)?.get(col).copied()
        }
        (ContextValue::JointTrajectory(traj), ValueSlot::TrajTime(i)) => {
            Some(traj.get(i)?.time_from_start)
        }
        (ContextValue::JointTrajectory(traj), ValueSlot::TrajJoint(i, j)) => {
            traj.get(i)?.positions.get(j).copied()
        }
        (ContextValue::PoseArray(poses), ValueSlot::PoseArray(i, part, comp)) => {
            Some(comp.get(part.get(poses.get(i)?)))
        }
        _ => None,
    }
}

/// 按绑定把有限 f64 写回数据；非有限值或定位失败返回 `false`。
pub fn apply_f64(data: &mut TaskGraphData, binding: &ValueBinding, value: f64) -> bool {
    if !value.is_finite() {
        return false;
    }
    let Some(field) = field_at_path_mut(&mut data.context_fields, &binding.field_path) else {
        return false;
    };
    match slot_in_value_mut(&mut field.value, binding.slot) {
        Some(target) => {
            *target = value;
            true
        }
        None => false,
    }
}

/// 按绑定把 i64 写回数据（Integer 字段专用）；定位失败返回 `false`
pub fn apply_i64(data: &mut TaskGraphData, binding: &ValueBinding, value: i64) -> bool {
    let Some(field) = field_at_path_mut(&mut data.context_fields, &binding.field_path) else {
        return false;
    };
    match (&mut field.value, binding.slot) {
        (ContextValue::Integer(target), ValueSlot::Scalar) => {
            *target = value;
            true
        }
        _ => false,
    }
}

/// 按绑定把布尔值写回数据；定位失败返回 `false`
pub fn apply_bool(data: &mut TaskGraphData, binding: &BoolBinding, value: bool) -> bool {
    let Some(field) = field_at_path_mut(&mut data.context_fields, &binding.field_path) else {
        return false;
    };
    match &mut field.value {
        ContextValue::Bool(target) => {
            *target = value;
            true
        }
        _ => false,
    }
}

/// 按绑定把文本写回数据；定位失败返回 `false`
pub fn apply_text(data: &mut TaskGraphData, binding: &TextBinding, value: &str) -> bool {
    let Some(field) = field_at_path_mut(&mut data.context_fields, &binding.field_path) else {
        return false;
    };
    match &mut field.value {
        ContextValue::Text(target) => {
            value.clone_into(target);
            true
        }
        _ => false,
    }
}

/// 按绑定读取当前 f64 值（ROS2 回填后刷新控件显示用）
pub fn read_f64(data: &TaskGraphData, binding: &ValueBinding) -> Option<f64> {
    let field = crate::model::field_at_path(&data.context_fields, &binding.field_path)?;
    slot_in_value(&field.value, binding.slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ContextField, TrajectoryPoint};

    /// 构造一份含各种类型的测试数据
    fn sample_data() -> TaskGraphData {
        TaskGraphData {
            map_id: "m".into(),
            task_id: "t".into(),
            context_fields: vec![
                ContextField {
                    key: "target_pose".into(),
                    value: ContextValue::Pose(RobotPose::default()),
                },
                ContextField {
                    key: "speed".into(),
                    value: ContextValue::Float(1.5),
                },
                ContextField {
                    key: "count".into(),
                    value: ContextValue::Integer(3),
                },
                ContextField {
                    key: "enabled".into(),
                    value: ContextValue::Bool(false),
                },
                ContextField {
                    key: "note".into(),
                    value: ContextValue::Text("hello".into()),
                },
                ContextField {
                    key: "offsets".into(),
                    value: ContextValue::NumericArray {
                        values: vec![0.1, 0.2, 0.3],
                        stringified: true,
                    },
                },
                ContextField {
                    key: "grid".into(),
                    value: ContextValue::NumericArray2D {
                        rows: vec![vec![1.0, 2.0], vec![3.0, 4.0]],
                        stringified: true,
                    },
                },
                ContextField {
                    key: "traj".into(),
                    value: ContextValue::JointTrajectory(vec![TrajectoryPoint {
                        positions: vec![0.0, 1.0],
                        time_from_start: 0.5,
                    }]),
                },
                ContextField {
                    key: "poses".into(),
                    value: ContextValue::PoseArray(vec![RobotPose::default()]),
                },
                ContextField {
                    key: "stations".into(),
                    value: ContextValue::NestedGroup(vec![ContextField {
                        key: "station_1".into(),
                        value: ContextValue::NestedGroup(vec![ContextField {
                            key: "slot_pose".into(),
                            value: ContextValue::Pose(RobotPose::default()),
                        }]),
                    }]),
                },
            ],
            graph: crate::model::SubGraph::default(),
            graph_edit: crate::model::GraphDocument::default(),
            raw_json: serde_json::json!({}),
        }
    }

    #[test]
    fn 位姿目标统一定位顶层嵌套字段和数组元素() {
        let mut data = sample_data();
        let ContextValue::NestedGroup(fields) =
            &mut field_at_path_mut(&mut data.context_fields, &[9, 0])
                .unwrap()
                .value
        else {
            panic!("应为嵌套分组");
        };
        fields.push(ContextField {
            key: "pick_poses".into(),
            value: ContextValue::PoseArray(vec![RobotPose::default(); 2]),
        });
        for (target, label) in [
            (PoseTarget::field(vec![0]), "target_pose"),
            (PoseTarget::array_element(vec![8], 0), "poses[0]"),
            (
                PoseTarget::field(vec![9, 0, 0]),
                "stations.station_1.slot_pose",
            ),
            (
                PoseTarget::array_element(vec![9, 0, 1], 1),
                "stations.station_1.pick_poses[1]",
            ),
        ] {
            assert_eq!(target.display_path(&data), label);
            assert_eq!(target.pose(&data).unwrap(), &RobotPose::default());
            target.pose_mut(&mut data).unwrap().chassis_pose.position.x = 1.2345678901234567;
            assert_eq!(
                target.pose(&data).unwrap().chassis_pose.position.x,
                1.2345678901234567
            );
        }
        let first = PoseTarget::array_element(vec![9, 0, 1], 0);
        assert_eq!(first.pose(&data).unwrap(), &RobotPose::default());
    }

    #[test]
    fn 位姿目标拒绝缺失路径类型不匹配及越界下标() {
        let mut data = sample_data();
        let original = data.context_fields.clone();
        for target in [
            PoseTarget::default(),
            PoseTarget::field(vec![usize::MAX]),
            PoseTarget::field(vec![1]),
            PoseTarget::field(vec![8]),
            PoseTarget::array_element(vec![0], 0),
            PoseTarget::array_element(vec![1], 0),
            PoseTarget::array_element(vec![8], 1),
            PoseTarget::array_element(vec![9, 0, 0], 0),
            PoseTarget::field(vec![8, 0]),
        ] {
            assert!(target.pose(&data).is_none(), "{target:?}");
            assert!(target.pose_mut(&mut data).is_none(), "{target:?}");
        }
        assert_eq!(data.context_fields, original);
    }

    #[test]
    fn 位姿分量写回() {
        let mut data = sample_data();
        let binding =
            ValueBinding::new(vec![0], ValueSlot::Pose(PosePart::Chassis, PoseComp::PosX));
        assert!(apply_f64(&mut data, &binding, 3.25));
        let ContextValue::Pose(pose) = &data.context_fields[0].value else {
            panic!("类型应为 Pose");
        };
        assert_eq!(pose.chassis_pose.position.x, 3.25);
    }

    #[test]
    fn 位姿七个分量互不串扰() {
        let mut data = sample_data();
        // 依次写入 7 个互不相同的值
        let comps = [
            (PoseComp::PosX, 1.0),
            (PoseComp::PosY, 2.0),
            (PoseComp::PosZ, 3.0),
            (PoseComp::OriW, 4.0),
            (PoseComp::OriX, 5.0),
            (PoseComp::OriY, 6.0),
            (PoseComp::OriZ, 7.0),
        ];
        for (comp, v) in comps {
            let binding = ValueBinding::new(vec![0], ValueSlot::Pose(PosePart::Head, comp));
            assert!(apply_f64(&mut data, &binding, v));
        }
        let ContextValue::Pose(pose) = &data.context_fields[0].value else {
            panic!("类型应为 Pose");
        };
        assert_eq!(pose.head_pose.position.x, 1.0);
        assert_eq!(pose.head_pose.position.y, 2.0);
        assert_eq!(pose.head_pose.position.z, 3.0);
        assert_eq!(pose.head_pose.orientation.w, 4.0);
        assert_eq!(pose.head_pose.orientation.x, 5.0);
        assert_eq!(pose.head_pose.orientation.y, 6.0);
        assert_eq!(pose.head_pose.orientation.z, 7.0);
        // 底盘与腰部不受影响
        assert_eq!(pose.chassis_pose.position.x, 0.0);
        assert_eq!(pose.waist_pose.orientation.w, 1.0);
    }

    #[test]
    fn 标量与整数写回() {
        let mut data = sample_data();
        let float_binding = ValueBinding::new(vec![1], ValueSlot::Scalar);
        assert!(apply_f64(&mut data, &float_binding, 9.75));
        assert!(matches!(
            data.context_fields[1].value,
            ContextValue::Float(v) if v == 9.75
        ));

        let int_binding = ValueBinding::new(vec![2], ValueSlot::Scalar);
        assert!(apply_i64(&mut data, &int_binding, 42));
        assert!(matches!(
            data.context_fields[2].value,
            ContextValue::Integer(42)
        ));
    }

    #[test]
    fn 所有浮点槽位拒绝非有限值并保持原数据() {
        let mut data = sample_data();
        let bindings = [
            ValueBinding::new(vec![1], ValueSlot::Scalar),
            ValueBinding::new(vec![0], ValueSlot::Pose(PosePart::Chassis, PoseComp::PosX)),
            ValueBinding::new(vec![5], ValueSlot::Array1D(0)),
            ValueBinding::new(vec![6], ValueSlot::Array2D(0, 0)),
            ValueBinding::new(vec![7], ValueSlot::TrajTime(0)),
            ValueBinding::new(vec![7], ValueSlot::TrajJoint(0, 0)),
            ValueBinding::new(
                vec![8],
                ValueSlot::PoseArray(0, PosePart::Head, PoseComp::OriW),
            ),
            ValueBinding::new(
                vec![9, 0, 0],
                ValueSlot::Pose(PosePart::Waist, PoseComp::PosZ),
            ),
        ];
        for binding in &bindings {
            let original = read_f64(&data, binding);
            assert!(original.is_some());
            for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert!(!apply_f64(&mut data, binding, invalid));
                assert_eq!(read_f64(&data, binding), original);
            }
        }
    }

    #[test]
    fn 整数字段不接受浮点写回() {
        let mut data = sample_data();
        // Integer 字段用 apply_f64 应当拒绝，避免把 3 悄悄写成 3.0
        let binding = ValueBinding::new(vec![2], ValueSlot::Scalar);
        assert!(!apply_f64(&mut data, &binding, 7.5));
        assert!(matches!(
            data.context_fields[2].value,
            ContextValue::Integer(3)
        ));
    }

    #[test]
    fn 布尔与文本写回() {
        let mut data = sample_data();
        let bool_binding = BoolBinding {
            field_path: vec![3],
        };
        assert!(apply_bool(&mut data, &bool_binding, true));
        assert!(matches!(
            data.context_fields[3].value,
            ContextValue::Bool(true)
        ));

        let text_binding = TextBinding {
            field_path: vec![4],
        };
        assert!(apply_text(&mut data, &text_binding, "world"));
        assert!(matches!(
            &data.context_fields[4].value,
            ContextValue::Text(s) if s == "world"
        ));
    }

    #[test]
    fn 一维与二维数组写回() {
        let mut data = sample_data();
        let b1 = ValueBinding::new(vec![5], ValueSlot::Array1D(1));
        assert!(apply_f64(&mut data, &b1, 8.5));
        let ContextValue::NumericArray { values: arr, .. } = &data.context_fields[5].value else {
            panic!("类型应为 NumericArray");
        };
        assert_eq!(arr, &[0.1, 8.5, 0.3]);

        let b2 = ValueBinding::new(vec![6], ValueSlot::Array2D(1, 0));
        assert!(apply_f64(&mut data, &b2, -1.0));
        let ContextValue::NumericArray2D { rows: arr2d, .. } = &data.context_fields[6].value else {
            panic!("类型应为 NumericArray2D");
        };
        assert_eq!(arr2d[1], vec![-1.0, 4.0]);
    }

    #[test]
    fn 轨迹时间与关节写回() {
        let mut data = sample_data();
        let t = ValueBinding::new(vec![7], ValueSlot::TrajTime(0));
        assert!(apply_f64(&mut data, &t, 2.5));
        let j = ValueBinding::new(vec![7], ValueSlot::TrajJoint(0, 1));
        assert!(apply_f64(&mut data, &j, -0.75));

        let ContextValue::JointTrajectory(traj) = &data.context_fields[7].value else {
            panic!("类型应为 JointTrajectory");
        };
        assert_eq!(traj[0].time_from_start, 2.5);
        assert_eq!(traj[0].positions, vec![0.0, -0.75]);
    }

    #[test]
    fn 位姿数组写回() {
        let mut data = sample_data();
        let b = ValueBinding::new(
            vec![8],
            ValueSlot::PoseArray(0, PosePart::Waist, PoseComp::PosY),
        );
        assert!(apply_f64(&mut data, &b, 1.25));
        let ContextValue::PoseArray(poses) = &data.context_fields[8].value else {
            panic!("类型应为 PoseArray");
        };
        assert_eq!(poses[0].waist_pose.position.y, 1.25);
    }

    /// 逐分量核验实际编辑入口；所有写入只发生在内存副本中。
    fn 核验字符串位姿数组绑定(content: &str, key: &str) -> usize {
        use crate::model::{parse_task_graph, serialize_task_graph};
        use serde_json::{Value, json};

        let original: Value = serde_json::from_str(content).unwrap();
        let data = parse_task_graph(content).unwrap();
        let field_index = data
            .context_fields
            .iter()
            .position(|f| f.key == key)
            .unwrap();
        let ContextValue::PoseArray(poses) = &data.context_fields[field_index].value else {
            panic!("{key} 必须识别成可编辑的位姿数组");
        };
        let raw = original["config"]["context"][key].as_array().unwrap();
        assert!(!raw.is_empty());
        assert_eq!(poses.len(), raw.len());
        let unchanged: Value = serde_json::from_str(&serialize_task_graph(&data).unwrap()).unwrap();
        assert_eq!(
            unchanged, original,
            "未编辑时整份文件及内层字符串必须保持原样"
        );

        let parts = [
            (PosePart::Chassis, "chassis_pose"),
            (PosePart::Head, "head_pose"),
            (PosePart::Waist, "waist_pose"),
        ];
        let components = [
            (PoseComp::PosX, "position", "x"),
            (PoseComp::PosY, "position", "y"),
            (PoseComp::PosZ, "position", "z"),
            (PoseComp::OriW, "orientation", "w"),
            (PoseComp::OriX, "orientation", "x"),
            (PoseComp::OriY, "orientation", "y"),
            (PoseComp::OriZ, "orientation", "z"),
        ];
        let mut checked = 0;
        for (index, raw_pose) in raw.iter().enumerate() {
            let source: Value = serde_json::from_str(raw_pose.as_str().unwrap()).unwrap();
            for (part, part_key) in parts {
                for (comp, group_key, comp_key) in components {
                    let binding = ValueBinding::new(
                        vec![field_index],
                        ValueSlot::PoseArray(index, part, comp),
                    );
                    let expected = source[part_key][group_key][comp_key].as_f64().unwrap();
                    assert_eq!(
                        read_f64(&data, &binding).unwrap().to_bits(),
                        expected.to_bits()
                    );

                    let replacement = match expected == 0.125 {
                        true => 0.375,
                        false => 0.125,
                    };
                    let mut edited = data.clone();
                    assert!(apply_f64(&mut edited, &binding, replacement));
                    let output = serialize_task_graph(&edited).unwrap();
                    let reparsed = parse_task_graph(&output).unwrap();
                    assert_eq!(read_f64(&reparsed, &binding), Some(replacement));

                    let mut saved: Value = serde_json::from_str(&output).unwrap();
                    let saved_pose = &mut saved["config"]["context"][key][index];
                    let decoded: Value =
                        serde_json::from_str(saved_pose.as_str().unwrap()).unwrap();
                    let mut expected_pose = source.clone();
                    expected_pose[part_key][group_key][comp_key] = json!(replacement);
                    assert_eq!(
                        decoded, expected_pose,
                        "编辑一个分量不能改变其他分量或扩展字段"
                    );
                    *saved_pose = raw_pose.clone();
                    assert_eq!(
                        saved, original,
                        "其他元素、context 字段和流程图必须原样保留"
                    );
                    checked += 1;
                }
            }
        }
        checked
    }

    #[test]
    fn 字符串位姿数组通过数值绑定编辑后保留原格式() {
        use serde_json::json;

        let mut pose = serde_json::to_value(RobotPose::default()).unwrap();
        pose["chassis_pose"]["position"]["x"] = json!(0.9216510910864573);
        pose["chassis_pose"]["position"]["accuracy"] = json!(0.01);
        pose["vendor"] = json!({"frame": "map"});
        let content = json!({
            "map_id": "m", "task_id": "t",
            "config": {"context": {
                "pick_poses": [pose.to_string(), serde_json::to_string_pretty(&pose).unwrap()],
                "enabled": false
            }, "nodes": [{"id": "保留流程"}]}
        })
        .to_string();
        assert_eq!(核验字符串位姿数组绑定(&content, "pick_poses"), 42);
    }

    #[test]
    #[ignore = "需设置 TASK_GRAPH_REAL_FILE，可用 TGE_POSE_ARRAY_FIELD 指定位姿数组字段"]
    fn 真实文件位姿数组解析与绑定往返() {
        let path = std::env::var("TASK_GRAPH_REAL_FILE").expect("需要 TASK_GRAPH_REAL_FILE");
        let key = std::env::var("TGE_POSE_ARRAY_FIELD").unwrap_or_else(|_| "pick_poses".into());
        let content = std::fs::read_to_string(path).expect("读取真实文件失败");
        let checked = 核验字符串位姿数组绑定(&content, &key);
        println!(
            "{key}：{} 个位姿、{checked} 个分量解析和独立编辑往返通过",
            checked / 21
        );
    }

    #[test]
    fn 嵌套分组下钻写回() {
        let mut data = sample_data();
        // stations(9) → station_1(0) → slot_pose(0)
        let b = ValueBinding::new(
            vec![9, 0, 0],
            ValueSlot::Pose(PosePart::Chassis, PoseComp::PosZ),
        );
        assert!(apply_f64(&mut data, &b, 0.42));
        assert_eq!(read_f64(&data, &b), Some(0.42));
    }

    #[test]
    fn 越界与类型不匹配安全返回() {
        let mut data = sample_data();
        // 字段索引越界
        let oob_field = ValueBinding::new(vec![99], ValueSlot::Scalar);
        assert!(!apply_f64(&mut data, &oob_field, 1.0));
        // 数组下标越界
        let oob_index = ValueBinding::new(vec![5], ValueSlot::Array1D(99));
        assert!(!apply_f64(&mut data, &oob_index, 1.0));
        // 槽位与字段类型不匹配
        let mismatch = ValueBinding::new(vec![1], ValueSlot::Array1D(0));
        assert!(!apply_f64(&mut data, &mismatch, 1.0));
        // 空路径
        let empty = ValueBinding::new(vec![], ValueSlot::Scalar);
        assert!(!apply_f64(&mut data, &empty, 1.0));
    }

    #[test]
    fn 读回值与写入一致_保持f64精度() {
        let mut data = sample_data();
        // ROS2 关节角是 9 位小数，f32 会丢精度，这里验证全程 f64
        let precise = 0.679999937_f64;
        let b = ValueBinding::new(vec![0], ValueSlot::Pose(PosePart::Waist, PoseComp::PosX));
        assert!(apply_f64(&mut data, &b, precise));
        assert_eq!(read_f64(&data, &b), Some(precise));
    }
}
