//! 基于机器人执行器 af05e31 注册表与实际构造器的节点目录。
//!
//! 这里只检查修改节点的静态参数，既不执行表达式，也不探测远程文件或设备。
//! 未知字段和未知类型仍可保留；调用方应对未知类型显示“尚未核验”。
//! 协议证据、动态边界和模板说明见 docs/graph-executor-protocol.md。

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeDefinition {
    pub type_id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub container: bool,
}

macro_rules! definitions {
    ($(($id:literal, $name:literal, $description:literal, $container:literal)),+ $(,)?) => {
        const NODE_TYPES: &[NodeDefinition] = &[
            $(NodeDefinition { type_id: $id, name: $name, description: $description, container: $container }),+
        ];
    };
}

definitions![
    (
        "condition",
        "条件判断",
        "叶节点读取 inputs.condition；含 nodes/edges 时按入口边条件选择分支。",
        true
    ),
    (
        "ros2_action",
        "ROS2 动作",
        "需填写已注册 action_type 及对应位姿或轨迹；依赖机器人运行环境。",
        false
    ),
    (
        "loop",
        "循环",
        "按 inputs.condition 重复执行内联子图；执行器没有 max_iterations 限制。",
        true
    ),
    (
        "sequence",
        "顺序执行",
        "从无 label 的 _entry 边开始；允许条件标签和回边。",
        true
    ),
    (
        "parallel",
        "并行执行",
        "各直接子节点并行，edges 不参与调度；分支内顺序请使用 sequence。",
        true
    ),
    (
        "preplan_navigation",
        "导航预规划",
        "需填写真实位姿或 context 引用，依赖 ROS2 导航环境。",
        false
    ),
    (
        "multi_pose_generator",
        "生成多个位姿",
        "需填写物体类型、位姿、层高和方向等业务几何参数。",
        false
    ),
    (
        "behavior_tree",
        "行为树",
        "需填写机器人端树文件路径与黑板参数，文件在执行时读取。",
        false
    ),
    (
        "pop_array",
        "弹出数组元素",
        "需填写数组变量与 outputs.element；默认更新原数组。",
        false
    ),
    (
        "peek_array",
        "读取数组元素",
        "需填写数组变量与 outputs.element；index_var 存在时优先。",
        false
    ),
    (
        "get_array_length",
        "数组长度",
        "需填写 inputs.variable 与 outputs.length。",
        false
    ),
    (
        "array_index",
        "按下标取值",
        "接收原生数组、数组字符串或动态表达式，结果写入 outputs.result。",
        false
    ),
    (
        "object_get",
        "读取对象属性",
        "需填写对象、键及 outputs.result；表达式在机器人端求值。",
        false
    ),
    (
        "concat_arrays",
        "合并数组",
        "inputs.arrays 为数组集合，结果写入 outputs.result。",
        false
    ),
    (
        "box_task_notify",
        "通知箱任务",
        "需填写服务地址；submit 需要 event_type，wait_existing 需要 task_id。",
        false
    ),
    (
        "box_task_acquire",
        "领取箱任务",
        "需填写任务服务地址；执行会与业务服务交互。",
        false
    ),
    (
        "box_task_finish",
        "完成箱任务",
        "需填写服务地址与 request_id；执行会更新业务任务状态。",
        false
    ),
    (
        "area_guard",
        "区域互锁",
        "需填写服务地址、mode 和区域；执行影响机器人区域占用。",
        false
    ),
    (
        "persist_chassis_pose",
        "保存底盘位姿",
        "需填写 context_key；启用任务写回时还需填写对应任务 ID。",
        false
    ),
    (
        "slot_pose_library",
        "工位位姿库",
        "需填写取放观察位姿、方向、尺寸、层高等业务参数。",
        false
    ),
    (
        "slot_flow_scheduler",
        "工位流程调度",
        "需填写调度 action，依赖真实工位状态；部分动作还需要 slot_id。",
        false
    ),
    (
        "set_variable",
        "设置变量",
        "需填写变量名，以及非 null 的 value 或非空 value_from。",
        false
    ),
    (
        "copy_variable",
        "复制变量",
        "source 和 target 是 context 变量名，不是节点 ID。",
        false
    ),
    (
        "increment",
        "变量递增",
        "需填写变量名；step 与 initial_value 使用 32 位整数。",
        false
    ),
    (
        "log",
        "记录日志",
        "message 支持字符串；空消息模板不执行机器人动作。",
        false
    ),
    (
        "delay",
        "延时",
        "duration_ms 必须为数值，不能填写表达式字符串。",
        false
    ),
    (
        "wait_condition",
        "等待条件",
        "条件在执行时求值；轮询间隔和超时支持数值表达式。",
        false
    ),
    (
        "compare",
        "比较",
        "需填写左右值、比较运算符与 outputs.result。",
        false
    ),
    (
        "is_empty",
        "判断为空",
        "需填写 inputs.variable，实际输出键为 outputs.result。",
        false
    ),
    (
        "compute",
        "数值计算",
        "支持 +、-、*、/、%；表达式求值与除零由运行环境判定。",
        false
    ),
    (
        "mock_action",
        "模拟动作",
        "仅模拟执行；实际参数名为 success 和 result_value。",
        false
    ),
    (
        "print_variable",
        "打印变量",
        "需提供非空变量名数组；支持 compact 或 pretty 格式。",
        false
    ),
    (
        "extract_waist_height",
        "提取腰部高度",
        "需填写字符串位姿；当前执行器只替换简单的 {var} 引用。",
        false
    ),
    (
        "extract_pose_meta",
        "提取位姿元数据",
        "需填写字符串位姿；可输出 level 和 box_index。",
        false
    ),
    (
        "max_value",
        "取最大值",
        "left 与 right 必须为字符串数值或表达式。",
        false
    ),
    (
        "set_waist_height",
        "设置腰部高度",
        "pose_string 和 height 都是字符串；需填写真实位姿及高度。",
        false
    ),
    (
        "publish_task_log",
        "发布任务日志",
        "无必填参数，但执行需要 ROS2 日志发布环境。",
        false
    ),
    (
        "user_confirmation",
        "请求操作员确认",
        "缺省显示确认/取消，执行需要操作员交互环境。",
        false
    ),
    (
        "pause_task",
        "暂停任务",
        "执行会暂停任务并等待操作员继续。",
        false
    ),
    (
        "box_task_inventory",
        "箱任务盘点",
        "需填写服务地址及业务盘点参数，执行会更新业务状态。",
        false
    ),
    (
        "leak_test",
        "启动泄漏测试",
        "执行触发设备测试；action_name 缺省为 leak_test_start。",
        false
    ),
    (
        "get_leak_status",
        "读取泄漏测试状态",
        "依赖实际设备测试状态，可配置超时和轮询间隔。",
        false
    ),
    (
        "leak_test_prepare",
        "准备泄漏测试",
        "执行访问设备服务；缺省 command=1 会触发急停复位。",
        false
    ),
    (
        "subtask",
        "外部子任务",
        "source 指向外部任务文件，编辑器不会自动打开或改写该文件。",
        false
    ),
];

