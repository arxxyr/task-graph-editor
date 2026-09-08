# 流程图执行器协议核验

核验日期：2026-09-08。执行器来源：用户指定的
`/Users/loosqk/repo/cpp/ros2/master_control/src/master_control`，提交 `af05e31`，核验时工作区干净。
本文只读检查源码和真实 JSON，没有编译、启动执行器或调用 ROS2、HTTP、SSH。
路径链接指向这份本机源码；移动或升级执行器后应重新核对。

## 依据与边界

实际 [NodeFactory 注册表](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/node_factory.cpp:104)
注册 **44** 种类型，未知类型创建时报错。`config/node_definitions_v2.json` 只有 **31** 种，
不是足以直接生成表单或阻止保存的 JSON Schema；字段与实际构造器已有差异。
本文按注册表逐项检查构造器，并对延迟校验的节点继续检查 `execute`。

编辑器仍需保存未编辑的完整 JSON、未知字段及字符串，不应把下表当成删减字段的白名单。
下表只列创建节点所需的字段、确定的缺省行为和关键边界，不声称穷尽机器人业务接口。
缺少真实 context、外部服务、文件或运行环境时，静态校验不能保证任务成功执行。

大多数工具节点使用 [getInputs](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/include/task_graph/node_utils.hpp:16)：
`inputs` 存在就优先读取，否则读取 `params`。编辑只有 `params` 的旧节点时应修改原字段，
不能新增不完整 `inputs` 悄悄遮蔽其余参数。`loop` 和 condition 叶节点单独读取 `inputs`。
`outputs` 通常是“输出名称 → context 变量名”的字符串映射，节点之间交换的是 context 数据。

## 44 种注册节点

以下 `I` 表示节点 `inputs` 对象中的键（支持回退时也可在 `params`）；`O` 表示节点顶层 `outputs` 的键。
“构造必填”是实际检查或 `.at()` 读取；“运行必需”表示可构造但执行会失败或无法产生所需结果。
“需填写”字段不能由编辑器猜测机器地址、动作参数、文件路径、位姿或业务变量。
默认值只在原字段缺失时描述执行器行为，不应自动补进已有节点。

