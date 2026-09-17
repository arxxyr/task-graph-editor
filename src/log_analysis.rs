//! 日志分析的原生适配与隔离执行；报告目录独占创建，永不清理使用者的已有文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use analyzer_core::AnalyzeArgs;
pub use master_control_analyzer::statistics::{RoundOverview, StatisticsOptions};
use serde::{Deserialize, Serialize};

pub mod workpiece;

static NEXT_RUN: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisRequest {
    #[serde(default)]
    pub statistics: StatisticsOptions,
    #[serde(default)]
    pub workpiece: Option<workpiece::Options>,
    pub inputs: Vec<PathBuf>,
    pub output: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReport {
    pub input: PathBuf,
    pub summary: String,
    #[serde(default)]
    pub rounds: Vec<RoundOverview>,
    #[serde(default)]
    pub duration_label: String,
    pub files: Vec<PathBuf>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisReport {
    #[serde(default)]
    pub statistics: StatisticsOptions,
    pub output: PathBuf,
    pub reports: Vec<FileReport>,
    pub merged: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// 每次运行有独立目录；同名输入也分别输出，避免轮次图片互相覆盖。
pub fn create_run(parent: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(parent).map_err(|e| format!("创建报告根目录失败：{e}"))?;
    let parent = parent.canonicalize().map_err(|e| e.to_string())?;
    for _ in 0..100 {
        let serial = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            "analysis-{}-{}-{serial}",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
            std::process::id()
        );
        let path = parent.join(name);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("创建本次报告目录失败：{e}")),
        }
    }
    Err("无法分配独立报告目录".into())
}

