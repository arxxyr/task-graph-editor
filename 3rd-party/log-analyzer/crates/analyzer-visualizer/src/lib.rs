//! Analyzer Visualizer - 多泳道甘特图生成模块
//!
//! 用于生成统一的多泳道甘特图，将来自不同日志源的事件在一张图上显示。
//!
//! # 功能
//!
//! - 生成多泳道甘特图（Gantt Chart）
//! - 支持自定义泳道顺序和颜色
//! - 支持事件过滤和时间窗口
//! - 高分辨率输出（可配置）
//! - 自动时间轴标注

// i18n 初始化
rust_i18n::i18n!("locales", fallback = "zh-CN");
use rust_i18n::t;

use analyzer_core::timeline::*;
use analyzer_merger::MergedTimeline;
use anyhow::Result;
use plotters::prelude::*;
use std::collections::HashMap;

mod font_loader;
use font_loader::FontLoader;
pub use font_loader::use_embedded_font;

/// 预检字体可用性
///
/// 在程序启动时调用，检测生成甘特图所需的字体是否可用。
/// 如果字体不可用，打印警告但不阻止程序继续运行。
pub fn check_font_availability() {
    let loader = FontLoader::default();
    if let Err(e) = loader.validate() {
        tracing::warn!("{}", e);
        eprintln!("[字体] 甘特图生成可能失败，CSV 分析结果不受影响");
    }
}

/// 可视化配置
#[derive(Debug, Clone)]
pub struct VisualizationConfig {
    /// 输出图片宽度（像素）
    pub width: u32,
    /// 输出图片高度（像素）
    pub height: u32,
    /// 每个泳道的高度（像素）
    pub track_height: u32,
    /// 泳道间距（像素）
    pub track_spacing: u32,
    /// 顶部边距（像素）
    pub margin_top: u32,
    /// 底部边距（像素）
    pub margin_bottom: u32,
    /// 左侧边距（像素，用于泳道标签）
    pub margin_left: u32,
    /// 右侧边距（像素）
    pub margin_right: u32,
    /// 时间轴标签数量
    pub time_label_count: usize,
    /// 是否显示事件文本标签
    pub show_event_labels: bool,
    /// 字体大小
    pub font_size: u32,
    /// 泳道优先级（控制显示顺序）
    pub track_priority: HashMap<String, i32>,
    /// 泳道颜色映射
    pub track_colors: HashMap<String, String>,
}

impl Default for VisualizationConfig {
    fn default() -> Self {
        let mut track_priority = HashMap::new();
        track_priority.insert("RoundMarker".to_string(), 0);
        track_priority.insert("Navigation".to_string(), 1);
        track_priority.insert("Arm".to_string(), 2);
        track_priority.insert("Gripper".to_string(), 3);
        track_priority.insert("Head".to_string(), 4);
        track_priority.insert("Waist".to_string(), 5);

        let mut track_colors = HashMap::new();
        track_colors.insert("RoundMarker".to_string(), "#E8F4F8".to_string());
        track_colors.insert("Navigation".to_string(), "#ADD8E6".to_string());
        track_colors.insert("Arm".to_string(), "#90EE90".to_string());
        track_colors.insert("Gripper".to_string(), "#F0E68C".to_string());
        track_colors.insert("Head".to_string(), "#FFB366".to_string());
        track_colors.insert("Waist".to_string(), "#DDA0DD".to_string());

        Self {
            width: 2800,
            height: 1600,
            track_height: 40,
            track_spacing: 10,
            margin_top: 80,
            margin_bottom: 80,
            margin_left: 200,
            margin_right: 50,
            time_label_count: 10,
            show_event_labels: true,
            font_size: 18,
            track_priority,
            track_colors,
        }
    }
}

/// 多泳道甘特图生成器
pub struct GanttChartGenerator {
    config: VisualizationConfig,
}

impl GanttChartGenerator {
    /// 创建新的生成器
    pub fn new(config: VisualizationConfig) -> Self {
        Self { config }
    }

    /// 使用默认配置创建生成器
    pub fn with_defaults() -> Self {
        Self::new(VisualizationConfig::default())
    }