| 类型 | 实际输入与输出约束 | 缺省行为、表单与模板线索 | 源码 |
| --- | --- | --- | --- |
| `condition` | 叶节点构造必填 I.condition 字符串；可选 O.result 字符串。节点同时有 nodes/edges 则变成容器，不读取叶条件 | 叶模板 I.condition=`"false"`；容器模板见后文；不要因类型相同强制两种形态相互转换 | [工厂:104](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/node_factory.cpp:104)、[叶构造:10](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/condition_node.cpp:10) |
| `ros2_action` | 构造必填节点顶层 action_type 或 I.action_type 字符串，顶层优先；对应适配器必须注册 | I.timeout_sec 默认 60，可为数字或表达式字符串；动作和位姿需填写；不能只用空 inputs 生成可运行节点 | [构造:27](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/ros2_action_node.cpp:27)、[适配器注册:1334](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/include/task_graph/nodes/action_adapters.hpp:1334) |
| `loop` | 构造必填 I.condition 字符串，以及节点顶层 nodes/edges 数组 | I.iteration_var 默认 `i`；I.checkpoint_per_iteration 默认 false 但保存仍未实现；无 max_iterations 执行限制。模板 condition=`"false"` 与空直通子图 | [构造:20](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/loop_node.cpp:20)、[循环:165](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/loop_node.cpp:165) |
| `sequence` | 构造必填节点顶层 nodes/edges 数组；执行需要 `_entry` 的普通后继 | 可生成空数组与 `_entry → _exit`；条件叶的 label 分支也合法 | [构造:19](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/sequence_node.cpp:19)、[后继:321](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/sequence_node.cpp:321) |
| `parallel` | 构造必填 nodes 或 branches 数组，branches 优先；每项是带 id/type 的实际节点对象 | I.policy 优先，兼容顶层 policy，默认 all_success；可选 any_success/first_success。直接子节点并行，edges 不决定调度；空 nodes 模板直接成功，随后可添加分支 | [构造:160](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/parallel_node.cpp:160)、[创建分支:557](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/parallel_node.cpp:557) |
| `preplan_navigation` | 运行必需 I.pose_string 字符串或 context 占位符，解析出 chassis_pose；需要 ROS2 LifecycleNode | I.cmd_code 默认 5、plan_type 默认 100、rotate_mode 默认 0，整数可用表达式；ignore_failure 默认 false；可选 O.success 等。机器位姿需填写 | [构造:17](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/preplan_navigation_node.cpp:17)、[执行:77](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/preplan_navigation_node.cpp:77) |
| `multi_pose_generator` | 运行必需字符串 I.object_id（解析后为 `0` 或 `6`）、pose_string、levels、heights、right_direction；有有效 pose_offset_configs 时可省 lean_forward_angle，否则需填写它 | levels/heights 是字符串化数组或变量，层索引为 1 到 heights 长度；boxes_count 字符串默认 `1`，boxes_length 字符串默认 `0.607`；可选 O.poses；真实位姿与几何不可默认 | [执行:171](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/multi_pose_generator_node.cpp:171) |
| `behavior_tree` | 构造必填 I.tree_file 字符串，解析后非空且本机文件存在；需要 ROS2/BT 环境 | tick_period_ms 默认 10、cache_tree 默认 true；可选 I.blackboard_inputs/blackboard_outputs；路径与黑板映射需填写 | [构造:729](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/behavior_tree_node.cpp:729) |
| `pop_array` | 构造必填 I.array_var 字符串、O.element 字符串；运行需对应 context 数组 | I.update_array 默认 true，可选 O.remaining_length；需要数组变量和输出变量，不自动创造数据源 | [构造:9](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/pop_array_node.cpp:9) |
| `peek_array` | 构造必填 I.array_var、O.element 字符串 | I.index 默认 0；I.index_var 字符串存在时优先；运行检查数组与下标 | [构造:9](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/peek_array_node.cpp:9) |
| `get_array_length` | 构造必填 I.variable、O.length 字符串 | 需选择已有数组变量和输出变量；不使用 schema 之外的猜测字段 | [构造:9](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/get_array_length_node.cpp:9) |
| `array_index` | 构造必填 I.array、I.index 任意 JSON 值和 O.result 字符串；运行解析后需数组/可解析数组字符串及数值下标 | 可用字面数组或 context 占位符；运行下标必须非负且在范围内；输出变量需填写 | [构造及执行:10](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/array_index_node.cpp:10) |
| `object_get` | 构造必填 I.object、I.key 和 O.result 字符串映射；值在运行时解析 | 需要实际对象、键或对应变量；不把 JSON 值全转换为字符串 | [构造:12](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/object_get_node.cpp:12) |
| `concat_arrays` | 构造必填 I.arrays 与 O.result 字符串；arrays 整体先解析表达式，结果应为数组，各项为数组或数组字符串 | 可以提供字面数组集合或返回数组集合的 context 引用，但输出变量需填写 | [构造及执行:10](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/concat_arrays_node.cpp:10) |
| `box_task_notify` | 构造必填 I.server_url；运行 operation=submit 时必需 event_type，wait_existing 时必需 task_id | operation 默认 submit；timeout_ms=300000、poll_interval_ms=1000；地址、站点、任务类型等需填写；自动 request_id 包含节点 ID | [构造:69](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/box_task_notify_node.cpp:69)、[执行:82](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/box_task_notify_node.cpp:82) |
| `box_task_acquire` | 构造必填 I.server_url，表达式解析后为字符串 | timeout_ms 默认 0、poll_interval_ms 默认 1000；HTTP 任务服务依赖，不能默认地址 | [构造:103](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/box_task_acquire_node.cpp:103) |
| `box_task_finish` | 构造必填 I.server_url、I.request_id | I.state 默认 succeeded、message 默认空串；请求身份需填写 | [构造:14](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/box_task_finish_node.cpp:14) |
| `area_guard` | 构造必填 I.server_url、mode、area；运行 mode 为 acquire/release/manual_release/status | enabled 默认 true；robot 默认 load_robot；request_id 默认节点 ID；timeout_ms=0、poll_interval_ms=500；站点/区域必须依据业务填写 | [构造:91](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/area_guard_node.cpp:91)、[执行:110](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/area_guard_node.cpp:110) |
| `persist_chassis_pose` | 运行必填非空 I.context_key；启用对应写回时需要 main_task_id/calibration_task_id、有效新鲜位姿及目标任务状态 | 两个 write_back 开关默认 false（dry-run）；max_pose_age_ms 默认 1000 且不得负数；ID 与路径相关，不提供真实默认目标 | [构造:117](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/persist_chassis_pose_node.cpp:117)、[执行:172](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/persist_chassis_pose_node.cpp:172) |
| `slot_pose_library` | 运行必填 I.get_observe、put_observe、right_direction、boxes_length、levels、heights；有效 pose_offset_configs 缺失时还需 lean_forward_angle | right_direction 为 x+/x-/y+/y-；pick_to_put_offset 默认 0.10。注意 boxes_length 即使解析函数有 fallback 1.017，键仍经 `.at` 必填；机器几何需填写 | [执行:218](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/slot_pose_library_node.cpp:218) |
| `slot_flow_scheduler` | 运行必填 I.action；支持 init/select/mark_pick/mark_put/mark_moving/box_done/select_final_put；各动作所需状态、位姿库与 slot_id 必须存在 | state_var 默认 slot_flow_state、capacity 默认 8；mark_moving/box_done 的 slot_id 必填，范围 1..4；initial_slot_states 若有必须为对象；不能默认业务状态 | [构造:61](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/slot_flow_scheduler_node.cpp:61)、[分发:543](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/slot_flow_scheduler_node.cpp:543) |
| `set_variable` | 构造必填 I.variable 字符串，以及非 null 的 I.value 或非空 I.value_from；value_from 非空时运行优先 | I.value_type 默认 auto；变量名与值需填写。不能以 `{value:null}` 冒充完整模板 | [构造:9](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/set_variable_node.cpp:9) |
| `copy_variable` | 构造必填 I.source、I.target 字符串；源 context 变量需实际存在 | 都是变量名，不是节点 ID；复制节点时保持变量含义 | [构造:9](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/copy_variable_node.cpp:9) |
| `increment` | 构造必填 I.variable 字符串；step 和 initial_value 若有按 C++ int 读取 | step 默认 1、initial_value 默认 0；变量需填写，可显式把 step=0 用于草稿但不是执行器缺省值 | [构造:9](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/increment_node.cpp:9) |
| `log` | 构造必填 I.message 字符串；可选 level 和字符串数组 variables | level 默认 info，可选 debug/info/warn/error；完整无动作模板 `{message:"",level:"info"}` | [构造:10](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/log_node.cpp:10) |
| `delay` | 构造必填 I.duration_ms，必须数值，浮点转 int；转换后不得负数 | 完整无动作模板 duration_ms=0；不是表达式字段，不能因为 UI 类型为数字就转存 `"{duration}"` | [构造:11](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/delay_node.cpp:11) |
| `wait_condition` | 构造必填 I.condition 字符串；可选 fail_condition 也须字符串 | poll_interval_ms 默认 100 且大于 0，timeout_ms 默认 0 且非负，两者允许数字或数值表达式；可生成 condition=`"true"` 的即时结束模板 | [构造:39](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/wait_condition_node.cpp:39) |
| `compare` | 构造必填 I.left、operator、right 和 O.result 字符串；operator 为 ==/eq/!=/ne/</lt/<=/le/>/gt/>=/ge | 左右值可解析 context 占位符；顺序比较要求数值或字符串兼容；输出变量需填写 | [构造:10](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/compare_node.cpp:10) |
| `is_empty` | 构造必填 I.variable 和 **O.result** 字符串 | 元数据的 O.is_empty/O.length 与实际构造不符；必须用 result | [构造:9](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/is_empty_node.cpp:9) |
| `compute` | 构造必填 I.left、operator、right 和 O.result；operator 为 +、-、*、/、% | 运行左右值解析成数值；除/模零失败，% 转 C++ int；输出变量需填写 | [构造:11](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/compute_node.cpp:11) |
| `mock_action` | 构造必填 I.action_name 字符串；可选 I.duration_ms 数字、success 布尔、result_value 任意值、O.result 字符串 | duration_ms 默认 0、success 默认 true；可生成 action_name=`mock`；实际不是 should_succeed/output_value | [构造:10](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/mock_action_node.cpp:10) |
| `print_variable` | 构造必填非空 I.variables 字符串数组 | level 默认 info，合法 debug/info/warn/error；format 默认 compact，可选 pretty；需填写变量名 | [构造:10](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/print_variable_node.cpp:10) |
| `extract_waist_height` | 构造可缺字段但会告警；运行需 I.pose_string 字符串解析出 waist_pose.position.x；O.height 可选，缺失就不写回 | 位姿/变量需填写。此实现仍只替换简单 `{var}`，不能承诺支持 `{a.b}` | [构造及执行:11](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/extract_waist_height_node.cpp:11) |
| `extract_pose_meta` | 构造可缺字段但会告警；运行需 I.pose_string 为有效 JSON 位姿；O.level、O.box_index 可选 | 位姿/变量需填写；此实现仍只替换简单 `{var}` | [构造及执行:11](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/extract_pose_meta_node.cpp:11) |
| `max_value` | I.left/I.right 若存在必须是字符串；缺失可构造但运行求值失败；O.result 可选 | 可生成 left=`"0"`、right=`"0"` 的无输出草稿；有用结果需填写输出变量；不能把字符串数值改成 JSON number | [构造及执行:12](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/max_value_node.cpp:12) |
| `set_waist_height` | I.pose_string 和 I.height 若存在须字符串；运行需有效位姿与数值高度；O.result 可选 | 缺字段只是构造告警，运行仍会失败；位姿和高度需填写，不提供机器人默认值 | [构造:10](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/set_waist_height_node.cpp:10)、[执行:92](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/set_waist_height_node.cpp:92) |
| `publish_task_log` | 无构造必填 I；message/log_level/task_name/task_id/node_name 若有为字符串；status/result 为 int8、progress 为 float | message 空串、log_level=INFO、task_name=task_graph；需 ROS2 发布环境；node_name 空时用节点 ID，属于显示/日志含义 | [构造:11](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/publish_task_log_node.cpp:11) |
| `user_confirmation` | 无构造必填 I；options 若有各元素为字符串；timeout_seconds 为 int；可选 O.response/O.response_index | title=确认、message=请确认操作、options 缺失/空时为确认/取消、timeout_seconds=30；运行需操作员交互环境 | [构造:13](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/user_confirmation_node.cpp:13) |
| `pause_task` | 无构造必填 I；可选 message/level 字符串 | message 默认“任务已暂停，等待操作员确认继续”，level=info；此节点会暂停任务，不把空配置作为无动作模板 | [构造:14](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/pause_task_node.cpp:14) |
| `box_task_inventory` | 构造必填 I.server_url；mode=reset/recover/reset_station_batch，其他键透传业务 payload | 默认 mode=reset、retry_count=5、retry_initial_delay_ms=1000、retry_max_delay_ms=5000；次数/初始延时非负且最大延时不小于初始；业务请求需填写 | [构造:57](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/box_task_inventory_node.cpp:57)、[缺省成员:27](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/include/task_graph/nodes/utils/box_task_inventory_node.hpp:27) |
| `leak_test` | 无构造必填 I；可选 action_name 字符串，O.started 字符串 | action_name 默认 leak_test_start；执行触发实际设备动作，不自动作为空行为模板 | [构造:9](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/leak_test_node.cpp:9)、[默认:91](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/include/task_graph/nodes/utils/leak_test_node.hpp:91) |
| `get_leak_status` | 无构造必填 I；可选 action_name 字符串、timeout_sec 数字、poll_interval_ms 整数；可选 O.status/is_testing/is_completed/passed/test_result/put_error | action_name=leak_test_start、timeout_sec=120、poll_interval_ms=500；运行依赖设备状态 | [构造:12](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/get_leak_status_node.cpp:12)、[默认:88](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/include/task_graph/nodes/utils/get_leak_status_node.hpp:88) |
| `leak_test_prepare` | 无 JSON 构造必填 I，但必须有 LifecycleNode；可选 service_name 字符串、command 整数/表达式、timeout_sec 数字、wait_result 布尔；可选 O.success/O.message | service_name=leak_test_prepare、command=1、timeout_sec=30、wait_result=true。command=1 会触发设备急停复位，不能作为通用草稿默认动作 | [构造:13](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/leak_test_prepare_node.cpp:13)、[执行:64](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/utils/leak_test_prepare_node.cpp:64) |
| `subtask` | 构造必填节点顶层 source 字符串；inputs 为子任务入参映射、outputs 为字符串输出映射 | 引用外部文件，不等于内联 nodes/edges；checkpoint 可为布尔或 `{enabled:bool}`。需填写真实子任务文件，不递归打开或改写外部文件 | [解析:26](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/subtask_node.cpp:26) |

