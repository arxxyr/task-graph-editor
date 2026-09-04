//! 中文字体：编译时嵌入更纱黑体，运行时覆盖到所有文本控件
//!
//! Feathers 控件内部硬编码了自带的 FiraSans（`embedded://bevy_feathers/...`），
//! 该字体不含 CJK 字形，中文界面会整片豆腐块。这里在文本布局之前统一把
//! `TextFont::font` 换成嵌入的更纱黑体，覆盖 Feathers 自建的控件与自己写的场景。

use bevy::prelude::*;
use bevy::text::FontSource;
use bevy::ui::UiSystems;

/// 编译时嵌入字体文件，无需运行时依赖外部文件
const SARASA_FONT: &[u8] = include_bytes!("../../assets/fonts/SarasaTermSCNerd-Regular.ttf");

/// 嵌入字体的句柄
#[derive(Resource)]
pub struct AppFont(pub Handle<Font>);

/// 注册嵌入字体，存入资源供后续覆盖使用
fn load_embedded_font(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let handle = fonts.add(Font::from_bytes(SARASA_FONT.to_vec()));
    commands.insert_resource(AppFont(handle));
}

/// 把所有文本的字体换成嵌入的更纱黑体
///
/// 只在字体确实不同时才写入：`Mut` 仅在解引用可变时标记变更，
/// 因此稳定后不会与 `Changed` 过滤形成每帧互相触发的循环。
fn override_text_fonts(
    font: Res<AppFont>,
    mut text_fonts: Query<&mut TextFont, Changed<TextFont>>,
) {
    let target = FontSource::Handle(font.0.clone());
    for mut text_font in &mut text_fonts {
        if text_font.font != target {
            text_font.font = target.clone();
        }
    }
}

/// 字体插件
pub struct FontsPlugin;

impl Plugin for FontsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, load_embedded_font).add_systems(
            PostUpdate,
            override_text_fonts
                .before(UiSystems::Content)
                .run_if(resource_exists::<AppFont>),
        );
    }
}