pub fn node_types() -> &'static [NodeDefinition] {
    NODE_TYPES
}

/// 生成结构草稿。空字符串/null 明确表示待填值，不猜测业务参数。
pub fn node_template(type_id: &str, id: &str) -> Option<Value> {
    if !NODE_TYPES
        .iter()
        .any(|definition| definition.type_id == type_id)
    {
        return None;
    }
    let mut node = json!({ "id": id, "type": type_id, "inputs": {} });
    let inputs = match type_id {
        "condition" => json!({"condition": "false"}),
        "ros2_action" => json!({"action_type": ""}),
        "loop" => json!({"condition": "false", "iteration_var": "i"}),
        "sequence" => json!({}),
        "parallel" => json!({"policy": "all_success"}),
        "preplan_navigation" | "extract_waist_height" | "extract_pose_meta" => {
            json!({"pose_string": ""})
        }
        "multi_pose_generator" => {
            json!({"object_id": "", "pose_string": "", "levels": "", "heights": "", "right_direction": "", "lean_forward_angle": ""})
        }
        "behavior_tree" => json!({"tree_file": ""}),
        "pop_array" | "peek_array" => json!({"array_var": ""}),
        "get_array_length" | "is_empty" | "increment" => json!({"variable": ""}),
        "array_index" => json!({"array": null, "index": null}),
        "object_get" => json!({"object": null, "key": null}),
        "concat_arrays" => json!({"arrays": null}),
        "box_task_notify" => json!({"server_url": "", "operation": "submit", "event_type": ""}),
        "box_task_acquire" | "box_task_inventory" => json!({"server_url": ""}),
        "box_task_finish" => json!({"server_url": "", "request_id": ""}),
        "area_guard" => json!({"server_url": "", "mode": "", "area": ""}),
        "persist_chassis_pose" => json!({"context_key": ""}),
        "slot_pose_library" => {
            json!({"get_observe": "", "put_observe": "", "right_direction": "", "boxes_length": null, "levels": null, "heights": null, "lean_forward_angle": null})
        }
        "slot_flow_scheduler" => json!({"action": ""}),
        "set_variable" => json!({"variable": "", "value": null}),
        "copy_variable" => json!({"source": "", "target": ""}),
        "log" => json!({"message": "", "level": "info"}),
        "delay" => json!({"duration_ms": 0}),
        "wait_condition" => json!({"condition": "true"}),
        "compare" | "compute" => json!({"left": null, "operator": "", "right": null}),
        "mock_action" => json!({"action_name": "mock", "duration_ms": 0, "success": true}),
        "print_variable" => json!({"variables": [""]}),
        "max_value" => json!({"left": "", "right": ""}),
        "set_waist_height" => json!({"pose_string": "", "height": ""}),
        // 这些类型确实没有必填输入，目录描述明确说明其真实执行效果及环境依赖。
        "publish_task_log" | "user_confirmation" | "pause_task" | "leak_test"
        | "get_leak_status" | "leak_test_prepare" | "subtask" => json!({}),
        _ => return None,
    };
    node["inputs"] = inputs;
    match type_id {
        "sequence" | "loop" => {
            node["nodes"] = json!([]);
            node["edges"] = json!([{"from": "_entry", "to": "_exit"}]);
        }
        "parallel" => {
            node["nodes"] = json!([]);
            node["edges"] = json!([]);
        }
        "pop_array" | "peek_array" => node["outputs"] = json!({"element": ""}),
        "get_array_length" => node["outputs"] = json!({"length": ""}),
        "array_index" | "object_get" | "concat_arrays" | "compare" | "is_empty" | "compute" => {
            node["outputs"] = json!({"result": ""});
        }
        "subtask" => node["source"] = json!(""),
        _ => {}
    }
    Some(node)
}

