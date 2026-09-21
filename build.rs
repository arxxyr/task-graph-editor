//! 构建脚本：Windows 下把应用图标写进 exe 资源，其他平台什么也不做。
//!
//! 图标由 `scripts/generate_app_icons.py` 生成。macOS 的 `.icns` 和 Linux 的桌面入口
//! 在打包阶段处理，运行期随主题切换的部分见 `src/ui/app_icon.rs`。

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=assets/icons/app-icon.ico");

    // winresource 只在 Windows 宿主上可用；再核对目标系统，避免交叉编译到其他平台时写入资源。
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icons/app-icon.ico");
        if let Err(error) = resource.compile() {
            panic!("写入 exe 图标资源失败: {error}");
        }
    }
}
