//! 唯一内嵌色板的解析与校验；运行时不访问配置文件或资源目录。

use std::sync::OnceLock;

use bevy::prelude::*;
use serde::{Deserialize, Deserializer};

use super::ThemeId;

pub(super) const PALETTES_JSON: &str = include_str!("../../../assets/themes/palettes.json");

#[derive(Deserialize)]
pub(super) struct Palette {
    pub id: ThemeId,
    pub colors: PaletteColors,
}

/// 固定字段保证缺色、拼错键名或无效十六进制颜色在加载时统一报错。
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PaletteColors {
    #[serde(deserialize_with = "deserialize_color")]
    pub canvas: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub shell: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub panel: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub header: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub input: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub button: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub hover: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub border: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub text: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub muted: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub faint: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub accent: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub accent_hover: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub on_accent: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub selected: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub selected_text: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub success: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub warning: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub danger: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub disabled_bg: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub disabled_text: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub pose_bg: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub pose_border: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub pose_text: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub flow: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub act: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub data: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub log: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub domain: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub back: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub axis_x: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub axis_y: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub axis_z: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub copy_bg: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub copy_hover: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub copy_text: Color,
}

fn deserialize_color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Color, D::Error> {
    let hex = String::deserialize(deserializer)?;
    if hex.len() != 7 || !hex.starts_with('#') {
        return Err(serde::de::Error::custom("色板颜色必须使用 #RRGGBB 格式"));
    }
    Srgba::hex(hex)
        .map(Color::Srgba)
        .map_err(serde::de::Error::custom)
}

pub(super) fn parse_palettes(json: &str) -> Result<[Palette; 4], String> {
    let palettes: [Palette; 4] = serde_json::from_str(json).map_err(|error| error.to_string())?;
    for id in ThemeId::ALL {
        if palettes.iter().filter(|palette| palette.id == id).count() != 1 {
            return Err(format!("色板必须且只能定义一次主题 {}", id.id()));
        }
    }
    Ok(palettes)
}

pub(super) fn colors(id: ThemeId) -> PaletteColors {
    static PALETTES: OnceLock<Result<[Palette; 4], String>> = OnceLock::new();
    match PALETTES.get_or_init(|| parse_palettes(PALETTES_JSON)) {
        Ok(palettes) => {
            if let Some(palette) = palettes.iter().find(|palette| palette.id == id) {
                return palette.colors;
            }
        }
        Err(error) => {
            bevy::log::error_once!("内嵌主题色板无效，使用黑白应急配色：{error}");
        }
    }
    // 开发期误改内嵌 JSON 时仍保持界面可读；正常路径的所有颜色只来自上述资源。
    // 自动化测试会拒绝损坏的资源，此处不以 panic 终止用户正在编辑的文档。
    PaletteColors::emergency(id.is_dark())
}

impl PaletteColors {
    fn emergency(dark: bool) -> Self {
        let (background, foreground) = match dark {
            true => (Color::BLACK, Color::WHITE),
            false => (Color::WHITE, Color::BLACK),
        };
        Self {
            canvas: background,
            shell: background,
            panel: background,
            header: background,
            input: background,
            button: background,
            hover: background,
            border: foreground,
            text: foreground,
            muted: foreground,
            faint: foreground,
            accent: foreground,
            accent_hover: foreground,
            on_accent: background,
            selected: background,
            selected_text: foreground,
            success: foreground,
            warning: foreground,
            danger: foreground,
            disabled_bg: background,
            disabled_text: foreground,
            pose_bg: background,
            pose_border: foreground,
            pose_text: foreground,
            flow: foreground,
            act: foreground,
            data: foreground,
            log: foreground,
            domain: foreground,
            back: foreground,
            axis_x: foreground,
            axis_y: foreground,
            axis_z: foreground,
            copy_bg: foreground,
            copy_hover: foreground,
            copy_text: background,
        }
    }
}
