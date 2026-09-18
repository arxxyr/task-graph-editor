//! 日志选择、远程浏览与分析结果；文件列表和报告按版本重建，选择只更新按钮样式。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use super::widgets::{BoxedScene, boxed};
use bevy::feathers::controls::{ButtonVariant, FeathersButton, FeathersCheckbox};
use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::text::{EditableText, TextEdit, TextEditChange};
use bevy::ui_widgets::{Activate, ScrollArea, ValueChange};
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};

use super::graph_view::ViewMode;
use super::worker_bridge::AppAction;
use super::{Session, UiSet, widgets};
use crate::log_analysis::{
    AnalysisReport, AnalysisRequest, AnalysisTask, StatisticsOptions, create_run,
};
use crate::ssh::logs::{LogListing, LogOperation, LogReply};
use crate::worker::{WakeFn, WorkerRequest};

mod view;
pub use view::pane;

const PAGE_SIZE: usize = 30;

// Nerd Fonts 私用区图标，字形已验证在内嵌更纱黑体的 cmap 内，常规 Text 即可渲染。
/// 文件夹：目录行与本地空态
const ICON_FOLDER: &str = "\u{f07b}";
/// 文件：日志行
const ICON_FILE: &str = "\u{f15b}";
/// 搜索：筛选行与远程空态
const ICON_SEARCH: &str = "\u{f002}";
/// 信息：状态条
const ICON_INFO: &str = "\u{f05a}";

#[derive(Component, Clone, Default)]
pub struct LogPane;
#[derive(Component, Clone, Default)]
struct ListSlot;
#[derive(Component, Clone, Default)]
struct ReportSlot;
#[derive(Component, Clone, Default)]
struct LogStatus;
#[derive(Component, Clone, Default)]
struct DirectoryField;
#[derive(Component, Clone, Default)]
struct DateField;
#[derive(Component, Clone, Copy, Default)]
enum StatisticsFlag {
    #[default]
    Paused,
    Errors,
    Interventions,
    DeductWait,
}
#[derive(Component, Clone, Default)]
struct FilterField;
#[derive(Component, Clone, Default)]
struct SelectionCaption;
/// 位置行说明：远程列表目录或本地文件来源
#[derive(Component, Clone, Default)]
struct LocationCaption;
#[derive(Component, Clone, Default)]
struct ChoiceMarker {
    version: u64,
    index: usize,
}

#[derive(Component, Event, Debug, Clone, Default)]
pub enum LogAction {
    #[default]
    LocalFiles,
    LocalDirectory,
    Remote,
    Parent,
    Enter {
        version: u64,
        index: usize,
    },
    Toggle {
        version: u64,
        index: usize,
    },
    SelectAll,
    Clear,
    Previous,
    Next,
    Output,
    Analyze,
    Mode(bool),
    Cancel,
    Open(PathBuf),
    ReportDirectory,
    ReportLog(bool),
    ReportPage(bool),
}

#[derive(Clone)]
struct Choice {
    /// 行内显示名称：远程为条目名，本地为完整路径
    name: String,
    /// 右端大小文本（如 "12.3 KiB"）；目录为 None，行尾改显示 ›
    size: Option<String>,
    path: PathBuf,
    directory: bool,
}

struct RemotePending {
    receiver: Mutex<mpsc::Receiver<Result<LogReply, String>>>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    output: Option<PathBuf>,
}
impl Drop for RemotePending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

enum DialogReply {
    Files(Vec<PathBuf>),
    Output(PathBuf),
    Cancelled,
    Error(String),
}

#[derive(Resource)]
struct Logs {
    directory: String,
    filter: String,
    directory_version: u64,
    choices: Vec<Choice>,
    visible: Vec<usize>,
    source: Option<(u64, String)>,
    selected: BTreeSet<usize>,
    version: u64,
    list_revision: u64,
    page: usize,
    rendered_list: Option<u64>,
    rendered_report: Option<u64>,
    report_revision: u64,
    report_index: usize,
    report_page: usize,
    report: Option<AnalysisReport>,
    output_parent: PathBuf,
    statistics: StatisticsOptions,
    workpiece: bool,
    date: String,
    include_interventions: bool,
    deduct_wait: bool,
    status: String,
    remote: Option<RemotePending>,
    task: Option<Mutex<AnalysisTask>>,
    dialog: Option<Mutex<mpsc::Receiver<DialogReply>>>,
}