正式 ROS2 action 适配器为 navigation、waist_control、head_control、double_arm、joint_trajectory；
另注册 navigation_always_fail/navigation_retry_success 作为测试类型。
每种适配器的位姿、关节、轨迹与动作参数见 action_adapters.hpp，创建入口必须让用户选择并填写或复制已有节点，
不能仅依靠“ros2_action”类型给出通用机器人动作模板。

编辑器实现位于 [graph_schema.rs](/Users/loosqk/repo/rust/task-graph-editor/src/model/graph_schema.rs)，
目录与草稿覆盖全部 44 种类型，完整基础模板为 condition 叶、loop、sequence、parallel、log、delay、
wait_condition、mock_action。业务必填值以空字符串或 null 留待填写；校验失败显示具体字段。
publish_task_log、user_confirmation、pause_task、leak_test、get_leak_status、leak_test_prepare
实际没有必填 JSON 输入，因此不能伪造必填规则拦截它们；目录明确说明其环境依赖与执行效果。
“静态校验通过”只表示已核对的字段满足静态约束，不代表机器人端可执行或业务参数正确。

新增/修改属性只验证相关节点，新建子树验证所有实际子节点，历史节点不因未编辑业务字段而阻止 context 保存。
校验不执行表达式、不检查编辑器本机是否存在机器人端文件，也不规范化原始 JSON。
未知类型允许原样保留与编辑，但新建目录不会冒充已核验类型；界面需显示未核验提示。
比较节点的 ==/!= 支持 null、空字符串等合法 JSON 字面量；不能把所有 null 一律当占位错误。

