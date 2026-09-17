//! 字体加载模块
//!
//! 分平台字体选择策略：
//! 1. Windows: 根据系统语言选择中文或英文系统字体
//! 2. Linux: `fc-list`/`fc-match` 定位 CJK 字体文件并注册，无则用英文并提示安装
//! 3. macOS: 按候选路径查找系统 CJK 字体文件并注册（PingFang 需在 AssetsV2 中动态查找）
//! 4. 兜底: 使用英文字体
//!
//! Linux 与 macOS 都必须把字体**文件数据**注册进 plotters：
//! 其 ab_glyph 后端只在自身注册表里按名字查字体，既不查 fontconfig
//! 也不查 CoreText，只给字体名而不注册，渲染时一律报 `FontUnavailable`。

use anyhow::Result;
use plotters::style::FontDesc;
use std::sync::OnceLock;

/// 字体选择结果
#[derive(Debug, Clone)]
pub enum FontChoice {
    /// 中文字体（字体名称）
    Chinese(String),
    /// 英文字体（字体名称）
    English(String),
}

/// 全局字体选择缓存
static FONT_CHOICE: OnceLock<FontChoice> = OnceLock::new();
/// 是否已显示安装提示
#[cfg(target_os = "linux")]
static INSTALL_HINT_SHOWN: OnceLock<bool> = OnceLock::new();

/// 宿主在分析前提供已嵌入的中文字体，消除系统字体与平台语言设置差异。
pub fn use_embedded_font(bytes: &'static [u8]) -> Result<()> {
    use plotters::style::{FontStyle, register_font};
    const NAME: &str = "TGE Sarasa";
    for style in [
        FontStyle::Normal,
        FontStyle::Bold,
        FontStyle::Italic,
        FontStyle::Oblique,
    ] {
        register_font(NAME, style, bytes)
            .map_err(|_| anyhow::anyhow!("注册内嵌字体失败：字体数据无效"))?;
    }
    let _ = FONT_CHOICE.set(FontChoice::Chinese(NAME.into()));
    Ok(())
}

/// 字体加载器
pub struct FontLoader {
    font_choice: FontChoice,
}

impl FontLoader {
    /// 创建字体加载器
    pub fn new() -> Result<Self> {
        let font_choice = Self::detect_font();
        Ok(Self { font_choice })
    }

    /// 检测并选择合适的字体
    fn detect_font() -> FontChoice {
        FONT_CHOICE
            .get_or_init(|| {
                #[cfg(target_os = "windows")]
                {
                    Self::detect_windows_font()
                }

                #[cfg(target_os = "linux")]
                {
                    Self::detect_linux_font()
                }

                #[cfg(target_os = "macos")]
                {
                    Self::detect_macos_font()
                }

                #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
                {
                    FontChoice::English("sans-serif".to_string())
                }
            })
            .clone()
    }

    /// Windows 字体检测
    #[cfg(target_os = "windows")]
    fn detect_windows_font() -> FontChoice {
        // 检测系统语言
        let is_chinese = Self::is_windows_chinese_locale();

        if is_chinese {
            // 按优先级尝试中文字体
            let chinese_fonts = ["Microsoft YaHei", "SimHei", "SimSun", "KaiTi", "FangSong"];

            for font in &chinese_fonts {
                if Self::windows_font_exists(font) {
                    eprintln!("[字体] Windows 中文环境，使用: {}", font);
                    return FontChoice::Chinese(font.to_string());
                }
            }

            eprintln!("[字体] Windows 中文环境，未找到中文字体，使用英文");
        } else {
            eprintln!("[字体] Windows 英文环境，使用系统字体");
        }

        // 英文字体
        let english_fonts = ["Segoe UI", "Arial", "Tahoma"];
        for font in &english_fonts {
            if Self::windows_font_exists(font) {
                return FontChoice::English(font.to_string());
            }
        }

        FontChoice::English("sans-serif".to_string())
    }

