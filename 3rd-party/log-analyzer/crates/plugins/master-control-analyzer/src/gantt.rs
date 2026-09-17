//! 甘特图生成模块
//!
//! 本模块负责生成任务轮次的甘特图可视化

use std::collections::HashMap;

use anyhow::Result;
use csv;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use rust_i18n::t;

use crate::font_loader::FontLoader;
use crate::models::{NavigationFlow, PauseEvent, Round, SubStep, merge_intervals};
use crate::utils::timestamp_to_beijing_time;

/// 计算在指定时间点之前的累计暂停时间
///
/// 用于压缩时间轴，删除暂停期间的空白时间。
/// 同一段暂停可能被多种检测模式重复记录，先合并重叠区间再累加，避免重复扣除。
fn pause_time_before(pause_events: &[PauseEvent], round_start: f64, ts: f64) -> f64 {
    // 未恢复的暂停视为持续到 ts
    let intervals = pause_events
        .iter()
        .map(|e| (e.pause_ts, e.resume_ts.unwrap_or(ts)))
        .collect();

    merge_intervals(intervals)
        .iter()
        .map(|&(pause_start, pause_end)| {
            if ts <= pause_start {
                // 时间点在暂停开始之前，不计算
                0.0
            } else if ts >= pause_end {
                // 时间点在暂停结束之后，计算完整暂停时间
                pause_end - pause_start
            } else {
                // 时间点在暂停期间，计算部分暂停时间
                ts - pause_start
            }
        })
        .sum::<f64>()
        .min(ts - round_start) // 确保不超过总时间
}

/// 计算动作在压缩时间轴上的长度
///
/// 甘特图横轴已扣除暂停时间，若动作长度仍按墙钟计算，
/// 跨越暂停的动作（如暂停期间挂起的 BT 节点）会冲出坐标轴，
/// 把其余动作挤成无法辨认的细线。
/// 因此起止两端都用同一套压缩映射，长度即两端压缩坐标之差。
///
/// `end_ts` 为 `None`（动作未完成）时给 1 秒占位，与原行为一致。
fn compressed_duration(round: &Round, start_ts: f64, end_ts: Option<f64>) -> f64 {
    let Some(end_ts) = end_ts else {
        return 1.0;
    };

    let start_compressed =
        start_ts - pause_time_before(&round.pause_events, round.start_ts, start_ts);
    let end_compressed = end_ts - pause_time_before(&round.pause_events, round.start_ts, end_ts);

    (end_compressed - start_compressed).max(0.0)
}

/// 动作类型配置（顺序、显示名称、颜色）
const ACTION_TYPE_CONFIG: &[(&str, &str, RGBColor)] = &[
    ("navigation", "导航", RGBColor(173, 216, 230)), // 浅蓝色
    ("preplan", "预打舵", RGBColor(255, 255, 150)),  // 浅黄色
    ("arm", "手臂", RGBColor(144, 238, 144)),        // 浅绿色
    ("head", "头部", RGBColor(255, 218, 185)),       // 浅橙色
    ("waist", "腰部", RGBColor(221, 160, 221)),      // 浅紫色
    ("ready_pose", "准备阶段", RGBColor(176, 224, 230)), // 粉蓝色
    ("det_obj_pose", "目标检测", RGBColor(255, 228, 181)), // 浅黄橙
    ("obstacle", "障碍物", RGBColor(255, 182, 193)), // 浅粉红
    ("transition", "过渡点", RGBColor(216, 191, 216)), // 淡紫色
    ("arm_move_plan", "手臂规划", RGBColor(144, 202, 249)), // 浅蓝色
    ("arm_move_exec", "手臂执行", RGBColor(152, 251, 152)), // 淡绿色
    ("gripper", "夹爪", RGBColor(240, 230, 140)),    // 卡其色
];

/// 获取动作类型的颜色
fn get_action_color(action_type: &str) -> RGBColor {
    for (type_key, _, color) in ACTION_TYPE_CONFIG {
        if *type_key == action_type || (action_type == "nav" && *type_key == "navigation") {
            return *color;
        }
    }
    RGBColor(192, 192, 192) // 灰色（默认）
}

