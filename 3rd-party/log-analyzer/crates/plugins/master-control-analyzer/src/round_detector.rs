//! 轮次检测模块
//!
//! 本模块负责从日志中检测任务轮次
//!
//! 日志格式（兼容多代格式）：
//! - 初始循环开始:
//!   - 新: `[初始循环] ===== 初始填充开始，已上件工位数 N =====`
//!   - 旧: `[初始循环] ===== 初始循环开始（气密设备为空）=====`
//! - 初始循环完成:
//!   - 最新: `[初始循环] 初始循环结束，扫码成功=true, 气密位状态=N`
//!   - 新: `[初始循环] 初始填充完成，已上件工位数 N`
//!   - 旧: `[初始循环] 初始循环完成，气密设备中有工件`
//! - 常规循环开始:
//!   - 新: `[常规循环] 常规循环 N，目标工位 M`
//!   - 旧: `[常规循环] 常规循环 N`
//! - 常规循环完成: `[常规循环] 常规循环 N 放置完成`（未变）
//! - 收尾/最终循环开始:
//!   - 新: `[收尾前校正] ...`
//!   - 旧: `[最终循环] 最终循环：取出气密工件并放置`
//! - 收尾/最终循环完成:
//!   - 最新: `[收尾循环] 最终循环完成`
//!   - 新: `[收尾循环] 双工位收尾完成`
//!   - 旧: `[最终循环] 最终循环完成`
//!
//! 注意：LogNode 节点注册时会在 log_node.cpp:39 输出一行
//! `LogNode[xxx] - 初始化完成: level='...', message='...'` 形式的模板日志。
//! 最新格式中部分模板不含 `{variable}` 占位符（如 `[收尾循环] 最终循环完成`），
//! 与真实事件文本完全相同，无法靠内容正则区分，因此统一用
//! [`LOG_NODE_REGISTRATION_REGEX`] 在检测前跳过整行；含占位符的旧模板
//! 仍额外通过强制要求 `\d+` 等具体值来双重防护。

use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;

use crate::models::{CycleType, LogLine, PauseEvent, Round};

// ===== 循环检测正则 =====
//
// 所有正则都必须用 `\d+` 等具体值形式，避免匹配 LogNode 注册行中的
// `{variable}` 占位符（那一行与真实事件长得像，但 `log_node.cpp:39`，
// 真实事件在 `log_node.cpp:62`）。

/// LogNode 模板注册行: `LogNode[xxx] - 初始化完成: level='...', message='...'`
///
/// 最新格式中部分模板正文不含占位符，与真实事件文本一字不差，
/// 必须在进入任何事件检测前跳过整行。
static LOG_NODE_REGISTRATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"LogNode\[[^\]]+\]\s*-\s*初始化完成:")
        .expect("invalid regex: LOG_NODE_REGISTRATION_REGEX")
});

/// 判断是否为 LogNode 模板注册行（非真实事件，检测时应整行跳过）
pub(crate) fn is_log_node_registration(line: &str) -> bool {
    LOG_NODE_REGISTRATION_REGEX.is_match(line)
}

/// 初始循环开始（新旧格式并存）
static INIT_START_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\[初始循环\]\s*=+\s*(?:初始填充开始，已上件工位数\s+\d+|初始循环开始（气密设备为空）)\s*=+",
    )
    .expect("invalid regex: INIT_START_REGEX")
});

/// 初始循环完成（多代格式并存）:
///   - 最新: `[初始循环] 初始循环结束，扫码成功=true, 气密位状态=N`
///     （要求 `扫码成功=` 后跟具体布尔值，避开模板行的 `{init_3001_success}` 占位符）
///   - 新: `[初始循环] 初始填充完成，已上件工位数 N`
///   - 旧: `[初始循环] 初始循环完成，气密设备中有工件`
static INIT_END_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\[初始循环\]\s*(?:初始循环结束，扫码成功=(?:true|false)|初始填充完成，已上件工位数\s+\d+|初始循环完成，气密设备中有工件)",
    )
    .expect("invalid regex: INIT_END_REGEX")
});

/// 常规循环开始:
///   - 新: `[常规循环] 常规循环 N，目标工位 M`
///   - 旧: `[常规循环] 常规循环 N`（行尾无后缀）
static NORMAL_START_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[常规循环\]\s*常规循环\s*(\d+)(?:，目标工位\s+\d+|\s*$)")
        .expect("invalid regex: NORMAL_START_REGEX")
});

/// 常规循环完成: [常规循环] 常规循环 N 放置完成
static NORMAL_END_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[常规循环\]\s*常规循环\s*(\d+)\s*放置完成")
        .expect("invalid regex: NORMAL_END_REGEX")
});

