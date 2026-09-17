# 日志分析核心来源

本目录按源码内嵌复用 `log-analyzer` 的分析能力，不依赖仓库外的绝对路径。

- 来源项目：`log-analyzer`
- 来源版本：0.5.8
- 来源提交：`3ca6da1129ed3929998c4e415aed3996516ec482`
- 原项目声明许可证：Apache-2.0
- 保留模块：analyzer-core、master-control-analyzer、analyzer-merger、analyzer-visualizer。
- 保留来源版本用于追溯；应用发布版本仍以根 `Cargo.toml` 为准。

本地适配增加两个绘图库的 `use_embedded_font` 接口，使用宿主提供的静态字体数据。
另增加 `statistics` 筛选模块与 `StatisticsOptions` 请求参数：暂停、错误轮次默认排除，
统一筛选常规轮次耗时图表和统计 CSV，保留完整动作明细；写出逐轮判定 JSON。
`round_detector` 暴露内部暂停标记复用函数，统计也识别尚未恢复的暂停。
原轮次检测、暂停扣除、动作统计及单轮绘图逻辑沿用来源实现。
工件节拍的整数微秒与证据分析独立实现在宿主 `src/log_analysis/workpiece.rs`，不混入旧轮次语义。
本目录不包含原项目的连接配置、密码、日志、报告、二进制或构建缓存。

CLI、动态插件发现及独立 SSH 工作流没有移植进桌面交互层。
宿主直接链接分析核心，复用自己的 SSH 工作线程；打包时无需额外携带插件库。
