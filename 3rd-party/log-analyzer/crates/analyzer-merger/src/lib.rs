//! Analyzer Merger - 时间轴合并模块
//!
//! 用于合并多个日志分析器生成的时间线数据，以主时间线为基准进行时间对齐。
//!
//! # 功能
//!
//! - 合并多个 Timeline 为统一的时间轴
//! - 基于主时间线进行时间对齐
//! - 支持时间容差（time tolerance）
//! - 事件排序和去重
//! - 父子事件关系保持

// i18n 初始化
rust_i18n::i18n!("locales", fallback = "zh-CN");
use rust_i18n::t;

use analyzer_core::timeline::*;
use anyhow::{Context, Result};
use std::collections::HashMap;

/// 时间对齐策略
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlignmentStrategy {
    /// 直接使用时间戳（假设所有日志时间已同步）
    Timestamp,
    /// 基于事件特征进行对齐（未实现）
    EventBased,
}

/// 合并配置
#[derive(Debug, Clone)]
pub struct MergeConfig {
    /// 主时间线来源名称（如 "master_control"）
    pub primary_source: String,
    /// 时间容差（秒），用于判断事件是否同时发生
    pub time_tolerance: f64,
    /// 对齐策略
    pub alignment_strategy: AlignmentStrategy,
    /// 泳道优先级映射（控制显示顺序）
    pub track_priority: HashMap<String, i32>,
}

impl Default for MergeConfig {
    fn default() -> Self {
        let mut track_priority = HashMap::new();
        track_priority.insert("RoundMarker".to_string(), 0);
        track_priority.insert("Navigation".to_string(), 1);
        track_priority.insert("Arm".to_string(), 2);
        track_priority.insert("Gripper".to_string(), 3);
        track_priority.insert("Head".to_string(), 4);
        track_priority.insert("Waist".to_string(), 5);

        Self {
            primary_source: "master_control".to_string(),
            time_tolerance: 1.0,
            alignment_strategy: AlignmentStrategy::Timestamp,
            track_priority,
        }
    }
}

/// 合并后的时间线
#[derive(Debug, Clone)]
pub struct MergedTimeline {
    /// 合并后的所有事件
    pub events: Vec<TimelineEvent>,
    /// 时间范围
    pub time_range: (f64, f64),
    /// 来源统计（每个来源的事件数）
    pub source_stats: HashMap<String, usize>,
    /// 泳道统计（每个泳道的事件数）
    pub track_stats: HashMap<String, usize>,
    /// 主时间线信息
    pub primary_timeline: Timeline,
}

/// 时间轴合并器
pub struct TimelineMerger {
    config: MergeConfig,
}

impl TimelineMerger {
    /// 创建新的合并器
    pub fn new(config: MergeConfig) -> Self {
        Self { config }
    }

    /// 使用默认配置创建合并器
    pub fn with_defaults() -> Self {
        Self::new(MergeConfig::default())
    }