#[derive(Clone, Copy)]
enum Kind {
    String,
    NonemptyString,
    Bool,
    Int32,
    Number,
    NumberOrExpression,
    AnyValue,
    Array,
    ArrayOrString,
    ObjectOrString,
    Strings,
    NonemptyStrings,
    Object,
}

struct Fields<'a> {
    value: &'a Value,
    path: &'a str,
}

impl<'a> Fields<'a> {
    fn required(&self, key: &str, kind: Kind) -> Result<(), String> {
        match self.value.get(key) {
            Some(value) => check_kind(value, kind, &format!("{}.{}", self.path, key)),
            None => Err(format!("请填写 {}.{}", self.path, key)),
        }
    }

    fn optional(&self, key: &str, kind: Kind) -> Result<(), String> {
        match self.value.get(key) {
            Some(value) => check_kind(value, kind, &format!("{}.{}", self.path, key)),
            None => Ok(()),
        }
    }

    fn required_many(&self, keys: &[&str], kind: Kind) -> Result<(), String> {
        for key in keys {
            self.required(key, kind)?;
        }
        Ok(())
    }

    fn optional_many(&self, keys: &[&str], kind: Kind) -> Result<(), String> {
        for key in keys {
            self.optional(key, kind)?;
        }
        Ok(())
    }

    fn choice(
        &self,
        key: &str,
        values: &[&str],
        required: bool,
        dynamic: bool,
    ) -> Result<(), String> {
        match self.value.get(key) {
            Some(value) => {
                check_kind(
                    value,
                    Kind::NonemptyString,
                    &format!("{}.{}", self.path, key),
                )?;
                let text = value.as_str().unwrap_or_default();
                if values.contains(&text) || (dynamic && is_expression(text)) {
                    Ok(())
                } else {
                    Err(format!(
                        "{}.{} 应为 {}{}",
                        self.path,
                        key,
                        values.join(" / "),
                        if dynamic {
                            " 或 context 表达式"
                        } else {
                            ""
                        }
                    ))
                }
            }
            None if required => Err(format!("请填写 {}.{}", self.path, key)),
            None => Ok(()),
        }
    }

    fn number_range(&self, key: &str, minimum: f64, maximum: f64) -> Result<(), String> {
        match self.value.get(key).and_then(Value::as_f64) {
            Some(number) if number < minimum || number > maximum => Err(format!(
                "{}.{} 应在 {} 到 {} 之间",
                self.path, key, minimum, maximum
            )),
            _ => Ok(()),
        }
    }
}

fn is_expression(value: &str) -> bool {
    value.contains('{') && value.contains('}')
}

fn check_kind(value: &Value, kind: Kind, path: &str) -> Result<(), String> {
    let (valid, expected) = match kind {
        Kind::String => (value.is_string(), "字符串"),
        Kind::NonemptyString => (
            value.as_str().is_some_and(|text| !text.trim().is_empty()),
            "非空字符串",
        ),
        Kind::Bool => (value.is_boolean(), "布尔值"),
        Kind::Int32 => (
            value
                .as_i64()
                .is_some_and(|number| i32::try_from(number).is_ok()),
            "32 位整数",
        ),
        Kind::Number => (value.is_number(), "数值"),
        Kind::NumberOrExpression => (
            value.is_number() || value.as_str().is_some_and(|text| !text.trim().is_empty()),
            "数值或数值表达式字符串",
        ),
        Kind::AnyValue => (true, "JSON 值"),
        Kind::Array => (value.is_array(), "数组"),
        Kind::ArrayOrString => (
            value.is_array() || value.as_str().is_some_and(|text| !text.trim().is_empty()),
            "数组或非空数组字符串/表达式",
        ),
        Kind::ObjectOrString => (
            value.is_object() || value.as_str().is_some_and(|text| !text.trim().is_empty()),
            "对象或非空对象字符串/表达式",
        ),
        Kind::Strings => (
            value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            "字符串数组",
        ),
        Kind::NonemptyStrings => (
            value.as_array().is_some_and(|items| {
                !items.is_empty()
                    && items
                        .iter()
                        .all(|item| item.as_str().is_some_and(|text| !text.trim().is_empty()))
            }),
            "非空变量名数组",
        ),
        Kind::Object => (value.is_object(), "对象"),
    };
    if valid {
        Ok(())
    } else {
        Err(format!("{path} 应为{expected}，请填写有效值"))
    }
}