验证证据：独立纯 Rust harness 直接引用 graph_schema.rs，11 项单测通过，Clippy 全目标零警告。
另外对下文三个权威原始样例只读逐节点运行静态校验，分别检查 53、451、615 个节点，均无校验错误；
这证明现有样例兼容性，仍不等同于运行机器人任务。

## 执行图语义与编辑校验

根图显式 `entry_point` 位于 `config.entry_point`，不是 inputs。
存在时执行器直接使用；缺失时按边数组顺序选择首条 `_entry → 非_exit`。
显式入口必须是本层真实节点，不能是虚拟 `_entry/_exit`。
顶层按队列遍历且带 visited，顶层回边不代表重复执行；不要把自动布局的拓扑层次称为并发依赖屏障。
依据：[parseConfig](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/task_graph_executor.cpp:331)、
[执行循环](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/task_graph_executor.cpp:520)。

sequence/loop/condition 容器使用本层 id→节点映射；跨层同名实际存在且不会由这些映射相互覆盖。
sequence/loop 的普通边和 label 边分开存储；条件叶结果对应 true/false 标签，default 可用于后备后继。
即使 label 是空字符串，它仍是标签边；入口必须完全不含 label 字段。
sequence 执行时要求普通 `_entry` 后继；loop 每次进入循环体时要求它，condition=false 可以跳过整个循环体。
sequence/loop 的空子图可使用 `_entry → _exit`；condition 容器构造时必须有 `_entry` 边，
空子图可用缺省/default 条件直接连接到 `_exit`。parallel 子图为空时直接成功，完全不要求入口边。
新建无标签平行出边或同标签重复边会产生第一条/最后一条生效的歧义，应在编辑器说明并避免意外生成；
原有边仍保留独立身份，不能因为 `(from,to)` 相同就合并。