    /// 检测 Windows 是否为中文环境
    #[cfg(target_os = "windows")]
    fn is_windows_chinese_locale() -> bool {
        use std::process::Command;

        // 方法1: 检查 LANG 环境变量
        if let Ok(lang) = std::env::var("LANG") {
            if lang.to_lowercase().contains("zh") || lang.to_lowercase().contains("chinese") {
                return true;
            }
        }

        // 方法2: 使用 PowerShell 获取系统语言
        if let Ok(output) = Command::new("powershell")
            .args(["-Command", "(Get-Culture).Name"])
            .output()
        {
            let locale = String::from_utf8_lossy(&output.stdout);
            if locale.to_lowercase().contains("zh") {
                return true;
            }
        }

        // 方法3: 检查代码页
        if let Ok(output) = Command::new("cmd").args(["/c", "chcp"]).output() {
            let codepage = String::from_utf8_lossy(&output.stdout);
            // 936 = GBK, 65001 = UTF-8 (可能是中文)
            if codepage.contains("936") {
                return true;
            }
        }

        false
    }

    /// 检查 Windows 字体是否存在
    #[cfg(target_os = "windows")]
    fn windows_font_exists(font_name: &str) -> bool {
        use std::path::Path;

        // 检查 Windows 字体目录
        if let Ok(windir) = std::env::var("WINDIR") {
            let fonts_dir = Path::new(&windir).join("Fonts");

            // 常见字体文件名映射
            let font_files: &[&str] = match font_name {
                "Microsoft YaHei" => &["msyh.ttc", "msyh.ttf", "msyhbd.ttc"],
                "SimHei" => &["simhei.ttf"],
                "SimSun" => &["simsun.ttc", "simsun.ttf"],
                "KaiTi" => &["simkai.ttf"],
                "FangSong" => &["simfang.ttf"],
                "Segoe UI" => &["segoeui.ttf", "segoeuib.ttf"],
                "Arial" => &["arial.ttf", "arialbd.ttf"],
                "Tahoma" => &["tahoma.ttf", "tahomabd.ttf"],
                _ => &[],
            };

            for file in font_files {
                if fonts_dir.join(file).exists() {
                    return true;
                }
            }
        }

        // 兜底：假设常见字体存在
        matches!(
            font_name,
            "Microsoft YaHei" | "SimHei" | "Arial" | "Segoe UI"
        )
    }

    /// Linux 字体检测
    ///
    /// plotters 的 ab_glyph 后端无法通过字体名称解析字体文件（不使用 fontconfig），
    /// 因此需要手动通过 fc-match 获取字体文件路径，读取字体数据后用 register_font 注册。
    #[cfg(target_os = "linux")]
    fn detect_linux_font() -> FontChoice {
        use std::process::Command;

        // 使用 fc-list 检测 CJK 字体
        if let Ok(output) = Command::new("fc-list").args([":lang=zh"]).output() {
            let fonts = String::from_utf8_lossy(&output.stdout);

            if !fonts.trim().is_empty() {
                let preferred_fonts = [
                    "Noto Sans CJK SC",
                    "Noto Sans CJK",
                    "WenQuanYi Micro Hei",
                    "WenQuanYi Zen Hei",
                    "Source Han Sans SC",
                    "Source Han Sans CN",
                    "Droid Sans Fallback",
                ];

                for font_name in &preferred_fonts {
                    if fonts.contains(font_name)
                        && let Some(path) = Self::fc_match_font_file(font_name)
                        && Self::register_font_from_file(font_name, &path)
                    {
                        eprintln!("[字体] Linux 注册 CJK 字体: {} ({})", font_name, path);
                        return FontChoice::Chinese(font_name.to_string());
                    }
                }
                eprintln!("[字体] 检测到 CJK 字体但均无法注册，回退到英文字体");
            }
        }

        // 未找到可用 CJK 字体，显示安装提示
        Self::show_linux_install_hint();

        // 尝试英文字体
        let english_fonts = ["DejaVu Sans", "Liberation Sans", "Noto Sans"];
        for font_name in &english_fonts {
            if let Some(path) = Self::fc_match_font_file(font_name)
                && Self::register_font_from_file(font_name, &path)
            {
                eprintln!("[字体] Linux 注册英文字体: {} ({})", font_name, path);
                return FontChoice::English(font_name.to_string());
            }
        }

        // 最终兜底
        eprintln!("[字体] Linux 使用兜底字体: sans-serif");
        FontChoice::English("sans-serif".to_string())
    }