/// 收尾/最终循环开始:
///   - 新: `[收尾前校正] loaded_leak_station_count=N, ...`（收尾阶段唯一入口）
///   - 旧: `[最终循环] 最终循环：取出气密工件并放置`
///
/// 必须要求 `loaded_leak_station_count=\d+`，才能避开：
///   - `log_node.cpp:39` 打印的 `message='[收尾前校正] loaded_leak_station_count={...}'`
///     模板注册行（占位符 `{...}` 非 `\d+`）
///   - `[收尾前校正] loaded_leak_station_count 与 put_poses 剩余数量不一致`
///     警告行（`count` 与 `与` 之间无 `=`）
static FINAL_START_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\[收尾前校正\]\s+loaded_leak_station_count=\d+|\[最终循环\]\s*最终循环：取出气密工件并放置",
    )
    .expect("invalid regex: FINAL_START_REGEX")
});

/// 收尾/最终循环完成:
///   - 最新: `[收尾循环] 最终循环完成`
///     （模板注册行含一字不差的同文，依赖 [`is_log_node_registration`] 前置过滤）
///   - 新: `[收尾循环] 双工位收尾完成`
///   - 旧: `[最终循环] 最终循环完成`
static FINAL_END_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[收尾循环\]\s*(?:双工位收尾完成|最终循环完成)|\[最终循环\]\s*最终循环完成")
        .expect("invalid regex: FINAL_END_REGEX")
});

// ===== 姿态信息正则 =====

/// 姿态字符串: [master_control]: 姿态字符串: {...}
static POSE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[master_control\]:\s*姿态字符串:\s*(\{.*\})").expect("invalid regex: POSE_REGEX")
});

// ===== 暂停/恢复检测正则 =====

/// 暂停检测模式 1: PauseTaskNode[...]: 请求暂停任务，等待操作员 RESUME
static PAUSE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"PauseTaskNode\[.*?\]:\s*请求暂停任务，等待操作员 RESUME")
        .expect("invalid regex: PAUSE_REGEX")
});

/// 恢复检测模式 1: PauseTaskNode[...]: 任务已恢复，继续执行
static RESUME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"PauseTaskNode\[.*?\]:\s*任务已恢复，继续执行")
        .expect("invalid regex: RESUME_REGEX")
});

/// 暂停检测模式 2: TaskGraphExecutor: 节点 ... 失败，进入失败暂停状态
static FAIL_PAUSE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"TaskGraphExecutor:\s*节点\s+\S+\s+失败，进入失败暂停状态")
        .expect("invalid regex: FAIL_PAUSE_REGEX")
});

/// 恢复检测模式 2: TaskGraphExecutor: 重试节点 / 跳过节点（视为成功）
/// 失败暂停可由「重试」或「跳过」结束，二者都应扣除等待时间
static RETRY_RESUME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"TaskGraphExecutor:\s*(?:重试节点|跳过节点)\s+\S+")
        .expect("invalid regex: RETRY_RESUME_REGEX")
});

/// 暂停检测模式 3: ROS2ActionAdapter[xxx] - 暂停（动作被暂停）
static ADAPTER_PAUSE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"ROS2ActionAdapter\[\w+\]\s*-\s*暂停").expect("invalid regex: ADAPTER_PAUSE_REGEX")
});

/// 恢复检测模式 3: ROS2ActionAdapter[xxx] - 恢复（动作恢复执行）
static ADAPTER_RESUME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"ROS2ActionAdapter\[\w+\]\s*-\s*恢复").expect("invalid regex: ADAPTER_RESUME_REGEX")
});

/// 暂停检测模式 4: TaskGraphExecutor: 用户请求暂停任务 xxx
static USER_PAUSE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"TaskGraphExecutor:\s*用户请求暂停任务\s+\S+")
        .expect("invalid regex: USER_PAUSE_REGEX")
});

/// 恢复检测模式 4: TaskGraphExecutor: 恢复任务 xxx
static USER_RESUME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"TaskGraphExecutor:\s*恢复任务\s+\S+").expect("invalid regex: USER_RESUME_REGEX")
});

/// 统计筛选也识别尚未恢复的暂停；复用实际解析模式并排除模板注册行。
pub(super) fn is_pause_marker(line: &str) -> bool {
    !is_log_node_registration(line)
        && (PAUSE_REGEX.is_match(line)
            || FAIL_PAUSE_REGEX.is_match(line)
            || ADAPTER_PAUSE_REGEX.is_match(line)
            || USER_PAUSE_REGEX.is_match(line))
}

