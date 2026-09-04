//! 界面截图
//!
//! - **F12**：随时把当前窗口存成 PNG，方便反馈界面问题时附图；
//! - **`TGE_SCREENSHOT=<路径>`**：启动后等界面稳定自动截一张再退出，
//!   供自动化验证和 CI 视觉回归使用，不需要任何屏幕录制权限。

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use super::UiSet;

/// 自动截图前等待的帧数
///
/// 场景是分帧 spawn 的（`queue_spawn_related_scenes` 要等命令应用），
/// 加上字体加载与文本布局，留出足够帧数才能拍到完整界面。
const AUTO_CAPTURE_DELAY_FRAMES: u32 = 30;

/// 截图触发后再等待的帧数
///
/// 截图要等这一帧渲染完再从 GPU 回读、编码、落盘，立刻退出会得到半个文件。
const AUTO_EXIT_DELAY_FRAMES: u32 = 60;

/// 自动截图状态
#[derive(Resource)]
struct AutoCapture {
    /// 输出路径
    path: String,
    /// 剩余等待帧数
    countdown: u32,
    /// 是否已经触发过
    fired: bool,
    /// 截图后到退出之间的剩余帧数
    exit_countdown: u32,
}

/// F12：截图到当前目录，文件名带时间戳
fn capture_on_key(keys: Res<ButtonInput<KeyCode>>, mut commands: Commands) {
    if !keys.just_pressed(KeyCode::F12) {
        return;
    }
    let path = format!(
        "task-graph-editor-{}.png",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    info!("截图保存到 {path}");
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

/// 启动若干帧后自动截图，随后退出
fn capture_on_start(
    mut auto: ResMut<AutoCapture>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    if auto.fired {
        match auto.exit_countdown.checked_sub(1) {
            Some(remaining) => auto.exit_countdown = remaining,
            None => {
                exit.write(AppExit::Success);
            }
        }
        return;
    }
    match auto.countdown.checked_sub(1) {
        Some(remaining) => auto.countdown = remaining,
        None => {
            auto.fired = true;
            let path = auto.path.clone();
            info!("自动截图保存到 {path}");
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path));
        }
    }
}

/// 截图插件
pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, capture_on_key.in_set(UiSet::Input));

        if let Ok(path) = std::env::var("TGE_SCREENSHOT") {
            app.insert_resource(AutoCapture {
                path,
                countdown: AUTO_CAPTURE_DELAY_FRAMES,
                fired: false,
                exit_countdown: AUTO_EXIT_DELAY_FRAMES,
            })
            .add_systems(Update, capture_on_start);
        }
    }
}
