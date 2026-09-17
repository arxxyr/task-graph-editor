//! Master Control Analyzer Plugin
//!
//! 用于分析机器人控制系统（master_control）日志的插件。
//!
//! # 功能
//!
//! - 解析日志文件并提取时间戳信息
//! - 检测任务轮次（基于循环标记）
//! - 分析导航流程和机械臂操作
//! - 生成 CSV 报告和甘特图

use abi_stable::{
    export_root_module, prefix_type::PrefixTypeTrait, sabi_extern_fn,
    sabi_trait::prelude::TD_Opaque, std_types::*,
};
use analyzer_core::*;
use anyhow::Result;

// i18n 初始化
rust_i18n::i18n!("locales", fallback = "zh-CN");
use rust_i18n::t;

// 模块声明
pub mod csv_exporter;
pub mod flow_detector;
mod font_loader;
pub use font_loader::use_embedded_font;
pub mod gantt;
pub mod models;
pub mod parser;
pub mod round_detector;
pub mod statistics;
pub mod utils;

// 重新导出常用类型
pub use models::{ActionOperation, CsvRecord, LogLine, NavigationFlow, Round, SubStep};

// ============================================================================
// 插件实现
// ============================================================================

/// Master Control Analyzer 插件
#[derive(Clone)]
struct MasterControlAnalyzer;

impl AnalyzerPlugin for MasterControlAnalyzer {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            name: "master-control-analyzer".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: t!("plugin.description").to_string().into(),
            author: "loosqk".into(),
            supported_extensions: vec![".log".into(), ".txt".into()].into(),
        }
    }

    fn analyze(&self, args: AnalyzeArgs) -> RResult<AnalyzeResult, RBoxError> {
        // 设置语言（从 args 中获取）
        let locale = args.locale.as_str();
        if !locale.is_empty() {
            rust_i18n::set_locale(locale);
        }

        // 将 ABI 稳定类型转换为 Rust 原生类型
        let input_file = args.input_file.as_str();
        let output_dir = args.output_dir.as_str();

        // 执行分析
        match args
            .extra_args
            .as_ref()
            .map(|json| serde_json::from_str::<statistics::StatisticsOptions>(json.as_str()))
            .into_option()
            .transpose()
            .map_err(anyhow::Error::from)
            .and_then(|options| {
                run_analysis_internal(input_file, output_dir, options.unwrap_or_default())
            }) {
            Ok(result) => ROk(result),
            Err(e) => {
                // 创建一个简单的错误类型
                #[derive(Debug)]
                struct AnalysisError(String);
                impl std::fmt::Display for AnalysisError {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        write!(f, "{}", self.0)
                    }
                }
                impl std::error::Error for AnalysisError {}

                let err = AnalysisError(format!("{:?}", e));
                RErr(RBoxError::new(err))
            }
        }
    }
}

/// 统计导航流程中的各类动作数量
///
/// # 返回
/// (导航数, 预打舵数, 手臂数, 头部数, 腰部数, 夹爪数)
fn count_actions(flows: &[NavigationFlow]) -> (usize, usize, usize, usize, usize, usize) {
    let mut nav_count = 0;
    let mut preplan_count = 0;
    let mut arm_count = 0;
    let mut head_count = 0;
    let mut waist_count = 0;
    let mut gripper_count = 0;

    for flow in flows {
        for op in &flow.operations {
            match op.action_type.as_str() {
                "navigation" => nav_count += 1,
                "preplan" => preplan_count += 1,
                "arm" => arm_count += 1,
                "head" => head_count += 1,
                "waist" => waist_count += 1,
                "gripper" => gripper_count += 1,
                _ => {}
            }
        }
    }

    (
        nav_count,
        preplan_count,
        arm_count,
        head_count,
        waist_count,
        gripper_count,
    )
}