    /// 使用 fc-match 获取字体文件路径
    #[cfg(target_os = "linux")]
    fn fc_match_font_file(font_name: &str) -> Option<String> {
        use std::process::Command;

        let output = Command::new("fc-match")
            .args([font_name, "--format=%{file}"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() || !std::path::Path::new(&path).exists() {
            return None;
        }

        Some(path)
    }

    /// 读取字体文件并注册到 plotters
    ///
    /// plotters 的 ab_glyph 后端无法通过字体名称查找字体文件
    /// （Linux 不走 fontconfig，macOS 不走 CoreText），
    /// 必须手动读取字体数据并通过 register_font 注册。
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn register_font_from_file(font_name: &str, font_path: &str) -> bool {
        use plotters::style::FontStyle;
        use plotters::style::register_font;

        let font_data = match std::fs::read(font_path) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("[字体] 读取字体文件失败 {}: {}", font_path, e);
                return false;
            }
        };

        // 将数据泄露为 'static 引用（字体只注册一次，泄露可接受）
        let leaked: &'static [u8] = Box::leak(font_data.into_boxed_slice());

        match register_font(font_name, FontStyle::Normal, leaked) {
            Ok(()) => true,
            Err(e) => {
                let _ = e;
                eprintln!("[字体] 注册字体失败 {}: 字体数据无效", font_name);
                false
            }
        }
    }

    /// 验证字体是否可被 plotters 位图后端渲染
    ///
    /// 创建一个小的内存位图尝试渲染文字，如果失败说明字体不可用
    fn verify_font_with_plotters(font_name: &str) -> bool {
        use plotters::prelude::*;

        let mut buffer = vec![0u8; 100 * 100 * 3];
        let root = BitMapBackend::with_buffer(&mut buffer, (100, 100)).into_drawing_area();
        if root.fill(&WHITE).is_err() {
            return false;
        }
        let font: FontDesc = (font_name, 12.0).into();
        if root
            .draw_text("测试", &font.color(&BLACK), (10, 10))
            .is_err()
        {
            return false;
        }
        root.present().is_ok()
    }

    /// 显示 Linux CJK 字体安装提示
    #[cfg(target_os = "linux")]
    fn show_linux_install_hint() {
        INSTALL_HINT_SHOWN.get_or_init(|| {
            eprintln!();
            eprintln!("╔══════════════════════════════════════════════════════════════╗");
            eprintln!("║  [字体] 未检测到中文字体，图表将使用英文字体                 ║");
            eprintln!("║                                                              ║");
            eprintln!("║  如需显示中文，请安装 CJK 字体：                             ║");
            eprintln!("║                                                              ║");
            eprintln!("║  Ubuntu/Debian:                                              ║");
            eprintln!("║    sudo apt install fonts-noto-cjk                           ║");
            eprintln!("║                                                              ║");
            eprintln!("║  Fedora/RHEL:                                                ║");
            eprintln!("║    sudo dnf install google-noto-sans-cjk-fonts               ║");
            eprintln!("║                                                              ║");
            eprintln!("║  Arch Linux:                                                 ║");
            eprintln!("║    sudo pacman -S noto-fonts-cjk                             ║");
            eprintln!("║                                                              ║");
            eprintln!("║  Alpine:                                                     ║");
            eprintln!("║    apk add font-noto-cjk                                     ║");
            eprintln!("╚══════════════════════════════════════════════════════════════╝");
            eprintln!();
            true
        });
    }