    /// 合并多个时间线
    ///
    /// # 参数
    ///
    /// - `timelines`: 要合并的时间线列表
    ///
    /// # 返回
    ///
    /// - 合并后的时间线
    ///
    /// # 错误
    ///
    /// - 如果没有找到主时间线
    /// - 如果时间线列表为空
    pub fn merge(&self, timelines: Vec<Timeline>) -> Result<MergedTimeline> {
        if timelines.is_empty() {
            anyhow::bail!("{}", t!("err.empty_timeline"));
        }

        // 1. 查找主时间线
        let primary_timeline = timelines
            .iter()
            .find(|t| t.is_primary)
            .or_else(|| {
                timelines
                    .iter()
                    .find(|t| t.name.as_str() == self.config.primary_source)
            })
            .context(t!("err.no_primary").to_string())?
            .clone();

        tracing::info!(
            "找到主时间线: {} (共 {} 个事件)",
            primary_timeline.name,
            primary_timeline.events.len()
        );

        // 2. 收集所有事件
        let mut all_events = Vec::new();
        for timeline in &timelines {
            for event in timeline.events.iter() {
                all_events.push(event.clone());
            }
        }

        tracing::info!("收集到 {} 个事件", all_events.len());

        // 3. 根据对齐策略处理事件（当前仅支持 Timestamp）
        match self.config.alignment_strategy {
            AlignmentStrategy::Timestamp => {
                // 直接使用时间戳，不做对齐处理
                tracing::debug!("使用时间戳对齐策略");
            }
            AlignmentStrategy::EventBased => {
                tracing::warn!("EventBased 对齐策略尚未实现，回退到 Timestamp");
            }
        }

        // 4. 按开始时间排序
        all_events.sort_by(|a, b| {
            a.start_time
                .partial_cmp(&b.start_time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 5. 计算时间范围
        use abi_stable::std_types::ROption::RSome;

        let min_time = all_events
            .iter()
            .map(|e| e.start_time)
            .min_by(|a, b| a.total_cmp(b))
            .unwrap_or(primary_timeline.log_start_time);

        let max_time = all_events
            .iter()
            .filter_map(|e| match &e.end_time {
                RSome(t) => Some(*t),
                _ => None,
            })
            .max_by(|a, b| a.total_cmp(b))
            .unwrap_or(primary_timeline.log_end_time);

        // 6. 统计信息
        let mut source_stats = HashMap::new();
        let mut track_stats = HashMap::new();

        for event in &all_events {
            *source_stats.entry(event.source.to_string()).or_insert(0) += 1;

            let track_name = match &event.track {
                Track::RoundMarker => "RoundMarker",
                Track::Navigation => "Navigation",
                Track::Arm => "Arm",
                Track::Head => "Head",
                Track::Waist => "Waist",
                Track::Custom(name) => name.as_str(),
            };
            *track_stats.entry(track_name.to_string()).or_insert(0) += 1;
        }

        tracing::info!("合并完成:");
        tracing::info!(
            "  - 时间范围: {:.2} ~ {:.2} (共 {:.2}s)",
            min_time,
            max_time,
            max_time - min_time
        );
        tracing::info!("  - 来源统计: {:?}", source_stats);
        tracing::info!("  - 泳道统计: {:?}", track_stats);

        Ok(MergedTimeline {
            events: all_events,
            time_range: (min_time, max_time),
            source_stats,
            track_stats,
            primary_timeline,
        })
    }

    /// 查找指定时间窗口内的事件
    ///
    /// 判断事件是否与时间窗口有重叠：
    /// - 事件开始时间 <= 窗口结束时间
    /// - 事件结束时间 >= 窗口开始时间
    ///
    /// # 参数
    ///
    /// - `merged`: 合并后的时间线
    /// - `start_time`: 开始时间
    /// - `end_time`: 结束时间
    ///
    /// # 返回
    ///
    /// - 与时间窗口有重叠的事件列表
    pub fn find_events_in_window<'a>(
        &self,
        merged: &'a MergedTimeline,
        start_time: f64,
        end_time: f64,
    ) -> Vec<&'a TimelineEvent> {
        use abi_stable::std_types::ROption::{RNone, RSome};

        merged
            .events
            .iter()
            .filter(|e| {
                let event_end = match &e.end_time {
                    RSome(t) => *t,
                    RNone => e.start_time, // 如果没有结束时间，使用开始时间
                };
                // 事件与窗口有重叠：event_start <= window_end && event_end >= window_start
                e.start_time <= end_time && event_end >= start_time
            })
            .collect()
    }

    /// 查找与指定事件同时发生的其他事件
    ///
    /// 使用时间容差判断事件是否同时发生
    ///
    /// # 参数
    ///
    /// - `merged`: 合并后的时间线
    /// - `event`: 参考事件
    ///
    /// # 返回
    ///
    /// - 同时发生的事件列表
    pub fn find_concurrent_events<'a>(
        &self,
        merged: &'a MergedTimeline,
        event: &TimelineEvent,
    ) -> Vec<&'a TimelineEvent> {
        use abi_stable::std_types::ROption::{RNone, RSome};