/// 内部分析函数（使用原生 Rust 类型）
///
/// 封装了完整的分析流程，从日志解析到结果输出。
fn run_analysis_internal(
    input_file: &str,
    output_dir: &str,
    options: statistics::StatisticsOptions,
) -> Result<AnalyzeResult> {
    use csv_exporter::{build_csv_records, export_csv, generate_action_timeline_csv};
    use flow_detector::detect_flows;
    use gantt::generate_gantt_charts;
    use parser::load_log_lines;
    use round_detector::detect_rounds;

    // 1. 加载日志行
    let lines = load_log_lines(input_file)?;
    if lines.is_empty() {
        anyhow::bail!("{}", t!("err.no_valid_timestamps"));
    }

    let t0 = lines[0].timestamp;
    let t_last = lines.last().unwrap().timestamp;

    // 2. 检测轮次
    let rounds = detect_rounds(&lines, t_last)?;
    let round_count = rounds.len();

    // 3. 检测导航流程
    let flows = detect_flows(&lines, &rounds)?;
    let flow_count = flows.len();

    // 统计各类动作
    let (nav_count, preplan_count, arm_count, head_count, waist_count, gripper_count) =
        count_actions(&flows);

    // 4. 构建CSV记录
    let records = build_csv_records(&flows, &rounds, t0);
    let record_count = records.len();

    // 5. 创建输出目录
    std::fs::create_dir_all(output_dir)?;

    // 6. 导出所有结果
    let mut output_files = Vec::new();

    // 导出主分析CSV
    export_csv(&records, output_dir)?;
    output_files.push(OutputFile {
        path: format!("{}/analysis.csv", output_dir).into(),
        file_type: "csv".into(),
        description: t!("output.analysis_csv").to_string().into(),
    });

    // 生成甘特图（字体不可用时跳过，不影响 CSV 结果）
    match generate_gantt_charts(&flows, &rounds, output_dir, t0) {
        Ok(()) => {
            let width = utils::digit_width(rounds.len());
            for round in &rounds {
                output_files.push(OutputFile {
                    path: format!(
                        "{}/round_{:0width$}_gantt.png",
                        output_dir,
                        round.id,
                        width = width
                    )
                    .into(),
                    file_type: "png".into(),
                    description: t!("output.round_gantt", id = round.id).to_string().into(),
                });
            }
        }
        Err(e) => {
            eprintln!("[字体] 甘特图生成失败（跳过）: {}", e);
        }
    }

    // 生成动作时间轴
    generate_action_timeline_csv(&flows, &rounds, output_dir, t0)?;
    output_files.push(OutputFile {
        path: format!("{}/action_timeline.csv", output_dir).into(),
        file_type: "csv".into(),
        description: t!("output.action_timeline").to_string().into(),
    });

    // 统计图与统计 CSV 共用同一份轮次集合，原始明细不因筛选而丢失。
    let selection = statistics::select_rounds(&rounds, &flows, &lines, options);
    let selected_ids: std::collections::HashSet<_> = selection
        .rounds
        .iter()
        .filter(|r| r.included)
        .map(|r| r.round_id)
        .collect();
    let selected_rounds: Vec<_> = rounds
        .iter()
        .filter(|r| selected_ids.contains(&r.id))
        .cloned()
        .collect();
    std::fs::write(
        std::path::Path::new(output_dir).join("statistics_selection.json"),
        serde_json::to_vec_pretty(&selection)?,
    )?;
    match gantt::generate_cycle_duration_chart(&selected_rounds, &flows, output_dir) {
        Ok(()) if selection.included_count > 0 => {
            output_files.push(OutputFile {
                path: format!("{}/cycle_duration_stats.png", output_dir).into(),
                file_type: "png".into(),
                description: t!("output.cycle_duration_stats").to_string().into(),
            });
        }
        Ok(()) => {}
        Err(e) => {
            eprintln!("[字体] 耗时统计图生成失败（跳过）: {}", e);
        }
    }

    std::fs::write(
        std::path::Path::new(output_dir).join("round_overview.json"),
        serde_json::to_vec_pretty(&statistics::round_overview(
            &rounds,
            &flows,
            &selection,
            std::path::Path::new(output_dir),
        ))?,
    )?;

    // 8. 构建标准化时间线数据
    let timeline = build_timeline(input_file, &rounds, &flows, t0, t_last)?;

    // 9. 构建分析摘要
    let summary = format!(
        "{}\n- {}\n- {}\n- {}\n- {}\n- {}",
        t!("summary.complete"),
        t!("summary.rounds_detected", count = round_count),
        t!("summary.flows_detected", count = flow_count),
        t!(
            "summary.action_stats",
            nav = nav_count,
            preplan = preplan_count,
            arm = arm_count,
            head = head_count,
            waist = waist_count,
            gripper = gripper_count
        ),
        t!("summary.csv_records", count = record_count),
        t!("summary.output_dir", path = output_dir)
    );

    Ok(AnalyzeResult {
        summary: format!("{summary}\n{}", selection.summary()).into(),
        output_files: output_files.into(),
        timeline,
    })
}