/// 纯分析入口供子进程及回归测试调用；单个坏文件不会丢弃其他文件的结果。
pub fn analyze(request: &AnalysisRequest) -> Result<AnalysisReport, String> {
    if request.inputs.is_empty() {
        return Err("请先选择日志文件".into());
    }
    if let Some(options) = &request.workpiece {
        return workpiece::analyze(request, options);
    }
    const FONT: &[u8] = include_bytes!("../assets/fonts/SarasaTermSCNerd-Regular.ttf");
    master_control_analyzer::use_embedded_font(FONT).map_err(|e| e.to_string())?;
    analyzer_visualizer::use_embedded_font(FONT).map_err(|e| e.to_string())?;
    let plugin = master_control_analyzer::create_plugin();
    let mut report = AnalysisReport {
        statistics: request.statistics,
        output: request.output.clone(),
        reports: Vec::new(),
        merged: None,
        warnings: Vec::new(),
    };
    let mut timelines = Vec::new();
    for (index, input) in request.inputs.iter().enumerate() {
        let output = request.output.join(format!("log-{:03}", index + 1));
        let mut item = FileReport {
            input: input.clone(),
            summary: String::new(),
            rounds: Vec::new(),
            duration_label: "有效耗时（扣除暂停）".into(),
            files: Vec::new(),
            error: None,
        };
        let result = (|| {
            fs::create_dir(&output).map_err(|e| format!("创建日志输出目录失败：{e}"))?;
            let args = AnalyzeArgs {
                input_file: input.to_str().ok_or("日志路径不是有效 UTF-8")?.into(),
                output_dir: output.to_str().ok_or("报告路径不是有效 UTF-8")?.into(),
                extra_args: Some(
                    serde_json::to_string(&request.statistics)
                        .map_err(|e| e.to_string())?
                        .into(),
                )
                .into(),
                locale: "zh-CN".into(),
            };
            plugin
                .analyze(args)
                .into_result()
                .map_err(|e| e.to_string())
        })();
        match result {
            Ok(mut result) => {
                item.summary = result.summary.to_string();
                match fs::read(output.join("round_overview.json"))
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
                {
                    Ok(rounds) => item.rounds = rounds,
                    Err(error) => report.warnings.push(format!("逐轮概览读取失败：{error}")),
                }

                for file in &result.output_files {
                    let path = PathBuf::from(file.path.as_str());
                    match path.is_file() {
                        true => item.files.push(path),
                        false => report
                            .warnings
                            .push(format!("未生成报告：{}", path.display())),
                    }
                }
                // 保留每份来源的身份；在同一时钟下按绝对时间合并，不重置日志起点。
                result.timeline.is_primary = timelines.is_empty();
                result.timeline.name = format!("日志 {}", index + 1).into();
                for event in &mut result.timeline.events {
                    event.id = format!("log-{index}/{}", event.id).into();
                    event.source = result.timeline.name.clone();
                    event.parent_id = event
                        .parent_id
                        .as_ref()
                        .map(|parent| format!("log-{index}/{parent}").into());
                }
                let timeline_path = output.join("timeline.json");
                match serde_json::to_vec_pretty(&result.timeline)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| fs::write(&timeline_path, bytes).map_err(|e| e.to_string()))
                {
                    Ok(()) => item.files.push(timeline_path),
                    Err(error) => report.warnings.push(format!("导出标准时间线失败：{error}")),
                }
                match result.timeline.events.is_empty() {
                    true => item.error = Some("未识别到任务轮次或动作事件，请选择 master_control 业务日志；仅包含启动输出或普通时间戳的日志无法进行任务统计。".into()),
                    false => timelines.push(result.timeline),
                }
            }
            Err(error) => item.error = Some(error),
        }
        // 原分析器还会输出轮次统计 CSV/TXT，枚举实际文件以完整展示所有产物。
        match fs::read_dir(&output) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) if entry.path().is_file() => {
                            let path = entry.path();
                            if !item.files.contains(&path) {
                                item.files.push(path);
                            }
                        }
                        Ok(_) => {}
                        Err(error) => report
                            .warnings
                            .push(format!("读取报告目录条目失败：{error}")),
                    }
                }
                item.files.sort();
            }
            Err(error) => report.warnings.push(format!("读取报告目录失败：{error}")),
        }
        report.reports.push(item);
    }
    if timelines.len() > 1 {
        let path = request.output.join("merged_gantt.png");
        let result = analyzer_merger::TimelineMerger::with_defaults()
            .merge(timelines)
            .and_then(|merged| {
                analyzer_visualizer::GanttChartGenerator::with_defaults().generate_gantt_chart(
                    &merged,
                    &path.to_string_lossy(),
                    "多日志合并时间线",
                )
            });
        match result {
            Ok(()) => report.merged = Some(path),
            Err(error) => report.warnings.push(format!("合并时间线生成失败：{error}")),
        }
    }
    Ok(report)
}

/// GUI 启动前识别内部分析模式，子进程不初始化窗口或 GPU。
pub fn worker_entry() -> Option<bool> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--internal-log-analysis")) {
        return None;
    }
    let Some(request_path) = args.next().map(PathBuf::from) else {
        return Some(false);
    };
    let Some(response_path) = args.next().map(PathBuf::from) else {
        return Some(false);
    };
    let result: Result<AnalysisReport, String> = fs::read(&request_path)
        .map_err(|e| e.to_string())
        .and_then(|data| {
            serde_json::from_slice::<AnalysisRequest>(&data).map_err(|e| e.to_string())
        })
        .and_then(|request| analyze(&request));
    let saved = serde_json::to_vec_pretty(&result)
        .ok()
        .and_then(|data| fs::write(response_path, data).ok())
        .is_some();
    Some(saved && result.is_ok())
}

pub struct AnalysisTask {
    pub receiver: mpsc::Receiver<Result<AnalysisReport, String>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for AnalysisTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl AnalysisTask {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    pub fn spawn(request: AnalysisRequest, wake: crate::worker::WakeFn) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::Builder::new()
            .name("log-analysis".into())
            .spawn(move || {
                let result = run_child(&request, &stop);
                let _ = sender.send(result);
                wake();
            })
            .map_err(|e| format!("启动日志分析失败：{e}"))?;
        Ok(Self { receiver, cancel })
    }
}