    /// macOS 候选 CJK 字体（显示名 → 候选文件路径，按优先级排列）
    ///
    /// plotters 只认注册进去的字体数据，字体名仅作为查表键，
    /// 因此这里的“显示名”不必与系统字体名一致，只需与注册时一致。
    #[cfg(target_os = "macos")]
    const MACOS_CJK_FONTS: &'static [(&'static str, &'static [&'static str])] = &[
        (
            "Hiragino Sans GB",
            &["/System/Library/Fonts/Hiragino Sans GB.ttc"],
        ),
        (
            "STHeiti",
            &[
                "/System/Library/Fonts/STHeiti Medium.ttc",
                "/System/Library/Fonts/STHeiti Light.ttc",
            ],
        ),
        (
            "Songti SC",
            &["/System/Library/Fonts/Supplemental/Songti.ttc"],
        ),
        (
            "Arial Unicode MS",
            &["/System/Library/Fonts/Supplemental/Arial Unicode.ttf"],
        ),
    ];

    /// macOS 候选英文字体（显示名 → 候选文件路径）
    #[cfg(target_os = "macos")]
    const MACOS_LATIN_FONTS: &'static [(&'static str, &'static [&'static str])] = &[
        (
            "Helvetica Neue",
            &["/System/Library/Fonts/HelveticaNeue.ttc"],
        ),
        ("Helvetica", &["/System/Library/Fonts/Helvetica.ttc"]),
        ("Arial", &["/System/Library/Fonts/Supplemental/Arial.ttf"]),
    ];

    /// macOS 字体检测
    ///
    /// 与 Linux 同理：ab_glyph 不查询 CoreText，只按名字在自身注册表里找，
    /// 因此必须先把字体文件读进来 `register_font` 注册，否则渲染时
    /// 一律报 `FontUnavailable`。
    #[cfg(target_os = "macos")]
    fn detect_macos_font() -> FontChoice {
        if Self::is_macos_chinese_locale() {
            // PingFang 在新版 macOS 上位于带哈希的 AssetsV2 目录，需动态查找
            if let Some(path) = Self::find_macos_pingfang()
                && Self::register_font_from_file("PingFang SC", &path)
            {
                eprintln!("[字体] macOS 注册 CJK 字体: PingFang SC ({})", path);
                return FontChoice::Chinese("PingFang SC".to_string());
            }

            for (font_name, paths) in Self::MACOS_CJK_FONTS {
                for path in *paths {
                    if std::path::Path::new(path).exists()
                        && Self::register_font_from_file(font_name, path)
                    {
                        eprintln!("[字体] macOS 注册 CJK 字体: {} ({})", font_name, path);
                        return FontChoice::Chinese(font_name.to_string());
                    }
                }
            }

            eprintln!("[字体] macOS 中文环境但未能注册任何 CJK 字体，回退到英文字体");
        }

        for (font_name, paths) in Self::MACOS_LATIN_FONTS {
            for path in *paths {
                if std::path::Path::new(path).exists()
                    && Self::register_font_from_file(font_name, path)
                {
                    eprintln!("[字体] macOS 注册英文字体: {} ({})", font_name, path);
                    return FontChoice::English(font_name.to_string());
                }
            }
        }

        eprintln!("[字体] macOS 使用兜底字体: sans-serif");
        FontChoice::English("sans-serif".to_string())
    }

    /// 查找 PingFang.ttc
    ///
    /// macOS 15+ 把 PingFang 挪进了 `/System/Library/AssetsV2/` 下带哈希的资产目录，
    /// 老路径 `/System/Library/Fonts/PingFang.ttc` 在新系统上不存在，故两处都找。
    #[cfg(target_os = "macos")]
    fn find_macos_pingfang() -> Option<String> {
        const LEGACY_PATH: &str = "/System/Library/Fonts/PingFang.ttc";
        if std::path::Path::new(LEGACY_PATH).exists() {
            return Some(LEGACY_PATH.to_string());
        }

        // AssetsV2 布局：<资产根>/<哈希>.asset/AssetData/PingFang.ttc
        const ASSET_ROOT: &str = "/System/Library/AssetsV2/com_apple_MobileAsset_Font8";
        let entries = std::fs::read_dir(ASSET_ROOT).ok()?;
        for entry in entries.flatten() {
            let candidate = entry.path().join("AssetData").join("PingFang.ttc");
            if candidate.is_file() {
                return candidate.to_str().map(str::to_string);
            }
        }

        None
    }

    /// 检测 macOS 是否为中文环境
    #[cfg(target_os = "macos")]
    fn is_macos_chinese_locale() -> bool {
        use std::process::Command;

        // 检查 LANG 环境变量
        if let Ok(lang) = std::env::var("LANG")
            && lang.to_lowercase().contains("zh")
        {
            return true;
        }

        // 使用 defaults 命令获取系统语言
        if let Ok(output) = Command::new("defaults")
            .args(["read", "-g", "AppleLanguages"])
            .output()
        {
            let languages = String::from_utf8_lossy(&output.stdout);
            if languages.to_lowercase().contains("zh") {
                return true;
            }
        }

        false
    }

    /// 获取字体文件路径（兼容旧 API，现在返回 None）
    #[allow(dead_code)]
    pub fn font_path(&self) -> Option<&std::path::PathBuf> {
        None
    }

    /// 创建字体描述符（用于 plotters）
    ///
    /// # 参数
    /// * `size` - 字体大小
    ///
    /// # 返回
    /// 字体描述符
    pub fn font_desc(&self, size: i32) -> FontDesc<'static> {
        let font_name: &'static str = match &self.font_choice {
            FontChoice::Chinese(name) => Self::leak_string(name),
            FontChoice::English(name) => Self::leak_string(name),
        };

        (font_name, size as f64).into()
    }

    /// 将 String 转换为 'static str（泄露内存，但字体名只会初始化一次）
    fn leak_string(s: &str) -> &'static str {
        use std::sync::Mutex;

        static CACHE: OnceLock<Mutex<std::collections::HashMap<String, &'static str>>> =
            OnceLock::new();

        let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
        let mut map = cache.lock().unwrap();

        if let Some(&leaked) = map.get(s) {
            return leaked;
        }

        let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
        map.insert(s.to_string(), leaked);
        leaked
    }

    /// 检查是否使用中文字体
    #[allow(dead_code)]
    pub fn is_chinese_font(&self) -> bool {
        matches!(self.font_choice, FontChoice::Chinese(_))
    }

    /// 检查字体是否可用（兼容旧 API）
    #[allow(dead_code)]
    pub fn is_available(&self) -> bool {
        true // 总是返回 true，因为我们有兜底方案
    }

    /// 验证检测到的字体是否可被 plotters 渲染
    ///
    /// 在程序启动时调用，及早发现字体问题，避免分析完成后才报错
    #[allow(dead_code)]
    pub fn validate(&self) -> Result<()> {
        let font_name = match &self.font_choice {
            FontChoice::Chinese(name) => name.as_str(),
            FontChoice::English(name) => name.as_str(),
        };

        if !Self::verify_font_with_plotters(font_name) {
            // macOS/Windows 自带 CJK 字体，渲染失败通常是注册环节出问题，
            // 提示安装字体只会误导，故按平台给不同建议
            #[cfg(target_os = "linux")]
            anyhow::bail!(
                "字体 '{}' 无法渲染。请安装 CJK 字体:\n\
                 \n\
                 Ubuntu/Debian: sudo apt install fonts-noto-cjk\n\
                 Fedora/RHEL:   sudo dnf install google-noto-sans-cjk-fonts\n\
                 Arch Linux:    sudo pacman -S noto-fonts-cjk\n\
                 Alpine:        apk add font-noto-cjk",
                font_name
            );

            #[cfg(not(target_os = "linux"))]
            anyhow::bail!(
                "字体 '{}' 无法渲染：系统字体文件未能注册到 plotters。",
                font_name
            );
        }

        Ok(())
    }
}

impl Default for FontLoader {
    fn default() -> Self {
        Self::new().unwrap_or(Self {
            font_choice: FontChoice::English("sans-serif".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_loader_creation() {
        let loader = FontLoader::new();
        assert!(loader.is_ok());
    }

    #[test]
    fn test_font_desc() {
        let loader = FontLoader::default();
        let desc = loader.font_desc(14);
        assert_eq!(desc.get_size(), 14.0);
    }
}
