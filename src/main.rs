//! 任务图编辑器 - 远程 JSON 位姿编辑工具

// Windows 下隐藏控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod model;
mod ssh;
mod ssh_config;
mod ui;
mod worker;

use bevy::feathers::FeathersPlugins;
use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};
use bevy::winit::{UpdateMode, WinitSettings};

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// 窗口初始尺寸
const WINDOW_SIZE: (u32, u32) = (1280, 800);

/// 窗口最小尺寸
const WINDOW_MIN_SIZE: (f32, f32) = (960.0, 600.0);

/// 日志配置
///
/// 默认只留警告以上；`TGE_LOG=debug`（或 `trace`）打开排查用的详细日志，
/// 例如 SSH 连接参数、SFTP 路径展开结果、文件列表条数、界面重建时机。
///
/// `icu_provider=off` 任何级别下都保留：parley 的分段器不带 CJK 词典，
/// 中文每次布局都会警告一次，不屏蔽会把真正有用的日志淹掉。
fn log_plugin() -> bevy::log::LogPlugin {
    let (level, deps) = match std::env::var("TGE_LOG").as_deref() {
        Ok("trace") => (bevy::log::Level::TRACE, "wgpu=warn,naga=warn"),
        Ok("debug") => (bevy::log::Level::DEBUG, "wgpu=error,naga=error"),
        _ => (bevy::log::Level::WARN, "wgpu=error,naga=error"),
    };
    bevy::log::LogPlugin {
        level,
        filter: format!("{deps},icu_provider=off"),
        ..default()
    }
}

/// 窗口刷新策略
///
/// 桌面应用默认走 reactive：无输入时不重绘，空闲 CPU 占用接近零；
/// 后台 SSH 线程有响应时通过 winit 的 `EventLoopProxy` 主动唤醒
/// （见 `ui::worker_bridge`）。
///
/// 自动截图是一次性批处理，必须切回连续渲染——reactive 下没有输入就几乎不出帧，
/// 按帧数计时的截图会一直等下去。
fn winit_settings() -> WinitSettings {
    if std::env::var("TGE_SCREENSHOT").is_ok() {
        return WinitSettings::continuous();
    }
    WinitSettings {
        focused_mode: UpdateMode::reactive(core::time::Duration::from_secs(5)),
        unfocused_mode: UpdateMode::reactive_low_power(core::time::Duration::from_secs(30)),
    }
}

fn main() -> AppExit {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    close_when_requested: false,
                    primary_window: Some(Window {
                        title: "任务图编辑器".into(),
                        // app id 用 ASCII，避免部分 Linux WM 对非 ASCII 值乱码
                        name: Some("task-graph-editor".into()),
                        resolution: WindowResolution::new(WINDOW_SIZE.0, WINDOW_SIZE.1),
                        resize_constraints: WindowResizeConstraints {
                            min_width: WINDOW_MIN_SIZE.0,
                            min_height: WINDOW_MIN_SIZE.1,
                            ..default()
                        },
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                })
                .set(log_plugin()),
        )
        .add_plugins(FeathersPlugins)
        .add_plugins(ui::EditorUiPlugin)
        .insert_resource(winit_settings())
        .run()
}