impl Default for Logs {
    fn default() -> Self {
        let base = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            directory: "~/.ros/log".into(),
            filter: String::new(),
            directory_version: 0,
            choices: Vec::new(),
            visible: Vec::new(),
            source: None,
            selected: BTreeSet::new(),
            version: 0,
            list_revision: 0,
            page: 0,
            rendered_list: None,
            rendered_report: None,
            report_revision: 0,
            report_index: 0,
            report_page: 0,
            report: None,
            statistics: StatisticsOptions::default(),
            workpiece: false,
            date: String::new(),
            include_interventions: false,
            deduct_wait: true,
            output_parent: base.join("task-graph-editor-reports"),
            status:
                "选择本地日志，或连接顶栏 SSH 后浏览远程日志目录。支持 master_control 新旧格式。"
                    .into(),
            remote: None,
            task: None,
            dialog: None,
        }
    }
}

impl Logs {
    fn refresh_filter(&mut self) {
        let filter = self.filter.to_lowercase();
        self.visible = self
            .choices
            .iter()
            .enumerate()
            .filter_map(|(index, choice)| {
                choice
                    .name
                    .to_lowercase()
                    .contains(&filter)
                    .then_some(index)
            })
            .collect();
    }

    fn visible_indices(&self) -> &[usize] {
        &self.visible
    }

    fn busy(&self) -> bool {
        self.task.is_some() || self.remote.is_some() || self.dialog.is_some()
    }

    fn replace(&mut self, choices: Vec<Choice>, source: Option<(u64, String)>, select: bool) {
        self.selected = match select {
            true => (0..choices.len())
                .filter(|&i| !choices[i].directory)
                .collect(),
            false => BTreeSet::new(),
        };
        self.choices = choices;
        self.refresh_filter();
        self.source = source;
        self.version += 1;
        self.list_revision += 1;
        self.page = 0;
    }

    fn start(&mut self, inputs: Vec<PathBuf>, output: PathBuf, wake: WakeFn) {
        match AnalysisTask::spawn(
            AnalysisRequest {
                statistics: self.statistics,
                workpiece: self
                    .workpiece
                    .then(|| crate::log_analysis::workpiece::Options {
                        date: self.date.trim().to_string(),
                        include_interventions: self.include_interventions,
                        deduct_wait: self.deduct_wait,
                    }),
                inputs,
                output: output.clone(),
            },
            wake,
        ) {
            Ok(task) => {
                self.task = Some(Mutex::new(task));
                self.status = format!("正在分析日志并生成报告…\n本次输出：{}", output.display());
            }
            Err(error) => self.status = error,
        }
    }