/// 只验证给定节点；图身份、引用、入口与递归范围由图命令事务负责。
pub fn validate_node(node: &Value) -> Result<(), String> {
    check_kind(node, Kind::Object, "节点")?;
    let top = Fields {
        value: node,
        path: "节点",
    };
    top.required_many(&["id", "type"], Kind::NonemptyString)?;
    let type_id = node["type"].as_str().unwrap_or_default();
    if !NODE_TYPES
        .iter()
        .any(|definition| definition.type_id == type_id)
    {
        return Ok(());
    }
    let empty = json!({});
    let (input_value, input_path) = match (node.get("inputs"), node.get("params"), type_id) {
        (Some(inputs), _, _) => (inputs, "inputs"),
        (None, _, "loop" | "condition") => (&empty, "inputs"),
        (None, Some(params), _) => (params, "params"),
        _ => (&empty, "inputs"),
    };
    check_kind(input_value, Kind::Object, input_path)?;
    let inputs = Fields {
        value: input_value,
        path: input_path,
    };
    let output_value = node.get("outputs").unwrap_or(&empty);
    check_kind(output_value, Kind::Object, "outputs")?;
    let outputs = Fields {
        value: output_value,
        path: "outputs",
    };
    // 输出映射会被 get<string>() 读取；保留名称，不能误当成节点引用。
    for (key, value) in output_value.as_object().into_iter().flatten() {
        check_kind(value, Kind::String, &format!("outputs.{key}"))?;
    }
    match type_id {
        "condition" if node.get("nodes").is_some() && node.get("edges").is_some() => {
            top.required_many(&["nodes", "edges"], Kind::Array)?;
        }
        "condition" => inputs.required("condition", Kind::NonemptyString)?,
        "sequence" => top.required_many(&["nodes", "edges"], Kind::Array)?,
        "loop" => {
            top.required_many(&["nodes", "edges"], Kind::Array)?;
            inputs.required("condition", Kind::NonemptyString)?;
            inputs.optional("iteration_var", Kind::String)?;
            inputs.optional("checkpoint_per_iteration", Kind::Bool)?;
        }
        "parallel" => {
            top.required(
                if node.get("branches").is_some() {
                    "branches"
                } else {
                    "nodes"
                },
                Kind::Array,
            )?;
            let fields = if inputs.value.get("policy").is_some() {
                &inputs
            } else {
                &top
            };
            fields.choice(
                "policy",
                &["all_success", "any_success", "first_success"],
                false,
                false,
            )?;
        }
        "ros2_action" => validate_action(&top, &inputs)?,
        "preplan_navigation" => {
            inputs.required("pose_string", Kind::NonemptyString)?;
            inputs.optional_many(
                &["cmd_code", "plan_type", "rotate_mode"],
                Kind::NumberOrExpression,
            )?;
            optional_bool_expression(&inputs, "ignore_failure")?;
        }
        "multi_pose_generator" => {
            inputs.required_many(
                &[
                    "object_id",
                    "pose_string",
                    "levels",
                    "heights",
                    "right_direction",
                ],
                Kind::NonemptyString,
            )?;
            inputs.optional_many(&["boxes_count", "boxes_length"], Kind::String)?;
            inputs.choice("object_id", &["0", "6"], true, true)?;
            validate_pose_offsets(&inputs, Kind::NonemptyString, true)?;
        }
        "behavior_tree" => {
            inputs.required("tree_file", Kind::NonemptyString)?;
            inputs.optional("tick_period_ms", Kind::Int32)?;
            inputs.optional("cache_tree", Kind::Bool)?;
            inputs.optional_many(&["blackboard_inputs", "blackboard_outputs"], Kind::Object)?;
        }
        "pop_array" | "peek_array" => {
            inputs.required("array_var", Kind::NonemptyString)?;
            outputs.required("element", Kind::NonemptyString)?;
            match type_id {
                "pop_array" => inputs.optional("update_array", Kind::Bool)?,
                _ => {
                    inputs.optional("index", Kind::Int32)?;
                    inputs.optional("index_var", Kind::String)?;
                }
            }
        }
        "get_array_length" | "is_empty" => {
            inputs.required("variable", Kind::NonemptyString)?;
            outputs.required(
                if type_id == "is_empty" {
                    "result"
                } else {
                    "length"
                },
                Kind::NonemptyString,
            )?;
        }
        "array_index" => {
            inputs.required("array", Kind::ArrayOrString)?;
            inputs.required("index", Kind::NumberOrExpression)?;
            outputs.required("result", Kind::NonemptyString)?;
        }
        "object_get" => {
            inputs.required("object", Kind::ObjectOrString)?;
            inputs.required("key", Kind::AnyValue)?;
            outputs.required("result", Kind::NonemptyString)?;
        }
        "concat_arrays" => {
            inputs.required("arrays", Kind::ArrayOrString)?;
            outputs.required("result", Kind::NonemptyString)?;
        }
        "box_task_notify" | "box_task_acquire" | "box_task_finish" | "area_guard"
        | "box_task_inventory" => {
            validate_box_service(type_id, &inputs)?;
        }
        "persist_chassis_pose" => {
            inputs.required("context_key", Kind::NonemptyString)?;
            for (flag, target) in [
                ("write_back_main_task", "main_task_id"),
                ("write_back_calibration_task", "calibration_task_id"),
            ] {
                optional_bool_expression(&inputs, flag)?;
                inputs.optional(target, Kind::NonemptyString)?;
                if inputs.value.get(flag).and_then(Value::as_bool) == Some(true) {
                    inputs.required(target, Kind::NonemptyString)?;
                }
            }
            inputs.optional("max_pose_age_ms", Kind::NumberOrExpression)?;
            inputs.number_range("max_pose_age_ms", 0.0, i32::MAX as f64)?;
        }
        "slot_pose_library" => {
            inputs.required_many(&["get_observe", "put_observe"], Kind::ObjectOrString)?;
            inputs.required("boxes_length", Kind::NumberOrExpression)?;
            inputs.required_many(&["levels", "heights"], Kind::ArrayOrString)?;
            inputs.choice("right_direction", &["x+", "x-", "y+", "y-"], true, true)?;
            inputs.optional("pick_to_put_offset", Kind::NumberOrExpression)?;
            validate_pose_offsets(&inputs, Kind::ArrayOrString, false)?;
        }
        "slot_flow_scheduler" => {
            inputs.choice(
                "action",
                &[
                    "init",
                    "select",
                    "mark_pick",
                    "mark_put",
                    "mark_moving",
                    "box_done",
                    "select_final_put",
                ],
                true,
                true,
            )?;
            inputs.optional("state_var", Kind::String)?;
            inputs.optional_many(&["capacity", "slot_id"], Kind::NumberOrExpression)?;
            inputs.optional("initial_slot_states", Kind::ObjectOrString)?;
            if matches!(
                inputs.value.get("action").and_then(Value::as_str),
                Some("mark_moving" | "box_done")
            ) {
                inputs.required("slot_id", Kind::NumberOrExpression)?;
                inputs.number_range("slot_id", 1.0, 4.0)?;
            }
        }
        "set_variable" => {
            inputs.required("variable", Kind::NonemptyString)?;
            inputs.optional_many(&["value_from", "value_type"], Kind::String)?;
            let has_source = inputs
                .value
                .get("value_from")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            if !has_source && inputs.value.get("value").is_none_or(Value::is_null) {
                return Err(format!(
                    "请填写非 null 的 {}.value 或非空 {}.value_from",
                    inputs.path, inputs.path
                ));
            }
        }
        "copy_variable" => inputs.required_many(&["source", "target"], Kind::NonemptyString)?,
        "increment" => {
            inputs.required("variable", Kind::NonemptyString)?;
            inputs.optional_many(&["step", "initial_value"], Kind::Int32)?;
        }
        "log" => {
            inputs.required("message", Kind::String)?;
            inputs.optional("variables", Kind::Strings)?;
            inputs.choice("level", &["debug", "info", "warn", "error"], false, false)?;
        }
        "delay" => {
            inputs.required("duration_ms", Kind::Number)?;
            inputs.number_range("duration_ms", 0.0, i32::MAX as f64)?;
        }
        "wait_condition" => {
            inputs.required("condition", Kind::NonemptyString)?;
            inputs.optional("fail_condition", Kind::String)?;
            inputs.optional_many(
                &["poll_interval_ms", "timeout_ms"],
                Kind::NumberOrExpression,
            )?;
            inputs.number_range("poll_interval_ms", 1.0, i32::MAX as f64)?;
            inputs.number_range("timeout_ms", 0.0, i32::MAX as f64)?;
        }
        "compare" | "compute" => {
            let kind = if type_id == "compute" {
                Kind::NumberOrExpression
            } else {
                Kind::AnyValue
            };
            inputs.required_many(&["left", "right"], kind)?;
            let operators: &[&str] = if type_id == "compute" {
                &["+", "-", "*", "/", "%"]
            } else {
                &[
                    "==", "eq", "!=", "ne", "<", "lt", "<=", "le", ">", "gt", ">=", "ge",
                ]
            };
            inputs.choice("operator", operators, true, false)?;
            outputs.required("result", Kind::NonemptyString)?;
        }
        "mock_action" => {
            inputs.required("action_name", Kind::NonemptyString)?;
            inputs.optional("duration_ms", Kind::Number)?;
            inputs.number_range("duration_ms", 0.0, i32::MAX as f64)?;
            inputs.optional("success", Kind::Bool)?;
        }
        "print_variable" => {
            inputs.required("variables", Kind::NonemptyStrings)?;
            inputs.choice("level", &["debug", "info", "warn", "error"], false, false)?;
            inputs.choice("format", &["compact", "pretty"], false, false)?;
        }
        "extract_waist_height" | "extract_pose_meta" => {
            inputs.required("pose_string", Kind::NonemptyString)?
        }
        "max_value" => inputs.required_many(&["left", "right"], Kind::NonemptyString)?,
        "set_waist_height" => {
            inputs.required_many(&["pose_string", "height"], Kind::NonemptyString)?
        }
        "publish_task_log" => {
            inputs.optional_many(
                &["message", "log_level", "task_name", "task_id", "node_name"],
                Kind::String,
            )?;
            inputs.optional("progress", Kind::Number)?;
            for key in ["status", "result"] {
                inputs.optional(key, Kind::Int32)?;
                inputs.number_range(key, i8::MIN as f64, i8::MAX as f64)?;
            }
        }
        "user_confirmation" => {
            inputs.optional_many(&["title", "message"], Kind::String)?;
            inputs.optional("options", Kind::Strings)?;
            inputs.optional("timeout_seconds", Kind::Int32)?;
        }
        "pause_task" => inputs.optional_many(&["message", "level"], Kind::String)?,
        "leak_test" => inputs.optional("action_name", Kind::String)?,
        "get_leak_status" => {
            inputs.optional("action_name", Kind::String)?;
            inputs.optional("timeout_sec", Kind::Number)?;
            inputs.optional("poll_interval_ms", Kind::Int32)?;
        }
        "leak_test_prepare" => {
            inputs.optional("service_name", Kind::String)?;
            inputs.optional("command", Kind::NumberOrExpression)?;
            inputs.optional("timeout_sec", Kind::Number)?;
            inputs.optional("wait_result", Kind::Bool)?;
        }
        "subtask" => {
            top.required("source", Kind::NonemptyString)?;
            if let Some(checkpoint) = node.get("checkpoint") {
                match checkpoint {
                    Value::Bool(_) => {}
                    Value::Object(fields) => {
                        if let Some(enabled) = fields.get("enabled") {
                            check_kind(enabled, Kind::Bool, "节点.checkpoint.enabled")?;
                        }
                    }
                    _ => return Err("节点.checkpoint 应为布尔值或含 enabled 的对象".into()),
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn optional_bool_expression(inputs: &Fields<'_>, key: &str) -> Result<(), String> {
    match inputs.value.get(key) {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(()),
        _ => inputs.optional(key, Kind::Bool),
    }
}

fn validate_pose_offsets(
    inputs: &Fields<'_>,
    lean_kind: Kind,
    invalid_falls_back: bool,
) -> Result<(), String> {
    let Some(value) = inputs.value.get("pose_offset_configs") else {
        return inputs.required("lean_forward_angle", lean_kind);
    };
    // 动态偏移由执行器求值，不能拿本机缺少 context 当作字段错误。
    if value.as_str().is_some_and(is_expression) {
        return Ok(());
    }
    if value.as_str().is_some_and(str::is_empty) {
        return inputs.required("lean_forward_angle", lean_kind);
    }
    let parsed;
    let offsets = match value.as_str() {
        Some(text) => {
            parsed = serde_json::from_str::<Value>(text).ok();
            parsed.as_ref().unwrap_or(&Value::Null)
        }
        None => value,
    };
    let valid = offsets.as_array().is_some_and(|items| {
        !items.is_empty()
            && items.iter().all(|item| {
                item.as_array()
                    .is_some_and(|row| row.len() == 3 && row.iter().all(Value::is_number))
            })
    });
    match (valid, invalid_falls_back) {
        (true, _) => Ok(()),
        (false, true) => inputs.required("lean_forward_angle", lean_kind),
        (false, false) => Err(format!(
            "{}.pose_offset_configs 应为非空的三列数值数组，或对应 JSON 字符串/表达式",
            inputs.path
        )),
    }
}

fn validate_action(top: &Fields<'_>, inputs: &Fields<'_>) -> Result<(), String> {
    let action_fields = if top.value.get("action_type").is_some() {
        top
    } else {
        inputs
    };
    action_fields.choice(
        "action_type",
        &[
            "navigation",
            "navigation_always_fail",
            "navigation_retry_success",
            "waist_control",
            "head_control",
            "double_arm",
            "joint_trajectory",
        ],
        true,
        false,
    )?;
    inputs.optional("timeout_sec", Kind::NumberOrExpression)?;
    match action_fields.value["action_type"]
        .as_str()
        .unwrap_or_default()
    {
        "navigation"
        | "navigation_always_fail"
        | "navigation_retry_success"
        | "waist_control"
        | "head_control" => {
            inputs.required(
                if inputs.value.get("pose_string").is_some() {
                    "pose_string"
                } else {
                    "target_pose"
                },
                Kind::NonemptyString,
            )?;
            inputs.optional("runtime", Kind::NumberOrExpression)?;
        }
        "double_arm" => {
            inputs.optional("action_type_code", Kind::NumberOrExpression)?;
            inputs.optional_many(
                &[
                    "target_poses",
                    "obj_info",
                    "reserved_para_names",
                    "reserved_paras",
                ],
                Kind::String,
            )?;
        }
        "joint_trajectory" => {
            inputs.required("trajectory_points", Kind::ArrayOrString)?;
            inputs.optional("joint_names", Kind::ArrayOrString)?;
            inputs.optional("arm", Kind::String)?;
            inputs.optional("goal_time_tolerance", Kind::NumberOrExpression)?;
            if let Some(points) = inputs.value["trajectory_points"].as_array() {
                if points.is_empty() {
                    return Err(format!(
                        "{}.trajectory_points 至少需要一个轨迹点",
                        inputs.path
                    ));
                }
                for (index, point) in points.iter().enumerate() {
                    let path = format!("{}.trajectory_points[{index}]", inputs.path);
                    check_kind(point, Kind::Object, &path)?;
                    let fields = Fields {
                        value: point,
                        path: &path,
                    };
                    fields.required("positions", Kind::Array)?;
                    fields.required("time_from_start", Kind::Number)?;
                    for (joint, position) in point["positions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                    {
                        check_kind(
                            position,
                            Kind::Number,
                            &format!("{path}.positions[{joint}]"),
                        )?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_box_service(type_id: &str, inputs: &Fields<'_>) -> Result<(), String> {
    inputs.required("server_url", Kind::NonemptyString)?;
    inputs.optional_many(
        &["timeout_ms", "poll_interval_ms"],
        Kind::NumberOrExpression,
    )?;
    match type_id {
        "box_task_notify" => {
            inputs.choice("operation", &["submit", "wait_existing"], false, true)?;
            match inputs
                .value
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("submit")
            {
                "submit" => inputs.required("event_type", Kind::NonemptyString)?,
                "wait_existing" => inputs.required("task_id", Kind::NonemptyString)?,
                _ => {}
            }
        }
        "box_task_finish" => {
            inputs.required("request_id", Kind::NonemptyString)?;
            inputs.optional_many(&["state", "message"], Kind::String)?;
        }
        "area_guard" => {
            inputs.choice(
                "mode",
                &["acquire", "release", "manual_release", "status"],
                true,
                true,
            )?;
            inputs.required("area", Kind::NonemptyString)?;
            optional_bool_expression(inputs, "enabled")?;
        }
        "box_task_inventory" => {
            inputs.choice(
                "mode",
                &["reset", "recover", "reset_station_batch"],
                false,
                true,
            )?;
            for key in [
                "retry_count",
                "retry_initial_delay_ms",
                "retry_max_delay_ms",
            ] {
                inputs.optional(key, Kind::NumberOrExpression)?;
                inputs.number_range(key, 0.0, i32::MAX as f64)?;
            }
            let initial = inputs
                .value
                .get("retry_initial_delay_ms")
                .map_or(Some(1000.0), Value::as_f64);
            let maximum = inputs
                .value
                .get("retry_max_delay_ms")
                .map_or(Some(5000.0), Value::as_f64);
            if matches!((initial, maximum), (Some(initial), Some(maximum)) if maximum < initial) {
                return Err(format!(
                    "{}.retry_max_delay_ms 不能小于 retry_initial_delay_ms",
                    inputs.path
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn 实际注册目录覆盖四十四类型且模板保留指定身份() {
        let ids: HashSet<_> = node_types()
            .iter()
            .map(|definition| definition.type_id)
            .collect();
        assert_eq!(ids.len(), 44);
        assert_eq!(node_types().len(), 44);
        for definition in node_types() {
            let node = node_template(definition.type_id, "example").unwrap();
            assert_eq!(node["id"], "example");
            assert_eq!(node["type"], definition.type_id);
        }
        assert!(node_template("unknown_business", "example").is_none());
    }

    #[test]
    fn 八种基础模板满足实际静态约束() {
        for type_id in [
            "condition",
            "loop",
            "sequence",
            "parallel",
            "log",
            "delay",
            "wait_condition",
            "mock_action",
        ] {
            let node = node_template(type_id, "new_node").unwrap();
            assert!(
                validate_node(&node).is_ok(),
                "{type_id}: {:?}",
                validate_node(&node)
            );
        }
    }

    #[test]
    fn 业务模板明确阻止空字段且错误定位字段() {
        for (type_id, field) in [
            ("ros2_action", "action_type"),
            ("pop_array", "array_var"),
            ("set_variable", "variable"),
            ("box_task_finish", "server_url"),
            ("subtask", "source"),
            ("multi_pose_generator", "object_id"),
        ] {
            let node = node_template(type_id, "new_node").unwrap();
            assert!(validate_node(&node).unwrap_err().contains(field));
        }
    }

    #[test]
    fn 旧参数回退保留但inputs存在时优先且loop不回退() {
        let mut node = json!({"id":"n", "type":"log", "params":{"message":"保留"}});
        assert!(validate_node(&node).is_ok());
        node["inputs"] = json!({});
        assert!(validate_node(&node).unwrap_err().contains("inputs.message"));
        let node = json!({"id":"n", "type":"loop", "params":{"condition":"false"}, "nodes":[], "edges":[]});
        assert!(
            validate_node(&node)
                .unwrap_err()
                .contains("inputs.condition")
        );
    }

    #[test]
    fn 动态数值支持与直接数值字段区别验证() {
        let node = json!({"id":"n", "type":"wait_condition", "inputs":{"condition":"{ready}", "poll_interval_ms":"{settings.poll_ms}"}});
        assert!(validate_node(&node).is_ok());
        let node =
            json!({"id":"n", "type":"delay", "inputs":{"duration_ms":"{settings.delay_ms}"}});
        assert!(validate_node(&node).unwrap_err().contains("duration_ms"));
        let node = json!({"id":"n", "type":"wait_condition", "inputs":{"condition":"true", "poll_interval_ms":0.9}});
        assert!(validate_node(&node).is_err());
    }

    #[test]
    fn 真实输出名与整数边界得到检查() {
        let mut node = json!({"id":"n", "type":"is_empty", "inputs":{"variable":"items"}, "outputs":{"is_empty":"result"}});
        assert!(validate_node(&node).unwrap_err().contains("outputs.result"));
        node["outputs"]["result"] = json!("empty");
        assert!(validate_node(&node).is_ok());
        let node = json!({"id":"n", "type":"increment", "inputs":{"variable":"count", "step":2147483648_u64}});
        assert!(validate_node(&node).is_err());
        let node = json!({"id":"n", "type":"publish_task_log", "inputs":{"status":128}});
        assert!(validate_node(&node).is_err());
    }

    #[test]
    fn 条件容器不要求叶条件且parallel优先branches() {
        let node = json!({"id":"n", "type":"condition", "nodes":[], "edges":[{"from":"_entry", "to":"_exit"}]});
        assert!(validate_node(&node).is_ok());
        let node = json!({"id":"n", "type":"parallel", "branches":[], "nodes":null});
        assert!(validate_node(&node).is_ok());
    }

    #[test]
    fn 未知节点与扩展字段原样保留且校验不修改输入() {
        let node =
            json!({"id":"n", "type":"unknown_business", "inputs":[1], "vendor":{"node_id":"n"}});
        let before = node.clone();
        assert!(validate_node(&node).is_ok());
        assert_eq!(node, before);
    }

    #[test]
    fn 只有启用对应写回才要求任务身份() {
        let mut node =
            json!({"id":"n", "type":"persist_chassis_pose", "inputs":{"context_key":"pose"}});
        assert!(validate_node(&node).is_ok());
        node["inputs"]["write_back_main_task"] = json!(true);
        assert!(validate_node(&node).unwrap_err().contains("main_task_id"));
        node["inputs"]["main_task_id"] = json!("{tasks.main}");
        assert!(validate_node(&node).is_ok());
    }

    #[test]
    fn 动作适配器分别校验位姿和轨迹而不伪造机器人数据() {
        let mut node =
            json!({"id":"n", "type":"ros2_action", "inputs":{"action_type":"navigation"}});
        assert!(validate_node(&node).unwrap_err().contains("target_pose"));
        node["inputs"]["pose_string"] = json!("{poses.pick}");
        assert!(validate_node(&node).is_ok());
        node["action_type"] = json!("joint_trajectory");
        node["inputs"]["trajectory_points"] = json!([]);
        assert!(validate_node(&node).is_err());
        node["inputs"]["trajectory_points"] = json!("{trajectory}");
        assert!(validate_node(&node).is_ok());
    }

    #[test]
    fn 合并数组允许动态集合且比较允许空值字面量() {
        let node = json!({"id":"n", "type":"concat_arrays", "inputs":{"arrays":"{array_groups}"}, "outputs":{"result":"all"}});
        assert!(validate_node(&node).is_ok());
        for literal in [Value::Null, json!("")] {
            let node = json!({"id":"n", "type":"compare", "inputs":{"left":literal, "operator":"==", "right":literal}, "outputs":{"result":"same"}});
            assert!(validate_node(&node).is_ok());
        }
    }
}