condition 容器的入口边需要编辑 `condition` 和 `priority`，仅编辑 from/to 不足以定义分支。
`priority` 实际读取 C++ `int`，新增值应限定 int32；字符串、浮点、超范围值不应被默默转换。
非入口边按第一条普通后继执行，入口排序规则为：default 最后，其余 priority 降序，同优先级按 to 字典序。
default 缺失时如果没有条件命中会失败；保留既有格式，不能按过时校验器强制所有叶 condition 变成容器。
依据：[条件容器排序](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/condition_container_node.cpp:250)。

parallel 的 edges 在实际调度中完全未使用；每个直接子节点是一条并行分支。
其 branches 与 nodes 数组元素同为带 id/type 的完整节点对象，没有隐式 sequence 包装。
只要 branches 字段存在就优先，包括非法 null（随后直接报类型错误，不回退到 nodes）。
同时存在 branches/nodes 时后者完全不执行；编辑器必须寻址实际数组并保留被遮蔽的原字段，不能写入无效 nodes 假装修改了分支。
需要一个分支内顺序执行时，直接子节点应为 sequence 容器。
三种策略均先等待全部直接子节点完成，再判定 all/any；first_success 当前与 any_success 同样检查任一成功。
界面必须保留这条真实语义，不能让连接两个 parallel 子节点看起来会强制先后执行。
依据：[任务提交及等待](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/parallel_node.cpp:329)、
[成功判定](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/parallel_node.cpp:604)。