/// 获取动作类型的本地化显示名称
fn get_action_type_display(action_type: &str) -> String {
    match action_type {
        "navigation" | "nav" => t!("action.navigation").to_string(),
        "preplan" => t!("action.preplan").to_string(),
        "arm" => t!("action.arm").to_string(),
        "head" => t!("action.head").to_string(),
        "waist" => t!("action.waist").to_string(),
        "ready_pose" => t!("action.ready_pose").to_string(),
        "det_obj_pose" => t!("action.det_obj_pose").to_string(),
        "obstacle" => t!("action.obstacle").to_string(),
        "transition" => t!("action.transition").to_string(),
        "arm_move_plan" => t!("action.arm_move_plan").to_string(),
        "arm_move_exec" => t!("action.arm_move_exec").to_string(),
        "gripper" => t!("action.gripper").to_string(),
        _ => action_type.to_string(),
    }
}

/// 子步骤颜色配置
struct SubStepColorConfig {
    pattern: &'static str,
    color: RGBColor,
    use_alternating: bool,
}

/// 子步骤颜色映射表
const SUBSTEP_COLOR_CONFIG: &[SubStepColorConfig] = &[
    // BehaviorTree 模块相关
    SubStepColorConfig {
        pattern: "GetReadyPose",
        color: RGBColor(135, 206, 250),
        use_alternating: true,
    },
    SubStepColorConfig {
        pattern: "DetObjPose",
        color: RGBColor(255, 228, 181),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "ArmObstacle",
        color: RGBColor(255, 182, 193),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "ModifyArmObstacle",
        color: RGBColor(255, 182, 193),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "GetGoalPose",
        color: RGBColor(152, 251, 152),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "ArmTransitionPoint",
        color: RGBColor(216, 191, 216),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "transition",
        color: RGBColor(216, 191, 216),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "ExecuteDoubleArmMove",
        color: RGBColor(152, 251, 152),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "arm_move_plan",
        color: RGBColor(144, 202, 249),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "arm_move_exec",
        color: RGBColor(152, 251, 152),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "gripper",
        color: RGBColor(240, 230, 140),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "节点开始",
        color: RGBColor(100, 149, 237),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "节点结束",
        color: RGBColor(34, 139, 34),
        use_alternating: false,
    },
    // ROS2ActionAdapter 阶段
    SubStepColorConfig {
        pattern: "等待服务器",
        color: RGBColor(255, 215, 0),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "服务器已就绪",
        color: RGBColor(50, 205, 50),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "[RESULT]",
        color: RGBColor(147, 112, 219),
        use_alternating: false,
    },
    // 通用状态
    SubStepColorConfig {
        pattern: "完成",
        color: RGBColor(34, 139, 34),
        use_alternating: false,
    },
    SubStepColorConfig {
        pattern: "开始",
        color: RGBColor(100, 149, 237),
        use_alternating: false,
    },
];

/// 精确匹配的子步骤颜色
const SUBSTEP_EXACT_COLORS: &[(&str, RGBColor)] = &[
    ("开始执行", RGBColor(100, 149, 237)),    // 矢车菊蓝
    ("发送目标", RGBColor(255, 165, 0)),      // 橙色
    ("执行中", RGBColor(135, 206, 250)),      // 淡天蓝
    ("设置导航目标", RGBColor(70, 130, 180)), // 钢蓝
    ("服务端接受", RGBColor(60, 179, 113)),   // 中海绿
    ("结果回调", RGBColor(147, 112, 219)),    // 中紫色
];

/// 前缀匹配的子步骤颜色
const SUBSTEP_PREFIX_COLORS: &[(&str, RGBColor)] = &[
    ("执行完成", RGBColor(34, 139, 34)),         // 森林绿
    ("发送导航目标", RGBColor(255, 165, 0)),     // 橙色
    ("发送头部控制目标", RGBColor(255, 165, 0)), // 橙色
    ("发送腰部控制目标", RGBColor(255, 165, 0)), // 橙色
    ("动作完成", RGBColor(147, 112, 219)),       // 中紫色
];

/// 默认交替颜色列表
const DEFAULT_ALTERNATING_COLORS: [RGBColor; 6] = [
    RGBColor(144, 238, 144), // 淡绿
    RGBColor(255, 218, 185), // 桃色
    RGBColor(173, 216, 230), // 淡蓝
    RGBColor(255, 182, 193), // 淡粉
    RGBColor(221, 160, 221), // 淡紫
    RGBColor(255, 255, 150), // 淡黄
];

