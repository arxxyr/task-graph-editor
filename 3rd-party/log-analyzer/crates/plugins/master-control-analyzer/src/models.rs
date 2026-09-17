//! 数据结构定义模块
//!
//! 本模块包含日志分析工具中使用的所有核心数据结构

use rust_i18n::t;
use serde::Serialize;

/// 日志行结构，包含时间戳和原始日志内容
#[derive(Debug, Clone)]
pub struct LogLine {
    /// Unix 时间戳（秒）
    pub timestamp: f64,
    /// 日志原始内容
    pub line: String,
}

/// 循环类型枚举
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleType {
    /// 初始循环（气密设备为空时执行）
    Initial,
    /// 常规循环（迭代执行的主循环，编号从1开始）
    Normal(u32),
    /// 最终循环（取出气密工件并放置）
    Final,
}

impl std::fmt::Display for CycleType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CycleType::Initial => write!(f, "{}", t!("cycle.initial")),
            CycleType::Normal(n) => write!(f, "{}", t!("cycle.normal", n = n)),
            CycleType::Final => write!(f, "{}", t!("cycle.final")),
        }
    }
}

/// 暂停事件
#[derive(Debug, Clone)]
pub struct PauseEvent {
    /// 暂停开始时间戳
    pub pause_ts: f64,
    /// 恢复时间戳
    pub resume_ts: Option<f64>,
}

impl PauseEvent {
    /// 计算暂停持续时间
    pub fn duration(&self) -> f64 {
        self.resume_ts
            .map(|resume| resume - self.pause_ts)
            .unwrap_or(0.0)
    }
}