/// 结束当前轮次并推入列表
fn finalize_current_round(current: &mut Option<Round>, rounds: &mut Vec<Round>, end_ts: f64) {
    if let Some(mut round) = current.take() {
        if round.end_ts.is_none() {
            round.end_ts = Some(end_ts);
        }
        rounds.push(round);
    }
}

/// 创建新轮次
fn create_round(rounds: &[Round], cycle_type: CycleType, loop_number: u32, start_ts: f64) -> Round {
    Round {
        id: rounds.len() + 1,
        loop_number: Some(loop_number),
        cycle_type,
        layer_index: 0,
        start_ts,
        end_ts: None,
        pose0: None,
        pose6: None,
        pause_events: Vec::new(),
    }
}

/// 检测日志中的任务轮次
///
/// 基于循环标记自动检测任务轮次，支持三种循环类型：
/// - 初始循环：气密设备为空时执行
/// - 常规循环：迭代执行的主循环（可执行N次）
/// - 最终循环：取出气密工件并放置
///
/// # 参数
/// * `lines` - 日志行切片
/// * `t_last` - 最后一行日志的时间戳
///
/// # 返回
/// 包含所有检测到的轮次的向量
pub fn detect_rounds(lines: &[LogLine], t_last: f64) -> Result<Vec<Round>> {
    // 引用静态预编译正则
    let init_start_regex = &*INIT_START_REGEX;
    let init_end_regex = &*INIT_END_REGEX;
    let normal_start_regex = &*NORMAL_START_REGEX;
    let normal_end_regex = &*NORMAL_END_REGEX;
    let final_start_regex = &*FINAL_START_REGEX;
    let final_end_regex = &*FINAL_END_REGEX;
    let pose_regex = &*POSE_REGEX;
    let pause_regex = &*PAUSE_REGEX;
    let resume_regex = &*RESUME_REGEX;
    let fail_pause_regex = &*FAIL_PAUSE_REGEX;
    let retry_resume_regex = &*RETRY_RESUME_REGEX;
    let adapter_pause_regex = &*ADAPTER_PAUSE_REGEX;
    let adapter_resume_regex = &*ADAPTER_RESUME_REGEX;
    let user_pause_regex = &*USER_PAUSE_REGEX;
    let user_resume_regex = &*USER_RESUME_REGEX;

    let mut rounds = Vec::new();
    let mut current: Option<Round> = None;
    let mut pending_pause_ts: Option<f64> = None; // 待匹配恢复的暂停时间戳（模式1）
    let mut pending_fail_pause_ts: Option<f64> = None; // 待匹配恢复的失败暂停时间戳（模式2）
    let mut pending_adapter_pause_ts: Option<f64> = None; // 待匹配恢复的动作暂停时间戳（模式3）
    let mut pending_user_pause_ts: Option<f64> = None; // 待匹配恢复的用户暂停时间戳（模式4）

    for line in lines {
        // 跳过 LogNode 模板注册行：其 message 与真实事件文本可能一字不差
        if is_log_node_registration(&line.line) {
            continue;
        }

        // 检测初始循环开始
        if init_start_regex.is_match(&line.line) {
            finalize_current_round(&mut current, &mut rounds, line.timestamp);
            current = Some(create_round(&rounds, CycleType::Initial, 0, line.timestamp));
            continue;
        }

        // 检测初始循环完成
        if init_end_regex.is_match(&line.line) {
            if let Some(ref mut round) = current
                && matches!(round.cycle_type, CycleType::Initial)
            {
                round.end_ts = Some(line.timestamp);
                rounds.push(current.take().unwrap());
            }
            continue;
        }

        // 检测常规循环开始
        if let Some(caps) = normal_start_regex.captures(&line.line) {
            let cycle_number = caps[1].parse::<u32>().unwrap_or(1);
            finalize_current_round(&mut current, &mut rounds, line.timestamp);
            current = Some(create_round(
                &rounds,
                CycleType::Normal(cycle_number),
                cycle_number,
                line.timestamp,
            ));
            continue;
        }

        // 检测常规循环完成
        if let Some(caps) = normal_end_regex.captures(&line.line) {
            let cycle_number = caps[1].parse::<u32>().unwrap_or(1);
            if let Some(ref mut round) = current
                && matches!(round.cycle_type, CycleType::Normal(n) if n == cycle_number)
            {
                round.end_ts = Some(line.timestamp);
                rounds.push(current.take().unwrap());
            }
            continue;
        }

        // 检测最终/收尾循环开始
        // 幂等：如果已在 Final 轮次中，忽略后续的重复起始标记（新日志中
        // `[收尾前校正]` 可能伴随多条警告/模板行，全部归入同一个收尾轮次）
        if final_start_regex.is_match(&line.line) {
            if !matches!(
                current.as_ref().map(|r| &r.cycle_type),
                Some(CycleType::Final)
            ) {
                finalize_current_round(&mut current, &mut rounds, line.timestamp);
                current = Some(create_round(&rounds, CycleType::Final, 999, line.timestamp));
            }
            continue;
        }

        // 检测最终循环完成
        if final_end_regex.is_match(&line.line) {
            if let Some(ref mut round) = current
                && matches!(round.cycle_type, CycleType::Final)
            {
                round.end_ts = Some(line.timestamp);
                rounds.push(current.take().unwrap());
            }
            continue;
        }

        // 检测暂停事件（模式1: PauseTaskNode）
        if pause_regex.is_match(&line.line) {
            pending_pause_ts = Some(line.timestamp);
            continue;
        }

        // 检测恢复事件（模式1: PauseTaskNode）
        if resume_regex.is_match(&line.line) {
            if let Some(pause_ts) = pending_pause_ts.take() {
                // 将暂停事件添加到当前轮次
                if let Some(ref mut round) = current {
                    round.pause_events.push(PauseEvent {
                        pause_ts,
                        resume_ts: Some(line.timestamp),
                    });
                }
            }
            continue;
        }

        // 检测失败暂停事件（模式2: 失败暂停状态）
        if fail_pause_regex.is_match(&line.line) {
            pending_fail_pause_ts = Some(line.timestamp);
            continue;
        }

        // 检测重试恢复事件（模式2: 重试节点）
        if retry_resume_regex.is_match(&line.line) {
            if let Some(pause_ts) = pending_fail_pause_ts.take() {
                // 将暂停事件添加到当前轮次
                if let Some(ref mut round) = current {
                    round.pause_events.push(PauseEvent {
                        pause_ts,
                        resume_ts: Some(line.timestamp),
                    });
                }
            }
            continue;
        }

        // 检测动作暂停事件（模式3: ROS2ActionAdapter 暂停）
        if adapter_pause_regex.is_match(&line.line) {
            pending_adapter_pause_ts = Some(line.timestamp);
            continue;
        }

        // 检测动作恢复事件（模式3: ROS2ActionAdapter 恢复）
        if adapter_resume_regex.is_match(&line.line) {
            if let Some(pause_ts) = pending_adapter_pause_ts.take() {
                // 将暂停事件添加到当前轮次
                if let Some(ref mut round) = current {
                    round.pause_events.push(PauseEvent {
                        pause_ts,
                        resume_ts: Some(line.timestamp),
                    });
                }
            }
            continue;
        }

        // 检测用户暂停事件（模式4: 用户请求暂停任务）
        if user_pause_regex.is_match(&line.line) {
            pending_user_pause_ts = Some(line.timestamp);
            continue;
        }

        // 检测用户恢复事件（模式4: 恢复任务）
        if user_resume_regex.is_match(&line.line) {
            if let Some(pause_ts) = pending_user_pause_ts.take() {
                // 将暂停事件添加到当前轮次
                if let Some(ref mut round) = current {
                    round.pause_events.push(PauseEvent {
                        pause_ts,
                        resume_ts: Some(line.timestamp),
                    });
                }
            }
            continue;
        }

        // 收集姿态信息
        if let Some(ref mut round) = current
            && let Some(caps) = pose_regex.captures(&line.line)
        {
            if round.pose0.is_none() {
                round.pose0 = Some(caps[1].to_string());
            } else if round.pose6.is_none() {
                round.pose6 = Some(caps[1].to_string());
            }
        }
    }

    // 最后一轮处理
    if let Some(mut round) = current {
        if round.end_ts.is_none() {
            round.end_ts = Some(t_last);
        }
        rounds.push(round);
    }

    Ok(rounds)
}