`ConfigValidator::validate` 中虽然写有全局 ID 唯一、容器必须无结构环、sequence/loop 单后继约束，
但源码中没有对它的调用。WebService 采用独立 validateConfig；普通执行器通过 NodeFactory 直接加载。
`TaskGraphExpander::validate` 又是另一套校验，仅顶层有 subtask 时进入对应展开路径。
因此不能直接复制任何一套旧校验器并声称与执行语义一致。
编辑器应检查 JSON 形状、同层身份、局部端点、可判定的参数类型，并用真实执行流程区分条件标签、条件入口和循环。
依据：[WebService 校验](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/web_service/task_graph_web_service.cpp:407)、
[普通/展开路径选择](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/task_graph_executor.cpp:163)。

## 重命名、删除和引用

重命名当前图的节点时，在同一事务中更新：

1. 目标节点顶层 id。
2. 当前图所有 edges 的 from/to 中与旧 ID 完全相等的值。
3. 当前图容器已有的 entry_point（根为 config.entry_point）。内联 sequence/loop 不读取此字段，但如果原文已有结构引用也应保持一致。
4. 当前层 type=condition 节点顶层 if_true/if_false 中完全匹配的值。这是旧式条件叶跳转格式，不在 inputs 内。

不能下钻兄弟或子图做同名替换；不能把参数字符串、context 变量名、outputs 变量名、日志 node_name 或外部 source 当成结构引用。
占位符解析查 ContextManager，不查节点：支持的通用形式为 `{var}`、`{a.b.c}`、`{arr[idx]}`、`{arr[0]}`。
其中两个 extract 节点仍只支持简单变量替换，编辑器应保存原字符串而非推断或规范化表达式。
依据：[PlaceholderResolver](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/placeholder_resolver.cpp:291)、
[顶层 if_true/if_false](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/task_graph_executor.cpp:1331)。