    /// 生成多泳道甘特图
    ///
    /// # 参数
    ///
    /// - `merged`: 合并后的时间线
    /// - `output_path`: 输出文件路径
    /// - `title`: 图表标题
    ///
    /// # 返回
    ///
    /// - 成功则返回 Ok(())
    pub fn generate_gantt_chart(
        &self,
        merged: &MergedTimeline,
        output_path: &str,
        title: &str,
    ) -> Result<()> {
        if merged.events.is_empty() {
            anyhow::bail!("{}", t!("err.no_events"));
        }

        // 创建字体加载器
        let font_loader = FontLoader::default();

        tracing::info!("开始生成甘特图: {}", output_path);
        tracing::info!("事件总数: {}", merged.events.len());

        // 1. 按泳道分组事件
        let tracks = self.group_events_by_track(&merged.events);
        let track_count = tracks.len();

        tracing::info!("泳道数量: {}", track_count);

        // 2. 预先生成泳道标签
        let track_labels: Vec<String> = tracks
            .iter()
            .map(|(track, _)| self.track_name_to_string(track))
            .collect();

        // 3. 计算图表高度
        let chart_height = self.config.margin_top
            + self.config.margin_bottom
            + (track_count as u32) * (self.config.track_height + self.config.track_spacing);

        // 4. 创建绘图区域
        let root =
            BitMapBackend::new(output_path, (self.config.width, chart_height)).into_drawing_area();
        root.fill(&WHITE)?;

        // 5. 获取时间范围
        let (t_min, t_max) = merged.time_range;
        let t_duration = t_max - t_min;

        // 6. 创建主图表区域
        let mut chart = ChartBuilder::on(&root)
            .caption(title, font_loader.font_desc(self.config.font_size as i32))
            .margin(5)
            .x_label_area_size(self.config.margin_bottom)
            .y_label_area_size(self.config.margin_left)
            .build_cartesian_2d(t_min..t_max, 0.0..(track_count as f64))?;

        // 7. 绘制时间轴
        // Clone track_labels for the closure to avoid lifetime issues
        let track_labels_for_formatter = track_labels.clone();
        chart
            .configure_mesh()
            .label_style(font_loader.font_desc(14))
            .x_labels(self.config.time_label_count)
            .y_labels(track_count)
            .y_label_formatter(&move |y| {
                let track_idx = *y as usize;
                track_labels_for_formatter
                    .get(track_idx)
                    .cloned()
                    .unwrap_or_default()
            })
            .x_label_formatter(&|x| format!("{:.1}s", x - t_min))
            .draw()?;

        // 8. 绘制每个泳道的事件
        for (track_idx, (track, events)) in tracks.iter().enumerate() {
            let y_base = track_idx as f64 + 0.1;
            let y_height = 0.8;

            for event in events {
                use abi_stable::std_types::ROption::RSome;

                let start = event.start_time;
                let end = match &event.end_time {
                    RSome(t) => *t,
                    _ => start + t_duration * 0.01, // 如果没有结束时间，显示一个小点
                };

                // 获取颜色
                let color_str = self.get_track_color(track);
                let color = self.parse_color(&color_str);

                // 绘制矩形
                chart.draw_series(std::iter::once(Rectangle::new(
                    [(start, y_base), (end, y_base + y_height)],
                    color.filled(),
                )))?;

                // 绘制边框
                chart.draw_series(std::iter::once(Rectangle::new(
                    [(start, y_base), (end, y_base + y_height)],
                    BLACK.stroke_width(1),
                )))?;

                // 绘制事件标签（如果启用）
                if self.config.show_event_labels && (end - start) > t_duration * 0.02 {
                    let label_x = start + (end - start) * 0.5;
                    let label_y = y_base + y_height * 0.5;
                    let label_text = event.name.to_string(); // 转换为 String 以避免生命周期问题

                    chart.draw_series(std::iter::once(Text::new(
                        label_text,
                        (label_x, label_y),
                        font_loader.font_desc(12).color(&BLACK),
                    )))?;
                }
            }
        }

        root.present()?;
        tracing::info!("甘特图生成完成: {}", output_path);

        Ok(())
    }

    /// 按泳道分组事件
    fn group_events_by_track(&self, events: &[TimelineEvent]) -> Vec<(Track, Vec<TimelineEvent>)> {
        // 使用 Track 的配置键名（英文）收集事件
        let mut track_map: HashMap<String, (Track, Vec<TimelineEvent>)> = HashMap::new();

        for event in events {
            let track_key = self.track_to_config_key(&event.track);
            track_map
                .entry(track_key)
                .or_insert_with(|| (event.track.clone(), Vec::new()))
                .1
                .push(event.clone());
        }

        // 按优先级排序泳道（使用配置键名查找优先级）
        let mut sorted_tracks: Vec<(String, i32, Track, Vec<TimelineEvent>)> = track_map
            .into_iter()
            .map(|(key, (track, events))| {
                let priority = self.config.track_priority.get(&key).copied().unwrap_or(999);
                (key, priority, track, events)
            })
            .collect();

        sorted_tracks.sort_by_key(|(_, priority, _, _)| *priority);

        // 返回 (Track, Vec<TimelineEvent>)
        sorted_tracks
            .into_iter()
            .map(|(_, _, track, events)| (track, events))
            .collect()
    }

    /// Track 转换为配置键名（英文，用于查找 track_priority）
    fn track_to_config_key(&self, track: &Track) -> String {
        match track {
            Track::RoundMarker => "RoundMarker".to_string(),
            Track::Navigation => "Navigation".to_string(),
            Track::Arm => "Arm".to_string(),
            Track::Head => "Head".to_string(),
            Track::Waist => "Waist".to_string(),
            Track::Custom(name) => name.to_string(),
        }
    }