/// 获取子步骤颜色
fn get_substep_color(name: &str, index: usize) -> RGBColor {
    // 精确匹配
    for (pattern, color) in SUBSTEP_EXACT_COLORS {
        if name == *pattern {
            return *color;
        }
    }

    // 前缀匹配
    for (prefix, color) in SUBSTEP_PREFIX_COLORS {
        if name.starts_with(prefix) {
            return *color;
        }
    }

    // 检查模式匹配（包含匹配）
    for config in SUBSTEP_COLOR_CONFIG {
        if name.contains(config.pattern) {
            if config.use_alternating {
                return if index.is_multiple_of(2) {
                    config.color
                } else {
                    RGBColor(176, 224, 230) // 粉蓝
                };
            }
            return config.color;
        }
    }

    // 默认：使用交替颜色增强分隔
    DEFAULT_ALTERNATING_COLORS[index % DEFAULT_ALTERNATING_COLORS.len()]
}

/// 生成所有轮次的甘特图
///
/// # 参数
/// * `flows` - 导航流程切片
/// * `rounds` - 轮次切片
/// * `outdir` - 输出目录
/// * `t0` - 起始时间戳
pub fn generate_gantt_charts(
    flows: &[NavigationFlow],
    rounds: &[Round],
    outdir: &str,
    t0: f64,
) -> Result<()> {
    // 按轮次分组flows
    let mut round_flows: HashMap<usize, Vec<&NavigationFlow>> = HashMap::new();
    for flow in flows {
        round_flows.entry(flow.round_id).or_default().push(flow);
    }

    // 计算需要的数字位数（根据轮次总数）
    let width = crate::utils::digit_width(rounds.len());

    for round in rounds {
        let round_flows_data = round_flows
            .get(&round.id)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        generate_round_gantt(round, round_flows_data, outdir, t0, width)?;
    }

    Ok(())
}