**condition 容器入口目标改名可能改变同优先级分支顺序。** 改名前后比较入口排序；
发生变化时必须保持原条件检查顺序或明确报告，不能只改 to 然后静默改变优先匹配分支。
可用方案：为全部非 default 入口边按旧排序分配从 N 到 1 的不同 int32 priority，
作为同一改名事务记录并可撤销，default 边原值不动；N 超范围拒绝。
若原排序已因重复 to+priority 而不确定，应先报告这一歧义，不能宣称恢复了原本不存在的稳定顺序。
已有非法 priority 也不能强行参与数值排序，应要求修正其具体位置。
多个 default 分支也按 to 字典序排列，priority 无法改变它们的先后；改名若改变其排序必须拒绝并提示先整理默认分支。

删除节点同时删除指向它的边，并处理当前图 entry_point 与同层 condition 的 if_true/if_false 引用。
if_true/if_false 可缺省，缺省会回退静态边；应在删除摘要中列出被移除的结构引用并纳入撤销。
删除入口不应擅自换一个真实节点：去掉失效入口引用后可作为编辑中的草稿，保存前检查仍有有效入口。
已损坏的其他层应保留，不能因局部修改而丢弃原始内容。

## checkpoint 的实际边界

顶层 checkpoint 布尔主要由展开结果和 WebService 展示携带，普通 executeNode 没有按该字段自动保存。
SubTaskNode 接受布尔或对象形式，读取了 enabled，但本文件未见基于该成员自动保存的执行调用。
LoopNode 读取 checkpoint_per_iteration，但保存代码明确还是 TODO。
编辑器可以完整编辑和保留这些字段，不能把开关文案描述为“保证每次自动保存”。
依据：[执行节点](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/task_graph_executor.cpp:1180)、
[子任务字段](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/subtask_node.cpp:53)、
[循环 TODO](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/src/task_graph/nodes/loop_node.cpp:203)。

## 可用夹具与建议

| 源文件 | 只读统计与用途 |
| --- | --- |
| [task_parallel_full_body_demo.json](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/task_graphs/task_parallel_full_body_demo.json) | 53 节点、66 边；parallel 包含直接 sequence 分支，分支内含 loop；无同层重复和结构环。适合提取两分支，把真实叶动作替换为 log/delay，保留必要拓扑 |
| [task_box_transfer_split.json](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/task_box_transfer_split.json) | 451 节点；23 对 true/false label 边，并有 condition 容器；无同层重复。适合抽取一个条件叶和一组条件容器，去掉位姿/HTTP 地址/业务标识 |
| [task_leak_fast_mock_7803.json](/Users/loosqk/repo/cpp/ros2/master_control/src/master_control/task_graphs/task_leak_fast_mock_7803.json) | 615 节点；无同层重复但有跨层同名，main_loop 和 loop_final_start 有结构回边。适合外部文件只读分析与规模验证，不把整份生产文件复制进编辑器仓库 |

测试夹具必须区分“保留原样读取”的真实外部文档与“从协议构造”的脱敏小图。
脱敏小图要保留节点顺序、label/condition/priority、同层引用和内联形式，不能靠删去分支或循环获得绿灯。
以上结果是源码和静态 JSON 证据，不是机器人执行验收。
