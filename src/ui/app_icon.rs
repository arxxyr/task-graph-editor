//! 应用图标随主题切换：深色主题用深色蜘蛛徽标，浅色主题用浅色版。
//!
//! 安装包里的图标是静态的（macOS `.icns`、Windows exe 资源、Linux 桌面入口），统一用深色版；
//! 这里负责运行期能改的部分：Windows 与 X11 的窗口、任务栏图标，以及 macOS 的 Dock 图标。
//! Wayland 不允许应用自己设置窗口图标，只能靠随包附带的桌面入口。
//!
//! 图标由 `scripts/generate_app_icons.py` 生成，编译时嵌入，运行时不读文件。

use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::NonSendMarker;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::winit::WINIT_WINDOWS;
use winit::window::Icon;

use super::UiSet;
use super::theme::ThemeSelection;

/// 同一个图标的深色、浅色两份 PNG 原文。
struct IconPair {
    dark: &'static [u8],
    light: &'static [u8],
}

impl IconPair {
    const fn pick(&self, dark: bool) -> &'static [u8] {
        match dark {
            true => self.dark,
            false => self.light,
        }
    }
}

/// Windows 标题栏小图标，小尺寸专门加粗过
#[cfg(any(windows, test))]
const TITLE_BAR: IconPair = IconPair {
    dark: include_bytes!("../../assets/icons/window-dark-32.png"),
    light: include_bytes!("../../assets/icons/window-light-32.png"),
};

/// Windows 任务栏与 X11 窗口图标
const TASKBAR: IconPair = IconPair {
    dark: include_bytes!("../../assets/icons/window-dark-256.png"),
    light: include_bytes!("../../assets/icons/window-light-256.png"),
};

/// macOS Dock 图标，四周按系统图标网格留白
#[cfg(any(target_os = "macos", test))]
const DOCK: IconPair = IconPair {
    dark: include_bytes!("../../assets/icons/dock-dark.png"),
    light: include_bytes!("../../assets/icons/dock-light.png"),
};

/// 解码后的 RGBA8 位图。
struct Bitmap {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// winit 要的是 RGBA 位图；借 Bevy 已启用的 PNG 解码器，不为图标再引入图像库。
fn decode(png: &[u8]) -> Option<Bitmap> {
    let image = Image::from_buffer(
        png,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::MAIN_WORLD,
    )
    .ok()?;
    let size = image.size();
    let rgba = image.data?;
    // 只接受每像素 4 字节的 RGBA8；其他格式交给系统会显示成花屏。
    (rgba.len() == size.x as usize * size.y as usize * 4).then_some(Bitmap {
        rgba,
        width: size.x,
        height: size.y,
    })
}

fn window_icon(png: &[u8]) -> Option<Icon> {
    let bitmap = decode(png)?;
    Icon::from_rgba(bitmap.rgba, bitmap.width, bitmap.height).ok()
}

/// 设置窗口图标；返回 `false` 表示窗口还没创建，调用方下一帧重试。
fn apply_window_icon(window: Entity, dark: bool) -> bool {
    WINIT_WINDOWS.with_borrow(|windows| {
        let Some(window) = windows.get_window(window) else {
            return false;
        };
        // Windows 的标题栏与任务栏各用一张；X11 只有一张，给大图让桌面环境自行缩放。
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowExtWindows;
            window.set_window_icon(window_icon(TITLE_BAR.pick(dark)));
            window.set_taskbar_icon(window_icon(TASKBAR.pick(dark)));
        }
        #[cfg(not(windows))]
        window.set_window_icon(window_icon(TASKBAR.pick(dark)));
        true
    })
}

/// macOS 没有窗口图标，运行期能换的是 Dock 图标；从终端直接运行时也不再是通用图标。
/// 返回 `false` 表示当前不在主线程，调用方下一帧重试。
#[cfg(target_os = "macos")]
fn apply_dock_icon(dark: bool) -> bool {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let data = NSData::with_bytes(DOCK.pick(dark));
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        warn!("Dock 图标解码失败，保留系统默认图标");
        return true;
    };
    let application = NSApplication::sharedApplication(main_thread);
    // SAFETY: 位于主线程；image 在调用期间存活，AppKit 会自行持有它。
    unsafe { application.setApplicationIconImage(Some(&image)) };
    true
}

#[cfg(not(target_os = "macos"))]
fn apply_dock_icon(_dark: bool) -> bool {
    true
}

/// 窗口图标的代码在所有平台都参与编译和测试；macOS 上 winit 的窗口图标是空操作，改设 Dock 图标。
/// 测试进程里没有 AppKit 应用，同样走窗口路径。
fn apply(window: Entity, dark: bool) -> bool {
    match cfg!(all(target_os = "macos", not(test))) {
        true => apply_dock_icon(dark),
        false => apply_window_icon(window, dark),
    }
}

/// 已经交给系统的图标：`Some(true)` 为深色版。主题在同为深色或同为浅色之间切换时不必重设。
///
/// 窗口要过几帧才创建出来，没设置成功就保持原值，下一帧再试。
#[derive(Resource, Default, Debug, PartialEq, Eq)]
struct AppliedIcon(Option<bool>);

/// 窗口与 Dock 图标只能在主线程上设置。
fn sync_app_icon(
    selection: Res<ThemeSelection>,
    mut applied: ResMut<AppliedIcon>,
    window: Query<Entity, With<PrimaryWindow>>,
    _main_thread: NonSendMarker,
) {
    let dark = selection.0.is_dark();
    if applied.0 == Some(dark) {
        return;
    }
    let Ok(window) = window.single() else {
        return;
    };
    if apply(window, dark) {
        debug!(theme = selection.0.label(), dark, "应用图标已随主题切换");
        applied.0 = Some(dark);
    }
}

pub struct AppIconPlugin;

impl Plugin for AppIconPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppliedIcon>()
            .add_systems(Update, sync_app_icon.after(UiSet::Rebuild));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::ThemeId;

    #[test]
    fn 内嵌图标都能解码为预期尺寸且四角透明() {
        for (pair, side) in [(&TITLE_BAR, 32), (&TASKBAR, 256), (&DOCK, 512)] {
            for dark in [true, false] {
                let bitmap = decode(pair.pick(dark)).expect("图标必须是 RGBA8 的 PNG");
                assert_eq!((bitmap.width, bitmap.height), (side, side));
                assert_eq!(bitmap.rgba.len(), (side * side * 4) as usize);
                // 圆角之外必须透明，否则任务栏上会多出一块方形底色。
                assert_eq!(bitmap.rgba[3], 0, "左上角应透明");
                assert!(window_icon(pair.pick(dark)).is_some());
            }
        }
        assert!(decode(b"not a png").is_none());
    }

    #[test]
    fn 深色主题取深色图标浅色主题取浅色图标() {
        for pair in [&TITLE_BAR, &TASKBAR, &DOCK] {
            assert_ne!(pair.dark, pair.light);
            assert_eq!(pair.pick(true), pair.dark);
            assert_eq!(pair.pick(false), pair.light);
        }
        let dark: Vec<bool> = ThemeId::ALL.iter().map(|theme| theme.is_dark()).collect();
        assert_eq!(dark, [true, true, false, false]);
    }

    #[test]
    fn 窗口未创建时保持未应用以便下一帧重试() {
        let mut app = App::new();
        app.insert_resource(ThemeSelection(ThemeId::WarmPaper))
            .add_plugins(AppIconPlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.update();
        // 测试里不存在真正的 winit 窗口，不能把图标记成已应用。
        assert_eq!(*app.world().resource::<AppliedIcon>(), AppliedIcon(None));
    }
}