    /// Track 转换为字符串（用于Y轴标签显示）
    fn track_name_to_string(&self, track: &Track) -> String {
        match track {
            Track::RoundMarker => t!("track.round_marker").to_string(),
            Track::Navigation => t!("track.navigation").to_string(),
            Track::Arm => t!("track.arm").to_string(),
            Track::Head => t!("track.head").to_string(),
            Track::Waist => t!("track.waist").to_string(),
            Track::Custom(name) => {
                // 自定义泳道的名称映射
                match name.as_str() {
                    "PrePlanNavigation" => t!("track.preplan").to_string(),
                    "Gripper" => t!("track.gripper").to_string(),
                    _ => name.to_string(),
                }
            }
        }
    }

    /// 获取泳道颜色
    fn get_track_color(&self, track: &Track) -> String {
        let track_key = match track {
            Track::RoundMarker => "RoundMarker",
            Track::Navigation => "Navigation",
            Track::Arm => "Arm",
            Track::Head => "Head",
            Track::Waist => "Waist",
            Track::Custom(name) => name.as_str(),
        };

        self.config
            .track_colors
            .get(track_key)
            .cloned()
            .unwrap_or_else(|| "#CCCCCC".to_string())
    }

    /// 解析颜色字符串
    fn parse_color(&self, color_str: &str) -> RGBColor {
        // 简单的十六进制颜色解析
        if color_str.starts_with('#') && color_str.len() == 7 {
            let r = u8::from_str_radix(&color_str[1..3], 16).unwrap_or(200);
            let g = u8::from_str_radix(&color_str[3..5], 16).unwrap_or(200);
            let b = u8::from_str_radix(&color_str[5..7], 16).unwrap_or(200);
            RGBColor(r, g, b)
        } else {
            RGBColor(200, 200, 200)
        }
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

    fn create_test_merged_timeline() -> MergedTimeline {
        let events = vec![
            create_test_event("e1", Track::RoundMarker, 0.0, Some(10.0), "master_control"),
            create_test_event("e2", Track::Navigation, 1.0, Some(3.0), "master_control"),
            create_test_event("e3", Track::Arm, 3.5, Some(6.0), "external_arm"),
            create_test_event(
                "e4",
                Track::Custom("Gripper".into()),
                6.2,
                Some(6.8),
                "master_control",
            ),
            create_test_event("e5", Track::Head, 7.0, Some(8.0), "master_control"),
        ];

        let mut source_stats = HashMap::new();
        source_stats.insert("master_control".to_string(), 4);
        source_stats.insert("external_arm".to_string(), 1);

        let mut track_stats = HashMap::new();
        track_stats.insert("RoundMarker".to_string(), 1);
        track_stats.insert("Navigation".to_string(), 1);
        track_stats.insert("Arm".to_string(), 1);
        track_stats.insert("Gripper".to_string(), 1);
        track_stats.insert("Head".to_string(), 1);

        let primary_timeline = Timeline {
            name: "master_control".into(),
            source_file: "test.log".into(),
            log_start_time: 0.0,
            log_end_time: 10.0,
            events: RVec::new(),
            is_primary: true,
            metadata: RNone,
        };

        MergedTimeline {
            events,
            time_range: (0.0, 10.0),
            source_stats,
            track_stats,
            primary_timeline,
        }
    }

    #[test]
    fn test_group_events_by_track() {
        let generator = GanttChartGenerator::with_defaults();
        let merged = create_test_merged_timeline();

        let grouped = generator.group_events_by_track(&merged.events);

        assert_eq!(grouped.len(), 5); // RoundMarker, Navigation, Arm, Gripper, Head
        assert_eq!(grouped[0].1.len(), 1); // RoundMarker has 1 event
        assert_eq!(grouped[1].1.len(), 1); // Navigation has 1 event
        assert!(matches!(
            &grouped[3].0,
            Track::Custom(name) if name.as_str() == "Gripper"
        ));
    }

    #[test]
    fn test_track_name_conversion() {
        let generator = GanttChartGenerator::with_defaults();

        // 测试泳道名称转换（依赖当前语言设置）
        let round_marker_name = generator.track_name_to_string(&Track::RoundMarker);
        let nav_name = generator.track_name_to_string(&Track::Navigation);
        let arm_name = generator.track_name_to_string(&Track::Arm);
        let gripper_name = generator.track_name_to_string(&Track::Custom("Gripper".into()));

        // 确保返回非空字符串
        assert!(!round_marker_name.is_empty());
        assert!(!nav_name.is_empty());
        assert!(!arm_name.is_empty());
        assert!(!gripper_name.is_empty());
    }

    #[test]
    fn test_color_parsing() {
        let generator = GanttChartGenerator::with_defaults();

        let color = generator.parse_color("#FF0000");
        assert_eq!(color, RGBColor(255, 0, 0));

        let color2 = generator.parse_color("#00FF00");
        assert_eq!(color2, RGBColor(0, 255, 0));

        let color3 = generator.parse_color("#0000FF");
        assert_eq!(color3, RGBColor(0, 0, 255));
    }
}