/// 生成单个轮次的甘特图
///
/// # 参数
/// * `round` - 轮次
/// * `flows` - 属于该轮次的导航流程列表
/// * `outdir` - 输出目录
/// * `t0` - 起始时间戳
/// * `width` - 编号宽度（用于前导零）
fn generate_round_gantt(
    round: &Round,
    flows: &[&NavigationFlow],
    outdir: &str,
    _t0: f64,
    width: usize,
) -> Result<()> {
    // 创建字体加载器
    let font_loader = FontLoader::default();

    let _round_start = round.start_ts - _t0;
    let total_duration = round.end_ts.map(|end| end - round.start_ts).unwrap_or(0.0);
    let pause_duration = round.total_pause_duration();
    let effective_duration = round.effective_duration();

    // 计算轮次开始的北京时间
    let round_start_beijing = timestamp_to_beijing_time(round.start_ts);

    // 准备图表数据: (label, detail_info, start, duration, type, sub_steps)
    let mut chart_data = Vec::new();

    for (flow_idx, flow) in flows.iter().enumerate() {
        let flow_id = flow_idx + 1;

        // 添加导航动作
        if let Some(nav_start_ts) = flow.nav_start_ts {
            // 计算压缩后的开始时间（扣除之前的暂停时间）
            let pause_before = pause_time_before(&round.pause_events, round.start_ts, nav_start_ts);
            let nav_start = (nav_start_ts - round.start_ts - pause_before).max(0.0);

            // 结束时间同样映射到压缩坐标系，保证跨暂停的动作不会冲出时间轴
            let nav_duration = compressed_duration(round, nav_start_ts, flow.nav_end_ts);

            let label = format!("F{}-nav", flow_id);
            let detail_info = match flow.nav_target_pos.as_deref() {
                Some(pos) => t!("label.navigation_to", pos = pos).to_string(),
                None => t!("action.navigation").to_string(),
            };

            chart_data.push((
                label,
                detail_info,
                nav_start,
                nav_duration.max(0.0),
                "navigation".to_string(),
                flow.nav_sub_steps.clone(),
            ));
        }

        // 添加其他动作
        for operation in &flow.operations {
            // 如果这个 navigation 已经从 nav_start_ts 添加了，跳过（按时间戳匹配避免重复）
            if operation.action_type == "navigation"
                && let (Some(nav_start), Some(op_start)) = (flow.nav_start_ts, operation.start_ts)
                && (nav_start - op_start).abs() < 0.01
            {
                continue;
            }
            if let Some(op_start_ts) = operation.start_ts {
                // 计算压缩后的开始时间（扣除之前的暂停时间）
                let pause_before =
                    pause_time_before(&round.pause_events, round.start_ts, op_start_ts);
                let op_start = (op_start_ts - round.start_ts - pause_before).max(0.0);

                // 结束时间同样映射到压缩坐标系（扣除跨越的暂停时间）
                let op_duration = compressed_duration(round, op_start_ts, operation.end_ts);

                let (label, detail_info) = match operation.action_type.as_str() {
                    "arm" => {
                        let action_code = operation.action_code.unwrap_or(0);
                        let status_suffix = if operation.end_ts.is_none() {
                            t!("label.not_completed").to_string()
                        } else {
                            String::new()
                        };
                        (
                            format!("F{}-arm-{}", flow_id, action_code),
                            format!(
                                "{}{}",
                                t!("label.arm_action", code = action_code),
                                status_suffix
                            ),
                        )
                    }
                    "head" => (
                        format!("F{}-head", flow_id),
                        t!("label.head_control").to_string(),
                    ),
                    "waist" => (
                        format!("F{}-waist", flow_id),
                        t!("label.waist_control").to_string(),
                    ),
                    "preplan" => {
                        let action_code = operation.action_code.unwrap_or(0);
                        (
                            format!("F{}-preplan-{}", flow_id, action_code),
                            t!("label.preplan_action", code = action_code).to_string(),
                        )
                    }
                    _ => (
                        format!("F{}-{}", flow_id, operation.action_type),
                        operation.label.clone(),
                    ),
                };

                chart_data.push((
                    label,
                    detail_info,
                    op_start.max(0.0),
                    op_duration.max(0.0),
                    operation.action_type.clone(),
                    operation.sub_steps.clone(),
                ));
            }
        }
    }

    if chart_data.is_empty() {
        return Ok(());
    }

    // 按开始时间排序chart_data，确保layer分配算法正确工作
    chart_data.sort_by(|a, b| a.2.total_cmp(&b.2));

    // 去重：移除完全相同的条目（基于label、start、duration、type）
    chart_data.dedup_by(|a, b| {
        a.0 == b.0 && // label
        (a.2 - b.2).abs() < 0.001 && // start (浮点数比较)
        (a.3 - b.3).abs() < 0.001 && // duration
        a.4 == b.4 // type
    });

    // 按动作类型分组并计算每种类型的总耗时
    let mut action_types: Vec<String> = Vec::new();
    let mut action_type_map: HashMap<String, usize> = HashMap::new();
    let mut action_type_durations: HashMap<String, f64> = HashMap::new();

    // 先计算每种动作类型的总耗时
    for (_, _, _, duration, step_type, _) in &chart_data {
        *action_type_durations
            .entry(step_type.clone())
            .or_insert(0.0) += duration;
    }

    // 使用配置表收集所有出现的动作类型并分配Y轴位置
    for (type_key, _, _) in ACTION_TYPE_CONFIG {
        if chart_data.iter().any(|(_, _, _, _, t, _)| t == type_key) {
            action_type_map.insert(type_key.to_string(), action_types.len());
            let total_duration = action_type_durations.get(*type_key).unwrap_or(&0.0);
            let display_name = get_action_type_display(type_key);
            action_types.push(format!(
                "{} ({})",
                display_name,
                t!("gantt.total_time", time = format!("{:.1}", total_duration))
            ));
        }
    }

    // 处理其他未定义的动作类型
    for (_, _, _, _, action_type, _) in &chart_data {
        if !action_type_map.contains_key(action_type) {
            action_type_map.insert(action_type.clone(), action_types.len());
            let total_duration = action_type_durations.get(action_type).unwrap_or(&0.0);
            let display_name = get_action_type_display(action_type);
            action_types.push(format!(
                "{} ({})",
                display_name,
                t!("gantt.total_time", time = format!("{:.1}", total_duration))
            ));
        }
    }

    let filename = format!(
        "{}/round_{:0width$}_gantt.png",
        outdir,
        round.id,
        width = width
    );
    let canvas_width = 14400; // 超高分辨率 (4x)
    let bar_height = 720; // 增加条形高度 (4x)
    let canvas_height = (action_types.len() * bar_height + 1600) as u32;

    let root = BitMapBackend::new(&filename, (canvas_width, canvas_height)).into_drawing_area();
    root.fill(&WHITE)?;

    // 横轴用的是扣除暂停后的压缩坐标，上限也必须取有效时长；
    // 若取墙钟总时长，有暂停的轮次右侧会拖出大片空白，动作全被挤到最左端
    let max_time = effective_duration.max(
        chart_data
            .iter()
            .map(|(_, _, start, dur, _, _)| start + dur)
            .fold(0.0, f64::max),
    );

    // 提取结束时间的时分秒部分（只显示 HH:MM:SS.ffffff）
    let round_end_beijing_short = round
        .end_ts
        .map(|ts| {
            let full = timestamp_to_beijing_time(ts);
            // 格式: "2026-01-13 17:35:02.438866" -> "17:35:02.438866"
            full.split_whitespace().nth(1).unwrap_or(&full).to_string()
        })
        .unwrap_or_else(|| t!("gantt.not_ended").to_string());

    // 构建标题，包含循环类型（去掉层级信息）
    // 如果有暂停时间，显示有效时间和暂停时间
    // 注意：plotters 的 caption 不支持换行符，这里用 " | " 拼接为单行
    let title = if pause_duration > 0.0 {
        format!(
            "{} | {}",
            t!(
                "gantt.title_with_pause",
                cycle_type = round.cycle_type.to_string(),
                id = round.id,
                effective = format!("{:.3}", effective_duration),
                pause = format!("{:.3}", pause_duration),
                total = format!("{:.3}", total_duration)
            ),
            t!(
                "gantt.beijing_time",
                start = &round_start_beijing,
                end = &round_end_beijing_short
            )
        )
    } else {
        format!(
            "{} | {}",
            t!(
                "gantt.title_no_pause",
                cycle_type = round.cycle_type.to_string(),
                id = round.id,
                total = format!("{:.3}", total_duration)
            ),
            t!(
                "gantt.beijing_time",
                start = &round_start_beijing,
                end = &round_end_beijing_short
            )
        )
    };

    let mut chart = ChartBuilder::on(&root)
        .caption(&title, font_loader.font_desc(192))
        .margin(200)
        .x_label_area_size(400)
        .y_label_area_size(800)
        .build_cartesian_2d(0.0..max_time * 1.1, 0.0..(action_types.len() as f64))?;

    // 配置网格和标签（使用 FontLoader 确保中文显示正常）
    let mut mesh = chart.configure_mesh();
    mesh.y_desc(t!("gantt.y_label").to_string())
        .x_desc(t!("gantt.x_label").to_string())
        .axis_desc_style(font_loader.font_desc(96))
        .label_style(font_loader.font_desc(72))
        .y_labels(action_types.len() + 1) // 确保显示所有类型标签
        .y_label_formatter(&|y| {
            let idx = *y as usize;
            if idx < action_types.len() {
                action_types[idx].clone()
            } else {
                String::new()
            }
        })
        .draw()?;

    for (_label, detail_info, start, duration, step_type, sub_steps) in &chart_data {
        let base_color = get_action_color(step_type);

        // 获取该动作类型的Y轴位置
        let y_base = *action_type_map.get(step_type).unwrap() as f64;

        // 所有同类型操作在同一水平线上，按时间横向排列
        let y_height = 0.6; // 条形高度
        let y_pos = y_base;

        // 绘制主方块
        chart.draw_series(std::iter::once(Rectangle::new(
            [
                (*start, y_pos + 0.15),
                (*start + *duration, y_pos + y_height + 0.15),
            ],
            base_color.mix(0.3).filled(),
        )))?;

        // 绘制子步骤
        if !sub_steps.is_empty() && *duration > 0.0 {
            draw_sub_steps(
                &mut chart,
                SubStepDrawParams {
                    sub_steps,
                    round,
                    start: *start,
                    duration: *duration,
                    sub_y_start: y_pos + 0.15,
                    sub_y_height: y_height,
                },
            )?;
        }

        // 在动作起点添加时间标注
        draw_time_label(&mut chart, &font_loader, *start, *duration, y_pos, y_height)?;

        // 在主方块顶部添加主标签
        draw_main_label(
            &mut chart,
            &font_loader,
            detail_info,
            *start,
            *duration,
            y_pos,
            step_type,
        )?;
    }

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .draw()?;
    root.present()?;

    // 单张甘特图不打印保存消息，避免刷屏；改为在分析结束时汇总显示
    Ok(())
}

