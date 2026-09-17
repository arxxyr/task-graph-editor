//! Analyzer Core - ABI 稳定的插件接口
//!
//! 这个库定义了日志分析器插件的核心接口，使用 `abi_stable` 确保跨版本 ABI 兼容性。
//!
//! # 架构说明
//!
//! - `AnalyzerPlugin`: 插件必须实现的 trait
//! - `AnalyzeArgs`: 分析任务的输入参数（ABI 稳定）
//! - `AnalyzeResult`: 分析任务的输出结果（ABI 稳定）
//! - `PluginMetadata`: 插件元信息（名称、版本、作者等）
//!
//! # 插件开发示例
//!
//! ```no_run
//! use analyzer_core::*;
//! use abi_stable::{
//!     export_root_module, prefix_type::PrefixTypeTrait, rvec, sabi_extern_fn,
//!     sabi_trait::prelude::TD_Opaque, std_types::*,
//! };
//!
//! #[derive(Clone)]
//! struct MyAnalyzer;
//!
//! impl AnalyzerPlugin for MyAnalyzer {
//!     fn metadata(&self) -> PluginMetadata {
//!         PluginMetadata {
//!             name: "my-analyzer".into(),
//!             version: "0.1.0".into(),
//!             description: "My custom analyzer".into(),
//!             author: "me".into(),
//!             supported_extensions: rvec![".log".into()],
//!         }
//!     }
//!
//!     fn analyze(&self, args: AnalyzeArgs) -> RResult<AnalyzeResult, RBoxError> {
//!         // 实现分析逻辑
//!         ROk(AnalyzeResult {
//!             summary: "Analysis complete".into(),
//!             output_files: RVec::new(),
//!             timeline: timeline::Timeline {
//!                 name: "my-analyzer".into(),
//!                 source_file: args.input_file.clone(),
//!                 log_start_time: 0.0,
//!                 log_end_time: 0.0,
//!                 events: RVec::new(),
//!                 is_primary: false,
//!                 metadata: RNone,
//!             },
//!         })
//!     }
//! }
//!
//! // 导出插件模块
//! #[export_root_module]
//! fn instantiate_root_module() -> AnalyzerPluginModule_Ref {
//!     AnalyzerPluginModule { create_plugin }.leak_into_prefix()
//! }
//!
//! #[sabi_extern_fn]
//! fn create_plugin() -> AnalyzerPlugin_TO<'static, RBox<()>> {
//!     AnalyzerPlugin_TO::from_value(MyAnalyzer, TD_Opaque)
//! }
//! ```

// 允许 abi_stable 宏生成的代码风格
#![allow(non_camel_case_types)] // abi_stable 宏生成的类型使用 snake_case 后缀
#![allow(non_local_definitions)] // abi_stable 宏在常量内生成 impl 块

use abi_stable::{
    StableAbi, declare_root_module_statics, package_version_strings, sabi_trait,
    sabi_types::VersionStrings, std_types::*,
};

// 导出时间线模块
pub mod timeline;

// ============================================================================
// 核心 Trait 定义
// ============================================================================

/// 日志分析器插件 Trait
///
/// 所有插件必须实现此 trait。使用 `sabi_trait` 确保 ABI 稳定。
#[sabi_trait]
pub trait AnalyzerPlugin: Clone + Send + Sync {
    /// 获取插件元信息
    fn metadata(&self) -> PluginMetadata;

    /// 执行日志分析
    ///
    /// # 参数
    ///
    /// - `args`: 分析任务参数（输入文件、输出目录等）
    ///
    /// # 返回
    ///
    /// - `Ok(AnalyzeResult)`: 分析成功，返回结果汇总
    /// - `Err(RBoxError)`: 分析失败，返回错误信息
    fn analyze(&self, args: AnalyzeArgs) -> RResult<AnalyzeResult, RBoxError>;
}

// ============================================================================
// 数据结构定义（所有字段必须是 ABI 稳定类型）
// ============================================================================

/// 插件元信息
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct PluginMetadata {
    /// 插件名称（如 "master-control-analyzer"）
    pub name: RString,
    /// 插件版本（如 "0.1.0"）
    pub version: RString,
    /// 插件描述
    pub description: RString,
    /// 插件作者
    pub author: RString,
    /// 支持的文件扩展名（如 [".log", ".txt"]）
    pub supported_extensions: RVec<RString>,
}

/// 分析任务参数
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct AnalyzeArgs {
    /// 输入日志文件路径
    pub input_file: RString,
    /// 输出目录路径
    pub output_dir: RString,
    /// 额外参数（插件特定，JSON 格式）
    pub extra_args: ROption<RString>,
    /// 语言设置（如 "zh-CN", "en"），用于国际化
    pub locale: RString,
}

/// 分析结果（v2 - 包含时间线数据）
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct AnalyzeResult {
    /// 分析摘要（人类可读）
    pub summary: RString,
    /// 生成的输出文件列表
    pub output_files: RVec<OutputFile>,
    /// 标准化的时间线数据（v2 新增）
    pub timeline: timeline::Timeline,
}

/// 输出文件信息
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct OutputFile {
    /// 文件路径
    pub path: RString,
    /// 文件类型（如 "csv", "png", "json"）
    pub file_type: RString,
    /// 文件描述
    pub description: RString,
}

// ============================================================================
// 插件模块导出接口
// ============================================================================

/// 插件模块根结构
///
/// 每个插件动态库必须导出这个结构，包含插件工厂函数。
#[repr(C)]
#[derive(StableAbi)]
#[sabi(kind(Prefix(prefix_ref = AnalyzerPluginModule_Ref)))]
#[sabi(missing_field(panic))]
pub struct AnalyzerPluginModule {
    /// 创建插件实例的工厂函数
    #[sabi(last_prefix_field)]
    pub create_plugin: extern "C" fn() -> AnalyzerPlugin_TO<'static, RBox<()>>,
}

// 手动实现 RootModule trait（这是必需的！）
impl abi_stable::library::RootModule for AnalyzerPluginModule_Ref {
    // 声明根模块静态变量
    declare_root_module_statics! {AnalyzerPluginModule_Ref}

    /// 库的基础名称
    const BASE_NAME: &'static str = "analyzer_plugin";

    /// 库的完整名称
    const NAME: &'static str = "analyzer_plugin";

    /// 版本信息
    const VERSION_STRINGS: abi_stable::sabi_types::VersionStrings = package_version_strings!();
}

/// 获取核心库版本信息
pub fn get_version() -> VersionStrings {
    package_version_strings!()
}

// ============================================================================
// 辅助类型别名
// ============================================================================

/// ABI 稳定的 Result 类型（短别名）
pub type AbiResult<T> = RResult<T, RBoxError>;

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_creation() {
        let meta = PluginMetadata {
            name: "test-analyzer".into(),
            version: "1.0.0".into(),
            description: "Test".into(),
            author: "tester".into(),
            supported_extensions: vec![".log".into()].into(),
        };
        assert_eq!(meta.name.as_str(), "test-analyzer");
    }

    #[test]
    fn test_analyze_args_creation() {
        let args = AnalyzeArgs {
            input_file: "/path/to/log.log".into(),
            output_dir: "/output".into(),
            extra_args: RSome("{\"key\": \"value\"}".into()),
            locale: "zh-CN".into(),
        };
        assert_eq!(args.input_file.as_str(), "/path/to/log.log");
        assert_eq!(args.locale.as_str(), "zh-CN");
    }
}