    fn browse(&mut self, session: &Session, directory: String) {
        if !session.is_connected || !session.interactive() {
            self.status = "请先连接远程主机，并等待当前 SSH 操作完成".into();
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        session.send(WorkerRequest::Logs {
            operation: LogOperation::List(directory),
            cancel: cancel.clone(),
            reply: sender,
        });
        self.remote = Some(RemotePending {
            receiver: Mutex::new(receiver),
            generation: session.connection_generation,
            cancel,
            output: None,
        });
        self.status = "正在读取远程日志目录…".into();
    }
}

fn click(event: On<Activate>, actions: Query<&LogAction>, mut writer: MessageWriter<AppAction>) {
    if let Ok(action) = actions.get(event.entity) {
        writer.write(AppAction::Logs(action.clone()));
    }
}

fn edit_filter(
    event: On<TextEditChange>,
    fields: Query<&EditableText, With<FilterField>>,
    mut logs: ResMut<Logs>,
) {
    if let Ok(field) = fields.get(event.event_target()) {
        let filter = field.value().to_string();
        if logs.filter != filter {
            logs.filter = filter;
            logs.refresh_filter();
            logs.page = 0;
            logs.list_revision += 1;
        }
    }
}

fn edit_directory(
    event: On<TextEditChange>,
    fields: Query<&EditableText, With<DirectoryField>>,
    mut logs: ResMut<Logs>,
) {
    if let Ok(field) = fields.get(event.event_target()) {
        logs.directory = field.value().to_string();
    }
}

fn wake_fn(proxy: Option<&EventLoopProxyWrapper>) -> WakeFn {
    match proxy {
        Some(proxy) => {
            let proxy = (**proxy).clone();
            Arc::new(move || {
                let _ = proxy.send_event(WinitUserEvent::WakeUp);
            })
        }
        None => Arc::new(|| {}),
    }
}

fn handle_action(
    event: On<LogAction>,
    mut logs: ResMut<Logs>,
    session: Res<Session>,
    proxy: Option<Res<EventLoopProxyWrapper>>,
) {
    let wake = wake_fn(proxy.as_deref());
    if matches!(&*event, LogAction::Cancel) {
        if let Some(task) = &logs.task {
            task.lock().unwrap_or_else(|e| e.into_inner()).cancel();
        }
        if let Some(pending) = &logs.remote {
            pending.cancel.store(true, Ordering::Release);
        }
        logs.status = match logs.busy() {
            true => "正在取消，请等待当前文件操作结束…".into(),
            false => "当前没有正在执行的分析".into(),
        };
        return;
    }
    if matches!(*event, LogAction::ReportDirectory) {
        if let Some(report) = &logs.report {
            let path = report.output.clone();
            if let Err(error) = open_path(&path) {
                logs.status = format!("打开报告目录失败：{error}");
            }
        }
        return;
    }
    if let LogAction::Open(path) = &*event {
        if let Err(error) = open_path(path) {
            logs.status = error;
        }
        return;
    }
    if logs.busy() {
        logs.status = "请等待当前操作完成，或先取消分析".into();
        return;
    }
    match &*event {
        LogAction::Mode(value) => {
            logs.workpiece = *value;
        }
        LogAction::LocalFiles | LogAction::LocalDirectory | LogAction::Output => {
            let action = event.clone();
            let (sender, receiver) = mpsc::channel();
            let result = std::thread::Builder::new()
                .name("log-dialog".into())
                .spawn(move || {
                    let reply = match action {
                        LogAction::LocalFiles => rfd::FileDialog::new()
                            .set_title("选择日志（可多选）")
                            .pick_files()
                            .map(DialogReply::Files)
                            .unwrap_or(DialogReply::Cancelled),
                        LogAction::LocalDirectory => match rfd::FileDialog::new()
                            .set_title("选择本地日志目录")
                            .pick_folder()
                        {
                            Some(dir) => match local_logs(&dir) {
                                Ok(files) => DialogReply::Files(files),
                                Err(error) => DialogReply::Error(error),
                            },
                            None => DialogReply::Cancelled,
                        },
                        _ => rfd::FileDialog::new()
                            .set_title("选择报告保存目录")
                            .pick_folder()
                            .map(DialogReply::Output)
                            .unwrap_or(DialogReply::Cancelled),
                    };
                    let _ = sender.send(reply);
                    wake();
                });
            match result {
                Ok(_) => {
                    logs.dialog = Some(Mutex::new(receiver));
                    logs.status = "等待选择文件或目录…".into();
                }
                Err(error) => logs.status = format!("打开文件选择器失败：{error}"),
            }
        }
        LogAction::Remote => {
            let directory = logs.directory.clone();
            logs.browse(&session, directory);
        }
        LogAction::Parent => {
            let Some((generation, directory)) = &logs.source else {
                return;
            };
            if *generation != session.connection_generation {
                logs.status = "连接已变化，请重新浏览远程目录".into();
                return;
            }
            let parent = std::path::Path::new(directory)
                .parent()
                .unwrap_or(std::path::Path::new("/"))
                .to_string_lossy()
                .into_owned();
            logs.browse(&session, parent);
        }
        LogAction::Enter { version, index } => {
            if *version != logs.version {
                return;
            }
            let Some((generation, _)) = &logs.source else {
                return;
            };
            if *generation != session.connection_generation {
                logs.status = "连接已变化，请重新浏览远程目录".into();
                return;
            }
            if let Some(choice) = logs.choices.get(*index).filter(|c| c.directory) {
                let path = choice.path.to_string_lossy().into_owned();
                logs.browse(&session, path);
            }
        }
        LogAction::Toggle { version, index } => {
            if *version == logs.version
                && logs.choices.get(*index).is_some_and(|c| !c.directory)
                && !logs.selected.remove(index)
            {
                logs.selected.insert(*index);
            }
        }
        LogAction::SelectAll => {
            let visible = logs.visible_indices().to_vec();
            for index in visible {
                if !logs.choices[index].directory {
                    logs.selected.insert(index);
                }
            }
        }
        LogAction::Clear => logs.selected.clear(),
        LogAction::Previous => {
            if logs.page > 0 {
                logs.page -= 1;
                logs.list_revision += 1;
            }
        }
        LogAction::Next => {
            if (logs.page + 1) * PAGE_SIZE < logs.visible_indices().len() {
                logs.page += 1;
                logs.list_revision += 1;
            }
        }
        LogAction::Analyze => {
            if logs.workpiece
                && !logs.date.trim().is_empty()
                && (logs.date.trim().len() != 10
                    || chrono::NaiveDate::parse_from_str(logs.date.trim(), "%Y-%m-%d").is_err())
            {
                logs.status = "日期请填写 YYYY-MM-DD，或留空分析全部日期（按北京时间）".into();
                return;
            }
            if logs.selected.is_empty() {
                logs.status = "请至少选择一个日志文件".into();
                return;
            }
            let inputs: Vec<_> = logs
                .selected
                .iter()
                .map(|&i| logs.choices[i].path.clone())
                .collect();
            if let Some((generation, _)) = &logs.source
                && (*generation != session.connection_generation
                    || !session.is_connected
                    || !session.interactive())
            {
                logs.status = "远程连接已变化或正在忙碌，请刷新目录后重试".into();
                return;
            }
            let output = match create_run(&logs.output_parent) {
                Ok(path) => path,
                Err(error) => {
                    logs.status = error;
                    return;
                }
            };
            match &logs.source {
                None => logs.start(inputs, output, wake),
                Some((generation, directory)) => {
                    let (sender, receiver) = mpsc::channel();
                    let cancel = Arc::new(AtomicBool::new(false));
                    let names = inputs
                        .iter()
                        .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_owned))
                        .collect();
                    session.send(WorkerRequest::Logs {
                        operation: LogOperation::Download {
                            directory: directory.clone(),
                            names,
                            destination: output.clone(),
                        },
                        cancel: cancel.clone(),
                        reply: sender,
                    });
                    logs.remote = Some(RemotePending {
                        receiver: Mutex::new(receiver),
                        generation: *generation,
                        cancel,
                        output: Some(output.clone()),
                    });
                    logs.status = format!("正在下载所选日志…\n本次输出：{}", output.display());
                }
            }
        }
        LogAction::ReportLog(forward) => {
            let count = logs.report.as_ref().map_or(0, |r| r.reports.len());
            let target = match forward {
                true => logs.report_index.saturating_add(1),
                false => logs.report_index.saturating_sub(1),
            };
            if target < count && target != logs.report_index {
                logs.report_index = target;
                logs.report_page = 0;
                logs.report_revision += 1;
            }
        }
        LogAction::ReportPage(forward) => {
            let count = logs
                .report
                .as_ref()
                .and_then(|r| r.reports.get(logs.report_index))
                .map_or(0, |r| r.rounds.len());
            let target = match forward {
                true => logs.report_page.saturating_add(1),
                false => logs.report_page.saturating_sub(1),
            };
            if target * PAGE_SIZE < count && target != logs.report_page {
                logs.report_page = target;
                logs.report_revision += 1;
            }
        }
        LogAction::Cancel | LogAction::Open(_) | LogAction::ReportDirectory => {}
    }
}