/// 构建标准化时间线数据
///
/// 将 rounds、flows 和 operations 转换为标准的 Timeline 结构
fn build_timeline(
    source_file: &str,
    rounds: &[Round],
    flows: &[NavigationFlow],
    log_start_time: f64,
    log_end_time: f64,
) -> Result<timeline::Timeline> {
    use analyzer_core::timeline::*;

    let mut events = Vec::new();

    // 1. 添加轮次标记事件
    for (idx, round) in rounds.iter().enumerate() {
        let event = TimelineEvent {
            id: format!("round_{}", round.id).into(),
            track: Track::RoundMarker,
            name: t!("timeline.round", id = round.id).into(),
            start_time: round.start_ts,
            end_time: round.end_ts.into(),
            status: if round.end_ts.is_some() {
                EventStatus::Success
            } else {
                EventStatus::InProgress
            },
            source: "master_control".into(),
            parent_id: RNone,
            metadata: {
                let mut meta = serde_json::json!({
                    "loop_number": round.loop_number,
                    "round_index": idx,
                });
                if let Some(ref pose0) = round.pose0 {
                    meta["pose0"] = serde_json::json!(pose0);
                }
                if let Some(ref pose6) = round.pose6 {
                    meta["pose6"] = serde_json::json!(pose6);
                }
                RSome(meta.to_string().into())
            },
            color_hint: RSome("#E8F4F8".into()), // 浅蓝色
        };
        events.push(event);
    }

    // 2. 添加导航流程事件
    for (flow_idx, flow) in flows.iter().enumerate() {
        let round_id = format!("round_{}", flow.round_id);

        // 导航主事件
        if let (Some(nav_start), Some(nav_end)) = (flow.nav_start_ts, flow.nav_end_ts) {
            let nav_event = TimelineEvent {
                id: format!("nav_{}", flow_idx).into(),
                track: Track::Navigation,
                name: t!("action.navigation").into(),
                start_time: nav_start,
                end_time: RSome(nav_end),
                status: match flow.nav_status.as_str() {
                    "ok" => EventStatus::Success,
                    s if s.starts_with("failed") => EventStatus::Failed,
                    "incomplete" => EventStatus::InProgress,
                    _ => EventStatus::Success,
                },
                source: "master_control".into(),
                parent_id: RSome(round_id.clone().into()),
                metadata: {
                    let meta = serde_json::json!({
                        "target_pos": flow.nav_target_pos,
                        "target_ori": flow.nav_target_ori,
                        "sub_steps": flow.nav_sub_steps.iter().map(|s| {
                            serde_json::json!({
                                "name": s.name,
                                "timestamp": s.timestamp,
                            })
                        }).collect::<Vec<_>>(),
                    });
                    RSome(meta.to_string().into())
                },
                color_hint: RSome("#ADD8E6".into()), // 浅蓝色
            };
            events.push(nav_event);
        }

        // 3. 添加机械臂、夹爪、头部、腰部动作事件
        for (op_idx, op) in flow.operations.iter().enumerate() {
            if let (Some(start), Some(end)) = (op.start_ts, op.end_ts) {
                let (track, color) = match op.action_type.as_str() {
                    "arm" => (Track::Arm, "#90EE90"),                          // 浅绿色
                    "gripper" => (Track::Custom("Gripper".into()), "#F0E68C"), // 卡其色
                    "head" => (Track::Head, "#FFB366"),                        // 浅橙色
                    "waist" => (Track::Waist, "#DDA0DD"),                      // 浅紫色
                    _ => continue,                                             // 跳过未知类型
                };

                let op_event = TimelineEvent {
                    id: format!("{}_{}_op_{}", op.action_type, flow_idx, op_idx).into(),
                    track,
                    name: op.label.clone().into(),
                    start_time: start,
                    end_time: RSome(end),
                    status: match op.status.as_str() {
                        "ok" | "completed" => EventStatus::Success,
                        s if s.starts_with("failed") => EventStatus::Failed,
                        "incomplete" | "in_progress" => EventStatus::InProgress,
                        "cancelled" => EventStatus::Cancelled,
                        _ => EventStatus::Success,
                    },
                    source: "master_control".into(),
                    parent_id: RSome(format!("nav_{}", flow_idx).into()),
                    metadata: {
                        let meta = serde_json::json!({
                            "action_code": op.action_code,
                            "action_type": op.action_type,
                            "sub_steps": op.sub_steps.iter().map(|s| {
                                serde_json::json!({
                                    "name": s.name,
                                    "timestamp": s.timestamp,
                                })
                            }).collect::<Vec<_>>(),
                        });
                        RSome(meta.to_string().into())
                    },
                    color_hint: RSome(color.into()),
                };
                events.push(op_event);
            }
        }
    }

    // 构建最终的 Timeline
    let timeline = Timeline {
        name: "master_control".into(),
        source_file: source_file.into(),
        log_start_time,
        log_end_time,
        events: events.into(),
        is_primary: true, // master_control 是主时间轴
        metadata: {
            let meta = serde_json::json!({
                "round_count": rounds.len(),
                "flow_count": flows.len(),
                "analyzer_version": env!("CARGO_PKG_VERSION"),
            });
            RSome(meta.to_string().into())
        },
    };

    Ok(timeline)
}

