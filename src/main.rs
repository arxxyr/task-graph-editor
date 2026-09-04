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

fn main() -> AppExit {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
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
                // 工具类应用不需要日志刷屏，只保留警告以上
                .set(bevy::log::LogPlugin {
                    level: bevy::log::Level::WARN,
                    // parley 的分段器不加载 CJK 词典（icu_segmenter 的
                    // new_for_non_complex_scripts），每段中文布局都会 warn 一次，
                    // 中文界面下会刷屏；断行退化为按字形断，对本应用的短标签无影响
                    filter: "wgpu=error,naga=error,icu_provider=off".into(),
                    ..default()
                }),
        )
        .add_plugins(FeathersPlugins)
        .add_plugins(ui::EditorUiPlugin)
        // 桌面应用：无输入时不重绘，CPU 占用接近零；
        // 后台 SSH 线程通过 winit 的 EventLoopProxy 主动唤醒（见 ui::worker_bridge）
        .insert_resource(WinitSettings {
            focused_mode: UpdateMode::reactive(core::time::Duration::from_secs(5)),
            unfocused_mode: UpdateMode::reactive_low_power(core::time::Duration::from_secs(30)),
        })
        .run()
}