        let tolerance = self.config.time_tolerance;
        merged
            .events
            .iter()
            .filter(|e| {
                // 排除自己
                if e.id == event.id {
                    return false;
                }

                // 检查时间重叠
                let e_start = e.start_time;
                let e_end = match &e.end_time {
                    RSome(t) => *t,
                    RNone => e_start,
                };
                let event_start = event.start_time;
                let event_end = match &event.end_time {
                    RSome(t) => *t,
                    RNone => event_start,
                };

                // 有重叠：e_start <= event_end + tolerance && e_end >= event_start - tolerance
                e_start <= event_end + tolerance && e_end >= event_start - tolerance
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi_stable::std_types::*;

    fn create_test_event(
        id: &str,
        track: Track,
        start: f64,
        end: Option<f64>,
        source: &str,
    ) -> TimelineEvent {
        TimelineEvent {
            id: id.into(),
            track,
            name: format!("Event {}", id).into(),
            start_time: start,
            end_time: end.into(),
            status: EventStatus::Success,
            source: source.into(),
            parent_id: RNone,
            metadata: RNone,
            color_hint: RNone,
        }
    }

    fn create_test_timeline(name: &str, is_primary: bool, events: Vec<TimelineEvent>) -> Timeline {
        Timeline {
            name: name.into(),
            source_file: format!("{}.log", name).into(),
            log_start_time: 0.0,
            log_end_time: 100.0,
            events: events.into(),
            is_primary,
            metadata: RNone,
        }
    }

    #[test]
    fn test_merge_empty_timelines() {
        let merger = TimelineMerger::with_defaults();
        let result = merger.merge(vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_merge_single_timeline() {
        let merger = TimelineMerger::with_defaults();
        let events = vec![
            create_test_event("e1", Track::RoundMarker, 1.0, Some(2.0), "master_control"),
            create_test_event("e2", Track::Navigation, 3.0, Some(4.0), "master_control"),
        ];
        let timeline = create_test_timeline("master_control", true, events);

        let merged = merger.merge(vec![timeline]).unwrap();
        assert_eq!(merged.events.len(), 2);
        assert_eq!(merged.time_range, (1.0, 4.0));
    }

    #[test]
    fn test_merge_multiple_timelines() {
        let merger = TimelineMerger::with_defaults();

        let events1 = vec![
            create_test_event("e1", Track::RoundMarker, 1.0, Some(10.0), "master_control"),
            create_test_event("e2", Track::Navigation, 2.0, Some(5.0), "master_control"),
        ];
        let timeline1 = create_test_timeline("master_control", true, events1);

        let events2 = vec![create_test_event(
            "e3",
            Track::Arm,
            3.0,
            Some(6.0),
            "external_arm",
        )];
        let timeline2 = create_test_timeline("external_arm", false, events2);

        let merged = merger.merge(vec![timeline1, timeline2]).unwrap();
        assert_eq!(merged.events.len(), 3);
        assert_eq!(merged.source_stats.len(), 2);
        assert_eq!(*merged.source_stats.get("master_control").unwrap(), 2);
        assert_eq!(*merged.source_stats.get("external_arm").unwrap(), 1);
    }

    #[test]
    fn test_events_sorted_by_time() {
        let merger = TimelineMerger::with_defaults();

        let events = vec![
            create_test_event("e3", Track::Arm, 5.0, Some(6.0), "master_control"),
            create_test_event("e1", Track::RoundMarker, 1.0, Some(2.0), "master_control"),
            create_test_event("e2", Track::Navigation, 3.0, Some(4.0), "master_control"),
        ];
        let timeline = create_test_timeline("master_control", true, events);

        let merged = merger.merge(vec![timeline]).unwrap();
        assert_eq!(merged.events[0].id.as_str(), "e1");
        assert_eq!(merged.events[1].id.as_str(), "e2");
        assert_eq!(merged.events[2].id.as_str(), "e3");
    }

    #[test]
    fn test_find_events_in_window() {
        let merger = TimelineMerger::with_defaults();

        let events = vec![
            create_test_event("e1", Track::RoundMarker, 1.0, Some(2.0), "master_control"),
            create_test_event("e2", Track::Navigation, 3.0, Some(4.0), "master_control"),
            create_test_event("e3", Track::Arm, 5.0, Some(6.0), "master_control"),
        ];
        let timeline = create_test_timeline("master_control", true, events);
        let merged = merger.merge(vec![timeline]).unwrap();

        let window_events = merger.find_events_in_window(&merged, 2.5, 5.5);
        assert_eq!(window_events.len(), 2); // e2 and e3
    }

    #[test]
    fn test_find_concurrent_events() {
        let merger = TimelineMerger::with_defaults();

        let events = vec![
            create_test_event("e1", Track::Navigation, 1.0, Some(5.0), "master_control"),
            create_test_event("e2", Track::Arm, 2.0, Some(4.0), "master_control"), // overlaps with e1
            create_test_event("e3", Track::Head, 10.0, Some(12.0), "master_control"), // no overlap
        ];
        let timeline = create_test_timeline("master_control", true, events);
        let merged = merger.merge(vec![timeline]).unwrap();

        let ref_event = &merged.events[0]; // e1
        let concurrent = merger.find_concurrent_events(&merged, ref_event);
        assert_eq!(concurrent.len(), 1); // only e2
        assert_eq!(concurrent[0].id.as_str(), "e2");
    }
}