fn local_logs(directory: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if entry.file_type().map_err(|e| e.to_string())?.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn open_path(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Err("报告文件已不存在".into());
    }
    let mut command = match std::env::consts::OS {
        "macos" => std::process::Command::new("open"),
        "windows" => std::process::Command::new("explorer.exe"),
        _ => std::process::Command::new("xdg-open"),
    };
    let mut child = command
        .arg(path)
        .spawn()
        .map_err(|e| format!("打开报告失败：{e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn accept_listing(logs: &mut Logs, listing: LogListing, generation: u64) {
    logs.directory.clone_from(&listing.directory);
    logs.directory_version += 1;
    let choices = listing
        .entries
        .into_iter()
        .map(|entry| {
            let path = PathBuf::from(&listing.directory).join(&entry.name);
            Choice {
                size: match entry.directory {
                    true => None,
                    false => Some(format!("{:.1} KiB", entry.size as f64 / 1024.0)),
                },
                name: entry.name,
                path,
                directory: entry.directory,
            }
        })
        .collect();
    logs.replace(choices, Some((generation, listing.directory)), false);
    logs.status =
        "仅显示日期目录和主控日志。日期及文件名中的创建时间均倒序排列；点击日期进入并选择日志。"
            .into();
}

fn poll(mut logs: ResMut<Logs>, session: Res<Session>, proxy: Option<Res<EventLoopProxyWrapper>>) {
    let dialog = logs
        .dialog
        .as_ref()
        .map(|rx| rx.lock().unwrap_or_else(|e| e.into_inner()).try_recv());
    if let Some(result) = dialog {
        match result {
            Ok(reply) => {
                logs.dialog = None;
                match reply {
                    DialogReply::Files(files) => {
                        let mut unique = BTreeSet::new();
                        let choices = files
                            .into_iter()
                            .filter(|p| unique.insert(p.clone()))
                            .map(|path| {
                                let size = std::fs::metadata(&path)
                                    .ok()
                                    .map(|meta| format!("{:.1} KiB", meta.len() as f64 / 1024.0));
                                Choice {
                                    name: path.to_string_lossy().into_owned(),
                                    size,
                                    path,
                                    directory: false,
                                }
                            })
                            .collect();
                        logs.replace(choices, None, true);
                        logs.status = "本地日志已选择，可以开始分析。".into();
                    }
                    DialogReply::Output(path) => {
                        logs.output_parent = path;
                        logs.status = "报告目录已更新；每次分析会创建独立子目录。".into();
                    }
                    DialogReply::Cancelled => logs.status = "已取消选择，原有选择保持不变。".into(),
                    DialogReply::Error(error) => logs.status = format!("读取本地目录失败：{error}"),
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                logs.dialog = None;
                logs.status = "文件选择器意外退出".into();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    if let Some(pending) = &logs.remote
        && (pending.generation != session.connection_generation || !session.is_connected)
    {
        logs.remote = None;
        logs.status = "连接已变化，已取消日志请求；请重新浏览目录。".into();
    }
    let remote = logs.remote.as_ref().map(|r| {
        r.receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv()
    });
    if let Some(result) = remote {
        match result {
            Ok(result) => {
                let pending = logs.remote.take().unwrap();
                if pending.cancel.load(Ordering::Acquire) {
                    logs.status = "日志操作已取消，原选择保持不变。".into();
                } else {
                    match result {
                        Ok(LogReply::Listed(listing)) => {
                            accept_listing(&mut logs, listing, pending.generation)
                        }
                        Ok(LogReply::Downloaded(inputs)) => {
                            if let Some(output) = &pending.output {
                                logs.start(inputs, output.clone(), wake_fn(proxy.as_deref()));
                            }
                        }
                        Err(error) => logs.status = format!("远程日志操作失败：{error}"),
                    }
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                logs.remote = None;
                logs.status = "远程日志通道已关闭，请重新连接".into();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    let result = logs.task.as_ref().map(|t| {
        t.lock()
            .unwrap_or_else(|e| e.into_inner())
            .receiver
            .try_recv()
    });
    if let Some(result) = result {
        match result {
            Ok(result) => {
                logs.task = None;
                match result {
                    Ok(report) => {
                        let failed = report.reports.iter().filter(|r| r.error.is_some()).count();
                        logs.status = format!(
                            "分析完成：生成 {} 组报告，失败 {failed} 组。",
                            report.reports.len() - failed
                        );
                        logs.report_index = 0;
                        logs.report_page = 0;
                        logs.report = Some(report);
                        logs.report_revision += 1;
                    }
                    Err(error) => logs.status = format!("日志分析：{error}"),
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                logs.task = None;
                logs.status = "分析线程意外退出，已有报告已保留".into();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}

/// 解析报告快照：磁盘上的 report.json 是 Result 信封（{"Ok": …}），也接受裸 AnalysisReport。
fn parse_report_snapshot(bytes: &[u8]) -> Option<AnalysisReport> {
    if let Ok(report) = serde_json::from_slice::<AnalysisReport>(bytes) {
        return Some(report);
    }
    let envelope = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    serde_json::from_value(envelope.get("Ok")?.clone()).ok()
}

pub struct LogAnalysisPlugin;
impl Plugin for LogAnalysisPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var_os("TGE_SCREENSHOT").is_some()
            && std::env::var("TGE_SCREENSHOT_VIEW").as_deref() == Ok("logs")
        {
            app.insert_resource(ViewMode::Logs);
        }
        if std::env::var_os("TGE_SCREENSHOT").is_some()
            && std::env::var("TGE_SCREENSHOT_VIEW").as_deref() == Ok("workpiece")
        {
            app.insert_resource(ViewMode::Logs);
            app.insert_resource(Logs {
                workpiece: true,
                ..Default::default()
            });
        }
        // 截图验收可加载已有报告快照，不执行分析或连接远端。
        if std::env::var_os("TGE_SCREENSHOT").is_some()
            && let Some(path) = std::env::var_os("TGE_SCREENSHOT_REPORT")
            && let Ok(bytes) = std::fs::read(path)
            && let Some(report) = parse_report_snapshot(&bytes)
        {
            app.insert_resource(ViewMode::Logs);
            app.insert_resource(Logs {
                report: Some(report),
                status: "已载入分析报告".into(),
                report_revision: 1,
                ..Default::default()
            });
        }
        app.init_resource::<Logs>()
            .add_observer(handle_action)
            .add_observer(edit_directory)
            .add_observer(edit_filter)
            .add_observer(edit_date)
            .add_observer(change_statistics)
            .add_systems(
                Update,
                (poll, view::sync_report_setup)
                    .chain()
                    .in_set(UiSet::Update),
            )
            .add_systems(Update, view::rebuild.in_set(UiSet::Rebuild))
            .add_systems(
                Update,
                (
                    view::sync,
                    view::sync_pane,
                    view::sync_gates,
                    view::sync_document_actions,
                    view::sync_statistics,
                )
                    .after(UiSet::Rebuild),
            );
    }
}

#[cfg(test)]
mod tests;

fn change_statistics(
    event: On<ValueChange<bool>>,
    flags: Query<&StatisticsFlag>,
    mut logs: ResMut<Logs>,
    guard: Option<Res<super::document_guard::DocumentGuard>>,
) {
    if logs.busy() || guard.is_some_and(|g| g.active()) {
        return;
    }
    if let Ok(flag) = flags.get(event.source) {
        match flag {
            StatisticsFlag::Paused => logs.statistics.include_paused = event.value,
            StatisticsFlag::Errors => logs.statistics.include_errors = event.value,
            StatisticsFlag::Interventions => logs.include_interventions = event.value,
            StatisticsFlag::DeductWait => logs.deduct_wait = event.value,
        }
    }
}

fn edit_date(
    event: On<TextEditChange>,
    fields: Query<&EditableText, With<DateField>>,
    mut logs: ResMut<Logs>,
) {
    if logs.busy() {
        return;
    }
    if let Ok(field) = fields.get(event.event_target()) {
        logs.date = field.value().to_string();
    }
}