/// 子步骤绘制参数
struct SubStepDrawParams<'a> {
    sub_steps: &'a [SubStep],
    round: &'a Round,
    start: f64,
    duration: f64,
    sub_y_start: f64,
    sub_y_height: f64,
}

/// 绘制子步骤
#[allow(clippy::too_many_arguments)]
fn draw_sub_steps<DB: DrawingBackend>(
    chart: &mut ChartContext<
        DB,
        Cartesian2d<plotters::coord::types::RangedCoordf64, plotters::coord::types::RangedCoordf64>,
    >,
    params: SubStepDrawParams<'_>,
) -> Result<()>
where
    DB::ErrorType: 'static,
{
    let SubStepDrawParams {
        sub_steps,
        round,
        start,
        duration,
        sub_y_start,
        sub_y_height,
    } = params;
    // 绘制每个子步骤之间的时间段
    for i in 0..sub_steps.len() {
        let sub_start_ts = sub_steps[i].timestamp;
        let sub_start_abs = (sub_start_ts - round.start_ts).max(0.0);

        // 确定子步骤的结束时间（绝对坐标）
        let sub_end_abs = if i + 1 < sub_steps.len() {
            (sub_steps[i + 1].timestamp - round.start_ts).max(0.0)
        } else {
            start + duration // 最后一个子步骤延伸到主动作结束
        };

        // 将子步骤限制在主方块的范围内
        let sub_start_clamped = sub_start_abs.max(start).min(start + duration);
        let sub_end_clamped = sub_end_abs.max(start).min(start + duration);

        let sub_duration = sub_end_clamped - sub_start_clamped;

        if sub_duration > 0.0 {
            // 根据子步骤名称获取颜色
            let sub_color = get_substep_color(&sub_steps[i].name, i);

            // 绘制子步骤方块（只在主方块范围内）
            chart.draw_series(std::iter::once(Rectangle::new(
                [
                    (sub_start_clamped, sub_y_start),
                    (sub_end_clamped, sub_y_start + sub_y_height),
                ],
                sub_color.filled(),
            )))?;

            // 绘制边框分隔线（黑色边框增强分隔效果）
            chart.draw_series(std::iter::once(Rectangle::new(
                [
                    (sub_start_clamped, sub_y_start),
                    (sub_end_clamped, sub_y_start + sub_y_height),
                ],
                BLACK.stroke_width(2),
            )))?;

            // 子步骤图块内不绘制文字
        }
    }

    Ok(())
}

