//! 任务图编辑器 - 远程 JSON 位姿编辑工具

// Windows 下隐藏控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod model;
mod ssh;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// 编译时嵌入字体文件，无需运行时依赖外部文件
const SARASA_FONT: &[u8] = include_bytes!("../assets/fonts/SarasaTermSCNerd-Regular.ttf");

/// 配置 egui 使用更纱黑体作为默认字体（覆盖所有文本类别）
fn setup_fonts(ctx: &eframe::egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;

    let mut fonts = FontDefinitions::default();

    // 注册字体数据
    fonts.font_data.insert(
        "sarasa".into(),
        Arc::new(FontData::from_static(SARASA_FONT)),
    );

    // 将更纱黑体插入到 Proportional 和 Monospace 族的最高优先级
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "sarasa".into());
    }

    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 720.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };

    eframe::run_native(
        "任务图编辑器",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            Ok(Box::new(app::App::default()))
        }),
    )
}