/// 将时间戳转换为对应的轮次ID
///
/// # 参数
/// * `ts` - 时间戳
/// * `rounds` - 轮次切片
///
/// # 返回
/// 对应的轮次ID，如果没有找到则返回0或最后一个轮次的ID
pub fn ts_to_round_id(ts: f64, rounds: &[Round]) -> usize {
    // 查找时间戳落在哪个轮次内
    rounds
        .iter()
        .find(|r| ts >= r.start_ts && ts < r.end_ts.unwrap_or(f64::INFINITY))
        .map(|r| r.id)
        .or_else(|| {
            // 如果在最后一个轮次之后，返回最后一个轮次的 ID
            rounds
                .last()
                .filter(|r| ts >= r.end_ts.unwrap_or(0.0))
                .map(|r| r.id)
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实日志行前缀（log_node.cpp:62 为真实事件，:39 为模板注册）
    fn event(timestamp: f64, text: &str) -> LogLine {
        LogLine {
            timestamp,
            line: format!(
                "2026-09-01 11:29:58.036536 [master_control] [0xD9C957DE] [INFO] [log_node.cpp:62] - {}",
                text
            ),
        }
    }

    /// LogNode 模板注册行（不是真实事件）
    fn registration(timestamp: f64, node: &str, message: &str) -> LogLine {
        LogLine {
            timestamp,
            line: format!(
                "2026-09-01 11:16:01.211405 [master_control] [0x05069E01] [INFO] [log_node.cpp:39] - LogNode[{}] - 初始化完成: level='info', message='{}'",
                node, message
            ),
        }
    }

    #[test]
    fn detects_current_init_cycle_markers() {
        let lines = vec![
            event(0.0, "[初始循环] ===== 初始填充开始，已上件工位数 0 ====="),
            event(60.0, "[初始循环] 初始循环结束，扫码成功=true, 气密位状态=2"),
        ];

        let rounds = detect_rounds(&lines, 60.0).expect("detect rounds");
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].cycle_type, CycleType::Initial);
        assert_eq!(rounds[0].end_ts, Some(60.0));
    }

    #[test]
    fn detects_current_final_cycle_end_marker() {
        let lines = vec![
            event(0.0, "[收尾前校正] loaded_leak_station_count=2"),
            event(30.0, "[收尾循环] 最终循环完成"),
        ];

        let rounds = detect_rounds(&lines, 30.0).expect("detect rounds");
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].cycle_type, CycleType::Final);
        assert_eq!(rounds[0].end_ts, Some(30.0));
    }

    #[test]
    fn ignores_log_node_registration_lines() {
        // 注册行的 message 与真实事件文本一字不差，必须整行跳过
        let lines = vec![
            registration(0.0, "log_final_complete", "[收尾循环] 最终循环完成"),
            registration(
                0.1,
                "log_init_complete",
                "[初始循环] 初始循环结束，扫码成功={init_3001_success}, 气密位状态={obj_in_leak_status}",
            ),
            registration(
                0.2,
                "loop_normal_start",
                "[常规循环] 常规循环 {iteration}，目标工位 {current_leak_station}",
            ),
        ];

        let rounds = detect_rounds(&lines, 0.2).expect("detect rounds");
        assert!(
            rounds.is_empty(),
            "模板注册行不应产生轮次，实际得到 {} 个",
            rounds.len()
        );
    }

    #[test]
    fn registration_line_does_not_close_real_round() {
        // 真实轮次进行中出现同文注册行，不得被提前结束
        let lines = vec![
            event(0.0, "[常规循环] 常规循环 1，目标工位 1"),
            registration(10.0, "log_final_complete", "[收尾循环] 最终循环完成"),
            event(70.0, "[常规循环] 常规循环 1 放置完成"),
        ];

        let rounds = detect_rounds(&lines, 70.0).expect("detect rounds");
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].cycle_type, CycleType::Normal(1));
        assert_eq!(rounds[0].end_ts, Some(70.0));
    }

    #[test]
    fn merges_duplicate_pause_detections_in_round() {
        // 失败暂停同时触发 TaskGraphExecutor 与 ROS2ActionAdapter 两种模式
        let lines = vec![
            event(0.0, "[常规循环] 常规循环 1，目标工位 1"),
            LogLine {
                timestamp: 10.0,
                line: "TaskGraphExecutor: 节点 normal_nav_to_put_direct 失败，进入失败暂停状态"
                    .to_string(),
            },
            LogLine {
                timestamp: 10.1,
                line: "ROS2ActionAdapter[head_control] - 暂停".to_string(),
            },
            LogLine {
                timestamp: 269.0,
                line: "TaskGraphExecutor: 重试节点 normal_nav_to_put_direct".to_string(),
            },
            LogLine {
                timestamp: 269.5,
                line: "ROS2ActionAdapter[head_control] - 恢复".to_string(),
            },
            event(328.5, "[常规循环] 常规循环 1 放置完成"),
        ];

        let rounds = detect_rounds(&lines, 328.5).expect("detect rounds");
        assert_eq!(rounds.len(), 1);
        assert_eq!(rounds[0].pause_events.len(), 2);
        // 两段重叠暂停合并后为 10.0..269.5，共 259.5 秒
        assert!((rounds[0].total_pause_duration() - 259.5).abs() < 1e-9);
        assert!((rounds[0].effective_duration() - 69.0).abs() < 1e-9);
    }
}