/// 绘制时间标注（居中于色块上方）
fn draw_time_label<DB: DrawingBackend>(
    chart: &mut ChartContext<
        DB,
        Cartesian2d<plotters::coord::types::RangedCoordf64, plotters::coord::types::RangedCoordf64>,
    >,
    font_loader: &FontLoader,
    start: f64,
    duration: f64,
    y_pos: f64,
    y_height: f64,
) -> Result<()>
where
    DB::ErrorType: 'static,
{
    let start_time_text = format!("{:.1}s", start);
    let text_x = start + duration / 2.0;
    chart.draw_series(std::iter::once(Text::new(
        start_time_text,
        (text_x, y_pos + y_height + 0.25),
        font_loader
            .font_desc(56) // 4x 分辨率
            .color(&BLACK)
            .pos(Pos::new(HPos::Center, VPos::Top))
            .transform(FontTransform::None),
    )))?;

    Ok(())
}

/// 构建消耗时间文本（图块下方显示，统一秒为单位，保留三位小数）
fn build_duration_text(duration: f64) -> String {
    format!("{:.3}s", duration)
}

/// 绘制消耗时间标签（图块下方）
fn draw_main_label<DB: DrawingBackend>(
    chart: &mut ChartContext<
        DB,
        Cartesian2d<plotters::coord::types::RangedCoordf64, plotters::coord::types::RangedCoordf64>,
    >,
    font_loader: &FontLoader,
    _detail_info: &str,
    start: f64,
    duration: f64,
    y_pos: f64,
    _step_type: &str,
) -> Result<()>
where
    DB::ErrorType: 'static,
{
    let text_x = start + duration / 2.0;

    // 消耗时间：图块下方
    let duration_text = build_duration_text(duration);
    chart.draw_series(std::iter::once(Text::new(
        duration_text,
        (text_x, y_pos + 0.10),
        font_loader
            .font_desc(56)
            .color(&BLACK)
            .pos(Pos::new(HPos::Center, VPos::Top))
            .transform(FontTransform::None),
    )))?;

    Ok(())
}

