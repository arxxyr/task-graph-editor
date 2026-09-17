//! 日志解析模块
//!
//! 本模块负责从日志文件中提取带时间戳的日志行

use std::fs;
use std::sync::LazyLock;

use anyhow::Result;
use chrono::{NaiveDateTime, TimeZone};
use chrono_tz::Asia::Shanghai;
use regex::Regex;

use crate::models::LogLine;

/// 新格式时间戳：YYYY-MM-DD HH:MM:SS.microseconds
static NEW_TS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2}\.\d+)")
        .expect("invalid regex: NEW_TS_REGEX")
});
/// 旧格式时间戳：[INFO/WARN/ERROR/DEBUG] [timestamp]
static OLD_TS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:INFO|WARN|ERROR|DEBUG)\]\s*\[(\d{9,}\.\d+)\]")
        .expect("invalid regex: OLD_TS_REGEX")
});

/// 将日期时间字符串转换为 Unix 时间戳
///
/// # 参数
/// * `datetime_str` - 格式为 "YYYY-MM-DD HH:MM:SS.microseconds" 的字符串
///
/// # 返回
/// Unix 时间戳（秒，包含小数部分）
fn parse_datetime_to_unix(datetime_str: &str) -> Option<f64> {
    // 格式：2025-12-24 17:16:04.520974
    let naive = NaiveDateTime::parse_from_str(datetime_str, "%Y-%m-%d %H:%M:%S%.f").ok()?;
    let shanghai_time = Shanghai.from_local_datetime(&naive).single()?;
    Some(
        shanghai_time.timestamp() as f64
            + shanghai_time.timestamp_subsec_micros() as f64 / 1_000_000.0,
    )
}

/// 加载并解析日志文件
///
/// 从日志文件中提取所有带有时间戳的日志行，并按时间戳排序
///
/// # 参数
/// * `log_path` - 日志文件路径
///
/// # 返回
/// 包含所有带时间戳的日志行的向量，按时间戳升序排列
///
/// # 日志格式
/// 支持两种时间戳格式：
/// 1. 新格式：`YYYY-MM-DD HH:MM:SS.microseconds [module] [thread_id] [LEVEL] ...`
///    例如：`2025-12-24 17:16:04.520974 [master_control] [0xF1EE3541] [INFO] ...`
/// 2. 旧格式：`[INFO/WARN/ERROR/DEBUG] [timestamp]`
///    例如：`[INFO] [1756803704.695] [master_control]: 日志内容`
///
/// # 错误处理
/// - 如果文件不存在，返回错误
/// - 如果文件不是有效的UTF-8编码，使用lossy转换处理无效字符
pub fn load_log_lines(log_path: &str) -> Result<Vec<LogLine>> {
    // 尝试以UTF-8格式读取文件，如果失败则使用lossy转换
    let content = match fs::read_to_string(log_path) {
        Ok(content) => content,
        Err(_) => {
            // 如果UTF-8读取失败，尝试读取字节并转换
            let bytes = fs::read(log_path)?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };

    // 引用静态预编译正则
    let new_ts_regex = &*NEW_TS_REGEX;
    let old_ts_regex = &*OLD_TS_REGEX;

    let mut lines = Vec::new();

    for line in content.lines() {
        // 先尝试新格式
        if let Some(caps) = new_ts_regex.captures(line)
            && let Some(timestamp) = parse_datetime_to_unix(&caps[1])
        {
            lines.push(LogLine {
                timestamp,
                line: line.to_string(),
            });
            continue;
        }

        // 回退到旧格式
        if let Some(caps) = old_ts_regex.captures(line)
            && let Ok(timestamp) = caps[1].parse::<f64>()
        {
            lines.push(LogLine {
                timestamp,
                line: line.to_string(),
            });
        }
    }

    // 按时间戳排序
    lines.sort_by(|a, b| a.timestamp.total_cmp(&b.timestamp));
    Ok(lines)
}