fn run_child(request: &AnalysisRequest, cancel: &AtomicBool) -> Result<AnalysisReport, String> {
    let request_path = request.output.join("request.json");
    let response_path = request.output.join("report.json");
    fs::write(
        &request_path,
        serde_json::to_vec(request).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let diagnostics =
        fs::File::create(request.output.join("diagnostics.txt")).map_err(|e| e.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("--internal-log-analysis")
        .arg(&request_path)
        .arg(&response_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(diagnostics);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    loop {
        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "分析已取消，已生成的部分结果保留在 {}",
                request.output.display()
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let bytes = fs::read(&response_path).map_err(|e| {
                    format!("分析进程退出（{status}），读取报告失败：{e}；详情见 diagnostics.txt")
                })?;
                return serde_json::from_slice(&bytes).map_err(|e| format!("报告格式错误：{e}"))?;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("读取分析进程状态失败：{error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 新格式多文件保留暂停统计并生成中文图表() {
        let root = create_run(&std::env::temp_dir()).unwrap();
        let input = root.join("有空格的日志 1.txt");
        fs::write(&input, include_str!("../tests/fixtures/master_control.txt")).unwrap();
        let output = create_run(&root).unwrap();
        let report = analyze(&AnalysisRequest {
            statistics: StatisticsOptions::default(),
            workpiece: None,
            inputs: vec![input.clone(), input],
            output,
        })
        .unwrap();
        assert_eq!(report.reports.len(), 2);
        assert!(
            report.reports.iter().all(|r| r.error.is_none()),
            "{report:?}"
        );
        assert!(report.merged.as_ref().unwrap().is_file(), "{report:?}");
        let mut ids = std::collections::BTreeSet::new();
        for item in &report.reports {
            assert!(
                item.files
                    .iter()
                    .any(|p| p.file_name().unwrap() == "analysis.csv")
            );
            let image = item
                .files
                .iter()
                .find(|p| p.file_name().unwrap() == "round_1_gantt.png")
                .unwrap();
            assert_eq!(&fs::read(image).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
            let path = item
                .files
                .iter()
                .find(|p| p.file_name().unwrap() == "timeline.json")
                .unwrap();
            let timeline: analyzer_core::timeline::Timeline =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert!(
                timeline
                    .events
                    .iter()
                    .any(|e| e.track == analyzer_core::timeline::Track::Custom("Gripper".into()))
            );
            for event in &timeline.events {
                assert!(ids.insert(event.id.to_string()));
            }
            let lines =
                master_control_analyzer::parser::load_log_lines(item.input.to_str().unwrap())
                    .unwrap();
            let rounds = master_control_analyzer::round_detector::detect_rounds(
                &lines,
                lines.last().unwrap().timestamp,
            )
            .unwrap();
            assert_eq!(rounds[0].pause_events.len(), 1);
            assert_eq!(
                rounds[0].pause_events[0].resume_ts.unwrap() - rounds[0].pause_events[0].pause_ts,
                2.0
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn 只有时间戳的普通日志不能伪装为分析成功() {
        let root = create_run(&std::env::temp_dir()).unwrap();
        let input = root.join("launch.txt");
        fs::write(
            &input,
            "2026-09-01 10:00:00.000000 [INFO] process started\n",
        )
        .unwrap();
        let output = create_run(&root).unwrap();
        let report = analyze(&AnalysisRequest {
            statistics: StatisticsOptions::default(),
            workpiece: None,
            inputs: vec![input],
            output,
        })
        .unwrap();
        assert!(
            report.reports[0]
                .error
                .as_ref()
                .unwrap()
                .contains("未识别到")
        );
        assert!(report.merged.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn 空选择不会启动分析() {
        assert!(
            analyze(&AnalysisRequest {
                statistics: StatisticsOptions::default(),
                workpiece: None,
                inputs: vec![],
                output: PathBuf::new()
            })
            .is_err()
        );
    }

    #[test]
    fn 独占报告目录且坏文件逐项报错() {
        let root = create_run(&std::env::temp_dir()).unwrap();
        let first = create_run(&root).unwrap();
        let second = create_run(&root).unwrap();
        assert_ne!(first, second);
        fs::write(first.join("保留.txt"), "已有报告").unwrap();
        let report = analyze(&AnalysisRequest {
            statistics: StatisticsOptions::default(),
            workpiece: None,
            inputs: vec![root.join("不存在一.log"), root.join("不存在二.log")],
            output: first.clone(),
        })
        .unwrap();
        assert_eq!(report.reports.len(), 2);
        assert!(report.reports.iter().all(|r| r.error.is_some()));
        assert_eq!(
            fs::read_to_string(first.join("保留.txt")).unwrap(),
            "已有报告"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod statistics_tests {
    use super::*;
    use master_control_analyzer::models::{CycleType, LogLine, Round};
    use master_control_analyzer::statistics::select_rounds;

    #[test]
    fn 暂停错误四种组合统一控制统计且初始未完成轮次不能纳入() {
        let mut rounds: Vec<_> = (0..6)
            .map(|id| Round {
                id,
                loop_number: Some(id as u32),
                cycle_type: CycleType::Normal(id as u32),
                layer_index: 0,
                start_ts: id as f64 * 10.0,
                end_ts: Some(id as f64 * 10.0 + 9.0),
                pose0: None,
                pose6: None,
                pause_events: vec![],
            })
            .collect();
        rounds[4].cycle_type = CycleType::Initial;
        rounds[5].end_ts = None;
        let lines=vec![
            LogLine{timestamp:11.0,line:"2026-09-10 10:00:11.000000 [master_control] [INFO] TaskGraphExecutor: 用户请求暂停任务 task".into()},
            LogLine{timestamp:21.0,line:"2026-09-10 10:00:21.000000 [master_control] [0x1] [ERROR] 故障".into()},
            LogLine{timestamp:31.0,line:"2026-09-10 10:00:31.000000 [master_control] [INFO] TaskGraphExecutor: 用户请求暂停任务 task".into()},
            LogLine{timestamp:32.0,line:"[CRITICAL] [32.000000] 故障".into()},
        ];
        for (paused, errors, count) in [
            (false, false, 1),
            (true, false, 2),
            (false, true, 2),
            (true, true, 4),
        ] {
            let selection = select_rounds(
                &rounds,
                &[],
                &lines,
                StatisticsOptions {
                    include_paused: paused,
                    include_errors: errors,
                },
            );
            assert_eq!(selection.included_count, count);
            assert!(!selection.rounds[4].included);
            assert!(!selection.rounds[5].included);
            let overview = master_control_analyzer::statistics::round_overview(
                &rounds,
                &[],
                &selection,
                Path::new("missing-report-directory"),
            );
            assert_eq!(overview.len(), rounds.len());
            assert_eq!(overview.iter().filter(|r| r.included).count(), count);
            assert_eq!(overview[0].raw_seconds, Some(9.0));
            assert_eq!(overview[0].duration_seconds, Some(9.0));
            assert!(overview[1].notes.contains("含暂停"));
            assert!(overview[2].notes.contains("含错误"));
            assert_eq!(overview[5].duration_seconds, None);
        }
    }

    #[test]
    fn 旧请求兼容且统计口径序列化不会丢失() {
        let request: AnalysisRequest =
            serde_json::from_str(r#"{"inputs":["a.log"],"output":"out"}"#).unwrap();
        assert!(!request.statistics.include_paused);
        assert!(!request.statistics.include_errors);
        assert!(request.workpiece.is_none());
        let request = AnalysisRequest {
            statistics: StatisticsOptions {
                include_paused: true,
                include_errors: false,
            },
            workpiece: Some(workpiece::Options {
                date: "2026-09-10".into(),
                include_interventions: true,
                deduct_wait: false,
            }),
            ..request
        };
        let restored: AnalysisRequest =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert!(restored.statistics.include_paused);
        assert!(restored.workpiece.as_ref().unwrap().include_interventions);
        assert!(!restored.workpiece.unwrap().deduct_wait);
    }
}