/// 生成常规循环耗时统计图
///
/// 横坐标是每个已完成的常规循环序号，纵坐标是对应的有效耗时（扣除暂停时间）
/// 同时绘制平均时间线，并生成对应的 CSV 统计文件
///
/// # 参数
/// * `rounds` - 轮次切片
/// * `flows` - 导航流程切片（用于获取动作级别的暂停时间）
/// * `outdir` - 输出目录
pub fn generate_cycle_duration_chart(
    rounds: &[Round],
    flows: &[NavigationFlow],
    outdir: &str,
) -> Result<()> {
    use crate::models::CycleType;

    let font_loader = FontLoader::default();

    // 筛选已完成的常规循环
    let completed_normal_cycles: Vec<_> = rounds
        .iter()
        .filter(|r| matches!(r.cycle_type, CycleType::Normal(_)) && r.end_ts.is_some())
        .collect();

    if completed_normal_cycles.is_empty() {
        eprintln!("{}", t!("stats.no_cycles"));
        return Ok(());
    }

    // 计算每个循环的时间数据：(总时间, 暂停时间, 有效时间)
    // 注意：暂停时间需要同时统计轮次级别和动作级别的暂停
    let time_data: Vec<(f64, f64, f64)> = completed_normal_cycles
        .iter()
        .map(|r| crate::statistics::round_timing(r, flows))
        .collect();

    // 提取有效耗时用于绘图
    let durations: Vec<f64> = time_data.iter().map(|(_, _, eff)| *eff).collect();

    let cycle_count = durations.len();
    let avg_duration = durations.iter().sum::<f64>() / cycle_count as f64;
    let max_duration = durations.iter().cloned().fold(0.0_f64, f64::max);
    let min_duration = durations.iter().cloned().fold(f64::MAX, f64::min);

    // 计算总暂停时间
    let total_pause: f64 = time_data.iter().map(|(_, pause, _)| *pause).sum();

    // 生成 CSV 统计文件
    generate_cycle_duration_csv(&completed_normal_cycles, &time_data, outdir)?;

    // 图表尺寸
    let width = 1600u32;
    let height = 900u32;

    let file_path = format!("{}/cycle_duration_stats.png", outdir);
    let root = BitMapBackend::new(&file_path, (width, height)).into_drawing_area();
    root.fill(&WHITE)?;

    // 标题：明确说明是有效耗时（扣除暂停时间）
    let title = if total_pause > 0.0 {
        t!(
            "stats.title_with_pause",
            count = cycle_count,
            avg = format!("{:.2}", avg_duration),
            pause = format!("{:.2}", total_pause)
        )
        .to_string()
    } else {
        t!(
            "stats.title_no_pause",
            count = cycle_count,
            avg = format!("{:.2}", avg_duration)
        )
        .to_string()
    };

    // Y轴范围，留出一些余量
    let y_max = (max_duration * 1.15).max(avg_duration * 1.3);
    let y_min = 0.0;

    let mut chart = ChartBuilder::on(&root)
        .caption(&title, font_loader.font_desc(28).color(&BLACK))
        .margin(20)
        .x_label_area_size(50)
        .y_label_area_size(80)
        .build_cartesian_2d(0.5..(cycle_count as f64 + 0.5), y_min..y_max)?;

    chart
        .configure_mesh()
        .x_labels(cycle_count.min(20))
        .x_label_formatter(&|x| {
            let idx = *x as usize;
            if idx >= 1 && idx <= cycle_count {
                format!("{}", idx)
            } else {
                String::new()
            }
        })
        .y_label_formatter(&|y| format!("{:.1}s", y))
        .x_desc(t!("stats.cycle_number").to_string())
        .y_desc(t!("stats.duration").to_string())
        .axis_desc_style(font_loader.font_desc(18).color(&BLACK))
        .label_style(font_loader.font_desc(14).color(&BLACK))
        .draw()?;

    // 绘制柱状图
    let bar_width = 0.6;
    chart.draw_series(durations.iter().enumerate().map(|(i, &duration)| {
        let x = (i + 1) as f64;
        let color = if duration > avg_duration * 1.2 {
            RGBColor(255, 100, 100) // 超过平均20%显示红色
        } else if duration < avg_duration * 0.8 {
            RGBColor(100, 200, 100) // 低于平均20%显示绿色
        } else {
            RGBColor(100, 150, 230) // 正常显示蓝色
        };
        Rectangle::new(
            [(x - bar_width / 2.0, 0.0), (x + bar_width / 2.0, duration)],
            color.filled(),
        )
    }))?;

    // 在柱状图上方显示具体数值
    for (i, &duration) in durations.iter().enumerate() {
        let x = (i + 1) as f64;
        chart.draw_series(std::iter::once(Text::new(
            format!("{:.1}", duration),
            (x, duration + y_max * 0.02),
            font_loader
                .font_desc(12)
                .color(&BLACK)
                .pos(Pos::new(HPos::Center, VPos::Bottom)),
        )))?;
    }

    // 绘制平均线
    chart.draw_series(std::iter::once(PathElement::new(
        vec![
            (0.5, avg_duration),
            (cycle_count as f64 + 0.5, avg_duration),
        ],
        ShapeStyle {
            color: RED.mix(0.8).to_rgba(),
            filled: false,
            stroke_width: 2,
        },
    )))?;

    // 平均线标签
    chart.draw_series(std::iter::once(Text::new(
        t!("stats.average", time = format!("{:.2}", avg_duration)).to_string(),
        (cycle_count as f64 + 0.3, avg_duration),
        font_loader
            .font_desc(16)
            .color(&RED)
            .pos(Pos::new(HPos::Right, VPos::Center)),
    )))?;

    // 添加统计信息
    let stats_text = t!(
        "stats.max_min_diff",
        max = format!("{:.2}", max_duration),
        min = format!("{:.2}", min_duration),
        diff = format!("{:.2}", max_duration - min_duration)
    )
    .to_string();
    root.draw(&Text::new(
        stats_text,
        (width as i32 / 2, height as i32 - 15),
        font_loader
            .font_desc(16)
            .color(&BLACK)
            .pos(Pos::new(HPos::Center, VPos::Bottom)),
    ))?;

    root.present()?;

    Ok(())
}