// ============================================================================
// 插件导出
// ============================================================================

// 在插件中导出根模块，使用 analyzer-core 中定义的类型
#[export_root_module]
pub fn get_root_module() -> AnalyzerPluginModule_Ref {
    AnalyzerPluginModule { create_plugin }.leak_into_prefix()
}

/// 创建插件实例的工厂函数
#[sabi_extern_fn]
pub fn create_plugin() -> AnalyzerPlugin_TO<'static, RBox<()>> {
    // 字体会在生成图表时自动检测系统字体
    // 无需预加载嵌入字体
    AnalyzerPlugin_TO::from_value(MasterControlAnalyzer, TD_Opaque)
}

// ============================================================================
// 公共 API（供直接调用使用，非插件模式）
// ============================================================================

/// 执行日志分析（公共 API）
///
/// 这个函数可以在非插件模式下直接调用，方便测试和独立使用。
///
/// # 参数
///
/// - `input_file`: 日志文件路径
/// - `output_dir`: 输出目录路径
///
/// # 返回
///
/// - `Ok(String)`: 分析成功，返回摘要信息
/// - `Err(anyhow::Error)`: 分析失败
pub fn run_analysis(input_file: &str, output_dir: &str) -> Result<String> {
    let result = run_analysis_internal(
        input_file,
        output_dir,
        statistics::StatisticsOptions::default(),
    )?;
    Ok(result.summary.to_string())
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use analyzer_core::timeline::Track;

    use crate::models::{CycleType, NavigationFlow, Round};

    #[test]
    fn test_plugin_metadata() {
        let plugin = MasterControlAnalyzer;
        let meta = plugin.metadata();
        assert_eq!(meta.name.as_str(), "master-control-analyzer");
        assert!(
            meta.supported_extensions
                .iter()
                .any(|ext| ext.as_str() == ".log")
        );
    }

    #[test]
    fn gripper_operation_uses_custom_timeline_track() {
        let rounds = vec![Round {
            id: 1,
            loop_number: Some(1),
            cycle_type: CycleType::Normal(1),
            layer_index: 0,
            start_ts: 0.0,
            end_ts: Some(10.0),
            pose0: None,
            pose6: None,
            pause_events: Vec::new(),
        }];

        let flows = vec![NavigationFlow {
            nav_start_ts: None,
            nav_end_ts: None,
            nav_target_pos: None,
            nav_target_ori: None,
            nav_status: "ok".to_string(),
            nav_sub_steps: Vec::new(),
            round_id: 1,
            operations: vec![ActionOperation {
                action_type: "gripper".to_string(),
                action_code: None,
                label: "gripper(right,open_mode=1,effective_open=true)".to_string(),
                start_ts: Some(1.0),
                end_ts: Some(2.0),
                status: "ok".to_string(),
                sub_steps: Vec::new(),
                pause_events: Vec::new(),
            }],
        }];

        let timeline =
            build_timeline("test.log", &rounds, &flows, 0.0, 10.0).expect("timeline should build");

        assert!(timeline.events.iter().any(|event| {
            matches!(&event.track, Track::Custom(name) if name.as_str() == "Gripper")
        }));
    }
}
