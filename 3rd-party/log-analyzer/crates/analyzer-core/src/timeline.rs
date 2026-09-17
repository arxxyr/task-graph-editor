//! 统一的时间线数据结构
//!
//! 所有分析器插件都输出标准化的时间线数据，方便后续合并和可视化

use abi_stable::{StableAbi, std_types::*};
use serde::{Deserialize, Serialize};

/// 泳道类型（用于甘特图分层显示）
#[derive(Debug, Clone, Serialize, Deserialize, StableAbi, PartialEq, Eq, Hash)]
#[repr(C)]
pub enum Track {
    /// 轮次标记（最顶层）
    RoundMarker,
    /// 导航
    Navigation,
    /// 机械臂
    Arm,
    /// 头部控制
    Head,
    /// 腰部控制
    Waist,
    /// 自定义泳道（用于扩展）
    Custom(RString),
}

/// 事件状态
#[derive(Debug, Clone, Serialize, Deserialize, StableAbi, PartialEq)]
#[repr(C)]
pub enum EventStatus {
    /// 成功
    Success,
    /// 失败
    Failed,
    /// 进行中
    InProgress,
    /// 取消
    Cancelled,
}

/// 时间线事件
#[derive(Debug, Clone, Serialize, Deserialize, StableAbi)]
#[repr(C)]
pub struct TimelineEvent {
    /// 事件唯一 ID
    pub id: RString,

    /// 所属泳道
    pub track: Track,

    /// 事件名称（显示在甘特图上）
    pub name: RString,

    /// 开始时间（Unix 时间戳，秒）
    pub start_time: f64,

    /// 结束时间（可选）
    pub end_time: ROption<f64>,

    /// 事件状态
    pub status: EventStatus,

    /// 数据来源（如 "master_control", "external_arm"）
    pub source: RString,

    /// 父事件 ID（用于层级关系，如某个动作属于某个导航）
    pub parent_id: ROption<RString>,

    /// 元数据（JSON 字符串，存储额外信息）
    pub metadata: ROption<RString>,

    /// 颜色提示（可选，用于甘特图，格式: "#RRGGBB"）
    pub color_hint: ROption<RString>,
}

impl TimelineEvent {
    /// 计算持续时间（秒）
    pub fn duration(&self) -> Option<f64> {
        self.end_time
            .as_ref()
            .map(|end| *end - self.start_time)
            .into()
    }

    /// 是否已完成
    pub fn is_completed(&self) -> bool {
        self.end_time.is_some()
    }
}

/// 时间线（来自单个日志文件）
#[derive(Debug, Clone, Serialize, Deserialize, StableAbi)]
#[repr(C)]
pub struct Timeline {
    /// 时间线名称（如 "master_control", "external_arm"）
    pub name: RString,

    /// 来源文件路径
    pub source_file: RString,

    /// 日志开始时间（Unix 时间戳）
    pub log_start_time: f64,

    /// 日志结束时间（Unix 时间戳）
    pub log_end_time: f64,

    /// 所有事件
    pub events: RVec<TimelineEvent>,

    /// 是否为主时间轴（用于时间对齐）
    pub is_primary: bool,

    /// 元数据（JSON 字符串）
    pub metadata: ROption<RString>,
}