/// 生成常规循环耗时统计 CSV 文件
///
/// # 参数
/// * `cycles` - 已完成的常规循环列表
/// * `time_data` - 每个循环的时间数据 (总时间, 暂停时间, 有效时间)
/// * `outdir` - 输出目录
fn generate_cycle_duration_csv(
    cycles: &[&Round],
    time_data: &[(f64, f64, f64)],
    outdir: &str,
) -> Result<()> {
    let csv_path = format!("{}/cycle_duration_stats.csv", outdir);
    let mut wtr = csv::Writer::from_path(&csv_path)?;

    // 写入表头
    wtr.write_record([
        t!("csv.seq_num").to_string(),
        t!("csv.cycle_type").to_string(),
        t!("csv.round_id").to_string(),
        t!("csv.total_time").to_string(),
        t!("csv.pause_time").to_string(),
        t!("csv.effective_time").to_string(),
        t!("csv.pause_ratio").to_string(),
    ])?;

    // 写入每个循环的数据
    for (i, (round, (total, pause, effective))) in cycles.iter().zip(time_data.iter()).enumerate() {
        let cycle_name = round.cycle_type.to_string();

        let pause_ratio = if *total > 0.0 {
            pause / total * 100.0
        } else {
            0.0
        };

        wtr.write_record(&[
            (i + 1).to_string(),
            cycle_name,
            round.id.to_string(),
            format!("{:.3}", total),
            format!("{:.3}", pause),
            format!("{:.3}", effective),
            format!("{:.1}", pause_ratio),
        ])?;
    }

    // 写入汇总行
    let total_sum: f64 = time_data.iter().map(|(t, _, _)| t).sum();
    let pause_sum: f64 = time_data.iter().map(|(_, p, _)| p).sum();
    let effective_sum: f64 = time_data.iter().map(|(_, _, e)| e).sum();
    let avg_effective = effective_sum / cycles.len() as f64;
    let pause_ratio_sum = if total_sum > 0.0 {
        pause_sum / total_sum * 100.0
    } else {
        0.0
    };

    wtr.write_record(&[
        t!("csv.total").to_string(),
        t!("csv.total_cycles", count = cycles.len()).to_string(),
        "-".to_string(),
        format!("{:.3}", total_sum),
        format!("{:.3}", pause_sum),
        format!("{:.3}", effective_sum),
        format!("{:.1}", pause_ratio_sum),
    ])?;

    wtr.write_record(&[
        t!("csv.average").to_string(),
        "-".to_string(),
        "-".to_string(),
        format!("{:.3}", total_sum / cycles.len() as f64),
        format!("{:.3}", pause_sum / cycles.len() as f64),
        format!("{:.3}", avg_effective),
        "-".to_string(),
    ])?;

    wtr.flush()?;

    Ok(())
}