/// 将若干时间区间合并为互不重叠的区间集合
///
/// 输入区间无需有序；空区间（end <= start）被丢弃。
/// 返回按起点升序、两两不相交的区间列表。
pub fn merge_intervals(mut intervals: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    intervals.retain(|(start, end)| end > start);
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        match merged.last_mut() {
            Some((_, last_end)) if start <= *last_end => {
                if end > *last_end {
                    *last_end = end;
                }
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// 计算暂停事件的净暂停时长（重叠区间只计一次）
///
/// 同一次暂停常被多种检测模式同时命中：失败暂停会同时产生
/// `TaskGraphExecutor: 节点 ... 失败` 与 `ROS2ActionAdapter[...] - 暂停` 两类日志，
/// 用户暂停会同时产生 `PauseTaskNode` 与 `TaskGraphExecutor: 用户请求暂停任务`。
/// 朴素逐事件求和会把同一段真实暂停扣除两次，导致轮次有效时长被扣成 0。
/// 未恢复（`resume_ts` 为 `None`）的事件视为零时长，不参与合并。
pub fn merged_pause_duration(events: &[PauseEvent]) -> f64 {
    let intervals = events
        .iter()
        .filter_map(|e| e.resume_ts.map(|resume| (e.pause_ts, resume)))
        .collect();
    merge_intervals(intervals)
        .iter()
        .map(|(start, end)| end - start)
        .sum()
}

/// 任务轮次
#[derive(Debug, Clone)]
pub struct Round {
    /// 轮次ID
    pub id: usize,
    /// 循环编号（如循环1、循环2等）- 保留用于向后兼容
    pub loop_number: Option<u32>,
    /// 循环类型（初始循环/常规循环/最终循环）
    pub cycle_type: CycleType,
    /// 层级索引（用于多层级场景）
    pub layer_index: u32,
    /// 开始时间戳
    pub start_ts: f64,
    /// 结束时间戳
    pub end_ts: Option<f64>,
    /// 初始姿态
    pub pose0: Option<String>,
    /// 目标姿态
    pub pose6: Option<String>,
    /// 暂停事件列表
    pub pause_events: Vec<PauseEvent>,
}

impl Round {
    /// 计算总暂停时间（重叠的暂停区间只计一次）
    pub fn total_pause_duration(&self) -> f64 {
        merged_pause_duration(&self.pause_events)
    }

    /// 计算有效持续时间（总时长减去暂停时间）
    pub fn effective_duration(&self) -> f64 {
        let total = self.end_ts.map(|end| end - self.start_ts).unwrap_or(0.0);
        (total - self.total_pause_duration()).max(0.0)
    }
}

/// 动作内部的子步骤
#[derive(Debug, Clone)]
pub struct SubStep {
    /// 子步骤名称
    pub name: String,
    /// 时间戳
    pub timestamp: f64,
}

/// 通用动作操作
#[derive(Debug, Clone)]
pub struct ActionOperation {
    /// 动作类型：arm, navigation, head, waist
    pub action_type: String,
    /// 动作代码
    pub action_code: Option<u32>,
    /// 动作标签
    pub label: String,
    /// 开始时间戳
    pub start_ts: Option<f64>,
    /// 结束时间戳
    pub end_ts: Option<f64>,
    /// 状态
    pub status: String,
    /// 子步骤列表
    pub sub_steps: Vec<SubStep>,
    /// 暂停事件列表
    pub pause_events: Vec<PauseEvent>,
}

impl ActionOperation {
    /// 计算总暂停时间（重叠的暂停区间只计一次）
    pub fn total_pause_duration(&self) -> f64 {
        merged_pause_duration(&self.pause_events)
    }

    /// 计算有效持续时间（总时长减去暂停时间）
    pub fn effective_duration(&self) -> f64 {
        match (self.start_ts, self.end_ts) {
            (Some(start), Some(end)) => (end - start - self.total_pause_duration()).max(0.0),
            _ => 0.0,
        }
    }
}

/// 导航流程
#[derive(Debug, Clone)]
pub struct NavigationFlow {
    /// 导航开始时间戳
    pub nav_start_ts: Option<f64>,
    /// 导航结束时间戳
    pub nav_end_ts: Option<f64>,
    /// 导航目标位置
    pub nav_target_pos: Option<String>,
    /// 导航目标姿态
    pub nav_target_ori: Option<String>,
    /// 导航状态
    pub nav_status: String,
    /// 导航子步骤
    pub nav_sub_steps: Vec<SubStep>,
    /// 所属轮次ID
    pub round_id: usize,
    /// 关联的其他动作操作
    pub operations: Vec<ActionOperation>,
}

// ============================================================================
// CSV 导出结构
// ============================================================================

/// CSV记录结构
#[derive(Debug, Serialize)]
pub struct CsvRecord {
    /// 轮次ID
    pub round_id: usize,
    /// 轮次开始相对时间（秒）
    pub round_start_rel_s: Option<f64>,
    /// 轮次结束相对时间（秒）
    pub round_end_rel_s: Option<f64>,
    /// 轮次持续时间（秒）
    pub round_duration_s: Option<f64>,
    /// 流程ID
    pub flow_id: usize,
    /// 步骤索引
    pub step_idx: usize,
    /// 步骤类型
    pub step_type: String,
    /// 导航目标位置
    pub nav_target_pos: Option<String>,
    /// 导航目标姿态
    pub nav_target_ori: Option<String>,
    /// 动作类型
    pub action_type: Option<String>,
    /// 动作代码
    pub action_code: Option<u32>,
    /// 动作标签
    pub action_label: Option<String>,
    /// 开始相对时间（秒）
    pub start_rel_s: Option<f64>,
    /// 结束相对时间（秒）
    pub end_rel_s: Option<f64>,
    /// 持续时间（秒）
    pub duration_s: Option<f64>,
    /// 状态
    pub status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pause(pause_ts: f64, resume_ts: f64) -> PauseEvent {
        PauseEvent {
            pause_ts,
            resume_ts: Some(resume_ts),
        }
    }

    #[test]
    fn merges_overlapping_intervals() {
        let merged = merge_intervals(vec![(0.0, 10.0), (5.0, 15.0), (20.0, 25.0)]);
        assert_eq!(merged, vec![(0.0, 15.0), (20.0, 25.0)]);
    }

    #[test]
    fn merges_unsorted_and_nested_intervals() {
        let merged = merge_intervals(vec![(20.0, 25.0), (0.0, 30.0), (5.0, 10.0)]);
        assert_eq!(merged, vec![(0.0, 30.0)]);
    }

    #[test]
    fn drops_empty_intervals() {
        let merged = merge_intervals(vec![(5.0, 5.0), (10.0, 3.0), (1.0, 2.0)]);
        assert_eq!(merged, vec![(1.0, 2.0)]);
    }

    #[test]
    fn counts_duplicate_pause_detections_once() {
        // 同一次暂停被两种模式重复记录：净暂停应为 100 秒而非 200 秒
        let events = vec![pause(1000.0, 1100.0), pause(1000.1, 1100.1)];
        assert!((merged_pause_duration(&events) - 100.1).abs() < 1e-9);
    }

    #[test]
    fn ignores_pause_without_resume() {
        let events = vec![
            pause(0.0, 10.0),
            PauseEvent {
                pause_ts: 50.0,
                resume_ts: None,
            },
        ];
        assert!((merged_pause_duration(&events) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn round_effective_duration_excludes_double_counted_pause() {
        // 轮次总时长 328.5 秒，其中一段 259 秒暂停被两种模式各记一次
        let round = Round {
            id: 1,
            loop_number: Some(1),
            cycle_type: CycleType::Normal(1),
            layer_index: 0,
            start_ts: 0.0,
            end_ts: Some(328.5),
            pose0: None,
            pose6: None,
            pause_events: vec![pause(10.0, 269.0), pause(10.5, 269.5)],
        };

        // 朴素求和会扣掉 518 秒 → 归零；合并后只扣 259.5 秒
        assert!((round.total_pause_duration() - 259.5).abs() < 1e-9);
        assert!((round.effective_duration() - 69.0).abs() < 1e-9);
    }
}
