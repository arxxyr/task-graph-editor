//! 任务图编辑器：元数据 + 按类型分组的 context 字段树
//!
//! 字段树按类型分成六组（位姿点位、基本参数、数组参数、轨迹数据、嵌套分组、其他），
//! 嵌套分组内部递归复用同一套分组结构。
//!
//! 重建策略：
//! - 只有"结构变化"（加载文件、把 null 变成位姿）才重建控件树，
//!   纯数值编辑不重建，否则每敲一个字符输入焦点就没了；
//! - 位姿选中高亮、ROS2 回填的新数值都只更新已有控件，不重建；
//! - 轨迹点、二维数组行的内容懒加载——展开时才建，避免大轨迹一次生成上千个输入框。

use bevy::feathers::controls::{
    ButtonVariant, FeathersNumberInput, NumberFormat, NumberInputValue, UpdateNumberInput,
};
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemeToken};
use bevy::feathers::tokens;
use bevy::prelude::*;
use bevy::text::{EditableText, LineBreak, TextEditChange};
use bevy::ui::Checked;
use bevy::ui_widgets::ValueChange;
use std::collections::HashSet;

use crate::model::{ContextField, ContextValue, RobotPose, TaskGraphData, TrajectoryPoint};

use super::binding::{
    BoolBinding, PoseComp, PosePart, PoseTarget, TextBinding, ValueBinding, ValueSlot, apply_bool,
    apply_f64, apply_i64, apply_text, read_f64,
};
use super::connect::ActionButton;
use super::shell::{ActionBarSlot, EditorSlot};
use super::theme;
use super::widgets::{
    self, BoxedScene, ButtonGate, Collapsible, NumberFieldBorder, NumberFieldInit, NumberInitValue,
    boxed,
};
use super::worker_bridge::AppAction;
use super::{Editor, Session, StatusLine, UiSet};

/// 当前文档尚未修复的数值输入错误，保存前必须检查。
///
/// 无效文本不写回模型，但仍保留在输入框中供用户修正；不能静默保存旧值。
#[derive(Resource, Default)]
pub struct InputValidation {
    structure_version: u64,
    invalid_fields: HashSet<Entity>,
    /// 同一非法字段继续输入也递增，不能只比较“是否有错误”。
    invalid_revision: u64,
}

impl InputValidation {
    pub(super) fn invalid_revision(&self) -> u64 {
        self.invalid_revision
    }
    /// 是否仍有属于当前字段树的输入错误。
    pub fn has_errors(&self, editor: &Editor) -> bool {
        self.structure_version == editor.structure_version && !self.invalid_fields.is_empty()
    }

    fn field_is_invalid(&self, editor: &Editor, entity: Entity) -> bool {
        self.structure_version == editor.structure_version && self.invalid_fields.contains(&entity)
    }

    fn reset_for(&mut self, editor: &Editor) {
        if self.structure_version != editor.structure_version {
            self.structure_version = editor.structure_version;
            self.invalid_fields.clear();
        }
    }

    fn set_invalid(&mut self, editor: &Editor, entity: Entity, invalid: bool) -> bool {
        self.reset_for(editor);
        if invalid || self.invalid_fields.contains(&entity) {
            self.invalid_revision += 1;
        }
        match invalid {
            true => self.invalid_fields.insert(entity),
            false => self.invalid_fields.remove(&entity),
        }
    }
}

/// 外部删除或整棵字段树重建后，不让已经消失的控件继续阻止保存。
fn clear_stale_input_errors(
    editor: Res<Editor>,
    fields: Query<(), With<ValueBinding>>,
    mut validation: ResMut<InputValidation>,
) {
    validation.reset_for(&editor);
    validation
        .invalid_fields
        .retain(|entity| fields.contains(*entity));
}

/// Feathers 只负责解析并发事件，本应用补齐空值、语法和数值范围校验。
fn validate_number_text(text: &str, format: NumberFormat) -> Result<(), &'static str> {
    let text = text.trim();
    match format {
        NumberFormat::F64 => match text.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(()),
            _ => Err("请输入有效且有限的 64 位浮点数"),
        },
        NumberFormat::F32 => match text.parse::<f32>() {
            Ok(value) if value.is_finite() => Ok(()),
            _ => Err("请输入有效且有限的 32 位浮点数"),
        },
        NumberFormat::I64 => text
            .parse::<i64>()
            .map(|_| ())
            .map_err(|_| "请输入有效的 64 位整数"),
        NumberFormat::I32 => text
            .parse::<i32>()
            .map(|_| ())
            .map_err(|_| "请输入有效的 32 位整数"),
    }
}

/// 每次文本变化都校验，覆盖不会产生 ValueChange 的空串、半截指数和整数溢出。
fn on_number_text_edit(
    change: On<TextEditChange>,
    texts: Query<(&ChildOf, &EditableText)>,
    fields: Query<(&ValueBinding, &NumberFormat, &NumberFieldBorder)>,
    editor: Res<Editor>,
    mut validation: ResMut<InputValidation>,
    mut status: ResMut<StatusLine>,
    mut commands: Commands,
) {
    let Ok((parent, editable)) = texts.get(change.event_target()) else {
        return;
    };
    let entity = parent.parent();
    let Ok((binding, format, normal_border)) = fields.get(entity) else {
        return;
    };
    let result = validate_number_text(&editable.value().to_string(), *format);
    let changed = validation.set_invalid(&editor, entity, result.is_err());
    match result {
        Err(reason) => {
            let field_name = editor
                .data
                .as_ref()
                .and_then(|data| {
                    crate::model::field_at_path(&data.context_fields, &binding.field_path)
                })
                .map_or("数值字段", |field| field.key.as_str());
            status.set(format!(
                "输入错误：{field_name}，{reason}；修正标红数值后才能保存"
            ));
            commands
                .entity(entity)
                .insert(ThemeBorderColor(theme::STATUS_ERROR));
        }
        Ok(()) if changed => {
            commands
                .entity(entity)
                .insert(ThemeBorderColor(normal_border.0.clone()));
            match validation.has_errors(&editor) {
                true => status.set("输入错误：仍有数值需要修正，请检查标红的输入框"),
                false => status.set("数值输入已修正，可保存"),
            }
        }
        Ok(()) => {}
    }
}

/// 元数据字段
#[derive(Component, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum MetaField {
    /// 地图 ID
    #[default]
    MapId,
    /// 任务 ID
    TaskId,
}

/// 当前编辑文档的远程完整来源，独立于左侧文件列表目录。
#[derive(Component, Clone, Default)]
struct DocumentSourceLabel;

fn document_source_label(editor: &Editor) -> String {
    match &editor.document {
        Some(document) => format!(
            "保存位置：{}/{}",
            document.remote_dir.trim_end_matches('/'),
            document.filename,
        ),
        None => "保存位置：尚未绑定远程文件".into(),
    }
}

fn sync_document_source(
    editor: Res<Editor>,
    mut labels: Query<(&mut Text, Ref<DocumentSourceLabel>)>,
) {
    for (mut text, marker) in &mut labels {
        if editor.is_changed() || marker.is_added() {
            let label = document_source_label(&editor);
            if text.0 != label {
                text.0 = label;
            }
        }
    }
}

/// 位姿卡片，记录独立位姿或数组元素的完整目标（用于选中）
#[derive(Component, Clone, Default)]
struct PoseCard {
    target: PoseTarget,
}

/// 位姿卡片的标题文字（选中时变色）
#[derive(Component, Clone, Default)]
struct PoseCardTitle {
    target: PoseTarget,
}

/// 懒加载内容的类型
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum LazyKind {
    /// 无懒加载内容
    #[default]
    None,
    /// 轨迹中第 n 个点的时间与关节值
    TrajPoint(usize),
    /// 二维数组第 n 行的各列
    Array2DRow(usize),
}

/// 懒加载内容已填充的标记
///
/// `queue_spawn_related_scenes` 排队后要等场景解析完才挂上 `Children`，
/// 期间若只看 `Children` 是否为空，会在同一个区块上重复填充。
#[derive(Component, Clone, Default)]
struct LazyFilled;

/// 挂在折叠区块内容节点上：展开时按需构建内容
#[derive(Component, Clone, Default)]
struct LazyBody {
    /// 所属字段的索引路径
    path: Vec<usize>,
    /// 内容类型
    kind: LazyKind,
}

/// 已渲染的编辑器版本
#[derive(Resource, Default)]
struct RenderedEditor {
    /// 已渲染的结构版本；`None` 表示还没建过
    ///
    /// 不能拿 0 当"没建过"的哨兵：版本号从 0 起算，
    /// 第一次加载文件（0 → 1）会被误判成已渲染，编辑器就永远是空的。
    structure: Option<u64>,
    /// 已渲染的数值版本
    values: u64,
    /// 已渲染的操作栏状态（有数据、已连接）
    action_bar: Option<(bool, bool)>,
    /// 已渲染的选中位姿目标
    selection: Option<PoseTarget>,
}

/// 四元数分量的色条：w 用中性色，xyz 与位置三轴同色，便于横排时快速定位
fn quat_axis_token(comp: PoseComp) -> ThemeToken {
    match comp {
        PoseComp::OriX => tokens::TEXT_INPUT_X_AXIS,
        PoseComp::OriY => tokens::TEXT_INPUT_Y_AXIS,
        PoseComp::OriZ => tokens::TEXT_INPUT_Z_AXIS,
        _ => tokens::TEXT_INPUT_LABEL_BG,
    }
}

/// 位姿数值的轴色：位置 xyz 用 X/Y/Z 三色，姿态四元数不着色
fn axis_of(comp: PoseComp) -> Option<(ThemeToken, &'static str)> {
    match comp {
        PoseComp::PosX => Some((tokens::TEXT_INPUT_X_AXIS, "x")),
        PoseComp::PosY => Some((tokens::TEXT_INPUT_Y_AXIS, "y")),
        PoseComp::PosZ => Some((tokens::TEXT_INPUT_Z_AXIS, "z")),
        PoseComp::OriW | PoseComp::OriX | PoseComp::OriY | PoseComp::OriZ => None,
    }
}

// ============================================================
// 顶栏操作按钮
// ============================================================

/// 远程操作按钮组
///
/// 只按"有没有数据、连没连上"决定按钮存不存在；忙碌与选中位姿是属性，交给
/// [`ButtonGate`]，否则每次忙碌翻转都重建一次，会撞上场景排队落地的竞态。
fn action_bar(has_data: bool, connected: bool) -> Vec<BoxedScene> {
    if !has_data || !connected {
        return Vec::new();
    }

    let mut items: Vec<BoxedScene> = vec![boxed(widgets::button_gated(
        "应用到远程文件",
        ButtonVariant::Primary,
        ButtonGate::WhenIdle,
        ActionButton(AppAction::SaveToRemote),
    ))];

    // 三个 ROS2 取数按钮还要求先选中一个位姿点位
    let fetches = [
        ("获取底盘位姿", AppAction::FetchChassisPose),
        ("获取头部关节", AppAction::FetchHeadJoints),
        ("获取腰部关节", AppAction::FetchWaistJoints),
    ];
    for (label, action) in fetches {
        items.push(boxed(widgets::button_gated(
            label,
            ButtonVariant::Normal,
            ButtonGate::WhenIdleAndPose,
            ActionButton(action),
        )));
    }
    // 提示只在没选中位姿时露出，显隐由 sync_pose_hint 控制——
    // 选中与否是属性，塞进重建条件会让整条操作栏跟着重建
    items.push(boxed(bsn! {
        Node { display: {Display::None} }
        PoseHintText
        Children [(widgets::hint("← 位姿相关按钮需先在下方选中一个点位"))]
    }));
    items
}

/// 顶栏那句"需先选中位姿"的提示
#[derive(Component, Clone, Default)]
struct PoseHintText;

/// 选中位姿后收起提示，取消选中再露出来
fn sync_pose_hint(editor: Res<Editor>, mut hints: Query<&mut Node, With<PoseHintText>>) {
    if !editor.is_changed() {
        return;
    }
    let display = match editor.has_pose_selection() {
        true => Display::None,
        false => Display::Flex,
    };
    for mut node in &mut hints {
        if node.display != display {
            node.display = display;
        }
    }
}

// ============================================================
// 元数据
// ============================================================

/// 元数据卡片
fn metadata_card(data: &TaskGraphData, editor: &Editor) -> impl Scene {
    widgets::card(
        "元数据",
        bsn_list![
            (
                Text({document_source_label(editor)})
                DocumentSourceLabel
                ThemeTextColor({tokens::TEXT_DIM})
                TextFont { font_size: px(11.0) }
                TextLayout { linebreak: {LineBreak::AnyCharacter} }
                Node { width: percent(100), margin: {UiRect::bottom(px(5.0))} }
            ),
            widgets::form_row(
                "map_id",
                widgets::text_field(data.map_id.clone(), MetaField::MapId)
            ),
            widgets::form_row(
                "task_id",
                widgets::text_field(data.task_id.clone(), MetaField::TaskId)
            ),
            widgets::hint("修改 task_id 后，保存时远程文件会一并改名为 {task_id}.json")
        ],
    )
}

// ============================================================
// 字段分组（递归）
// ============================================================

/// 按类型把一层字段分成六组
fn field_groups(
    fields: &[ContextField],
    path_prefix: &[usize],
    selected: Option<&PoseTarget>,
) -> Vec<BoxedScene> {
    let mut poses = Vec::new();
    let mut scalars = Vec::new();
    let mut arrays = Vec::new();
    let mut trajectories = Vec::new();
    let mut nested = Vec::new();
    let mut others = Vec::new();

    for (index, field) in fields.iter().enumerate() {
        let bucket = match &field.value {
            ContextValue::Pose(_) => &mut poses,
            ContextValue::Bool(_) | ContextValue::Integer(_) | ContextValue::Float(_) => {
                &mut scalars
            }
            ContextValue::NumericArray { .. } | ContextValue::NumericArray2D { .. } => &mut arrays,
            ContextValue::JointTrajectory(_) => &mut trajectories,
            ContextValue::NestedGroup(_) => &mut nested,
            _ => &mut others,
        };
        bucket.push(index);
    }

    let mut out: Vec<BoxedScene> = Vec::new();

    if !poses.is_empty() {
        let body: Vec<BoxedScene> = poses
            .iter()
            .map(|&i| {
                let path = child_path(path_prefix, i);
                let is_selected = selected == Some(&PoseTarget::field(path.clone()));
                boxed(pose_card(&fields[i], path, is_selected))
            })
            .collect();
        out.push(boxed(widgets::collapsible(
            "位姿点位",
            Some(poses.len().to_string()),
            true,
            body,
        )));
    }

    if !scalars.is_empty() {
        let body: Vec<BoxedScene> = scalars
            .iter()
            .map(|&i| boxed(scalar_row(&fields[i], child_path(path_prefix, i))))
            .collect();
        out.push(boxed(widgets::collapsible(
            "基本参数",
            Some(scalars.len().to_string()),
            true,
            body,
        )));
    }

    if !arrays.is_empty() {
        let body: Vec<BoxedScene> = arrays
            .iter()
            .map(|&i| array_section(&fields[i], child_path(path_prefix, i)))
            .collect();
        out.push(boxed(widgets::collapsible(
            "数组参数",
            Some(arrays.len().to_string()),
            true,
            body,
        )));
    }

    if !trajectories.is_empty() {
        let body: Vec<BoxedScene> = trajectories
            .iter()
            .map(|&i| boxed(trajectory_section(&fields[i], child_path(path_prefix, i))))
            .collect();
        out.push(boxed(widgets::collapsible(
            "轨迹数据",
            Some(trajectories.len().to_string()),
            false,
            body,
        )));
    }

    if !nested.is_empty() {
        let body: Vec<BoxedScene> = nested
            .iter()
            .map(|&i| {
                boxed(nested_section(
                    &fields[i],
                    child_path(path_prefix, i),
                    selected,
                ))
            })
            .collect();
        out.push(boxed(widgets::collapsible(
            "嵌套分组",
            Some(nested.len().to_string()),
            true,
            body,
        )));
    }

    if !others.is_empty() {
        let body: Vec<BoxedScene> = others
            .iter()
            .map(|&i| other_field(&fields[i], child_path(path_prefix, i), selected))
            .collect();
        out.push(boxed(widgets::collapsible(
            "其他",
            Some(others.len().to_string()),
            true,
            body,
        )));
    }

    out
}

/// 拼接子路径
fn child_path(prefix: &[usize], index: usize) -> Vec<usize> {
    prefix.iter().copied().chain([index]).collect()
}

// ============================================================
// 位姿
// ============================================================

/// 位姿卡片：外层容器负责选中高亮，内部是以 key 为标题的折叠区块
fn pose_card(field: &ContextField, path: Vec<usize>, selected: bool) -> impl Scene {
    let ContextValue::Pose(pose) = &field.value else {
        unreachable!("调用方已按类型分组");
    };
    let parts: Vec<BoxedScene> = PosePart::ALL
        .iter()
        .map(|&part| boxed(pose_part(part, &path, pose)))
        .collect();

    selectable_pose_card(field.key.clone(), PoseTarget::field(path), selected, parts)
}

/// 独立位姿和数组元素共用选中外壳，折叠及数值控件保持原实体。
fn selectable_pose_card(
    title: String,
    target: PoseTarget,
    selected: bool,
    parts: Vec<BoxedScene>,
) -> impl Scene {
    let (bg, border) = pose_card_tokens(selected);
    let card_marker = PoseCard {
        target: target.clone(),
    };
    let title_marker = PoseCardTitle { target };

    bsn! {
        Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            padding: {UiRect::all(px(theme::PAD_SM))},
            border: {UiRect::all(px(1.0))},
            border_radius: {BorderRadius::all(px(theme::RADIUS_SM))},
            margin: {UiRect::bottom(px(3.0))},
        }
        template_value(card_marker)
        ThemeBackgroundColor(bg)
        ThemeBorderColor(border)
        Children [(widgets::collapsible_titled(title, title_marker, None, false, parts))]
    }
}

/// 位姿卡片的背景/边框 token
fn pose_card_tokens(selected: bool) -> (ThemeToken, ThemeToken) {
    match selected {
        true => (theme::POSE_SELECTED_BG, theme::POSE_SELECTED_BORDER),
        false => (theme::CARD_BG, theme::CARD_BORDER),
    }
}

/// 位姿标题的文字 token
fn pose_title_token(selected: bool) -> ThemeToken {
    match selected {
        true => theme::POSE_SELECTED_TEXT,
        false => theme::SECTION_TEXT,
    }
}

/// 一个部位的位姿：位置三分量 + 姿态四分量
fn pose_part(part: PosePart, path: &[usize], pose: &RobotPose) -> impl Scene {
    let values = part.get(pose);
    let position: Vec<BoxedScene> = PoseComp::POSITION
        .iter()
        .map(|&comp| {
            boxed(widgets::number_field(
                comp.get(values),
                axis_of(comp),
                ValueBinding::new(path.to_vec(), ValueSlot::Pose(part, comp)),
            ))
        })
        .collect();
    let orientation: Vec<BoxedScene> = PoseComp::ORIENTATION
        .iter()
        .map(|&comp| {
            boxed(widgets::number_field(
                comp.get(values),
                Some((quat_axis_token(comp), comp.label())),
                ValueBinding::new(path.to_vec(), ValueSlot::Pose(part, comp)),
            ))
        })
        .collect();

    bsn! {
        Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(3),
            margin: {UiRect::bottom(px(5.0))},
        }
        Children [
            (widgets::subheading(part.label().to_string())),
            (widgets::field_row(
                "位置 xyz",
                widgets::row(4.0, position),
            )),
            (widgets::field_row(
                "姿态 wxyz",
                widgets::row(4.0, orientation),
            ))
        ]
    }
}

// ============================================================
// 标量
// ============================================================

/// 标量字段一行
fn scalar_row(field: &ContextField, path: Vec<usize>) -> impl Scene {
    let key = field.key.clone();
    let control: BoxedScene = match &field.value {
        ContextValue::Bool(value) => boxed(widgets::checkbox(
            *value,
            BoolBinding {
                field_path: path.clone(),
            },
        )),
        ContextValue::Integer(value) => boxed(widgets::int_field(
            *value,
            ValueBinding::new(path.clone(), ValueSlot::Scalar),
        )),
        ContextValue::Float(value) => boxed(widgets::number_field(
            *value,
            None,
            ValueBinding::new(path.clone(), ValueSlot::Scalar),
        )),
        _ => boxed(widgets::readonly_value("(类型不支持)")),
    };
    widgets::field_row(key, control)
}

// ============================================================
// 数组
// ============================================================

/// 数组字段：一维直接列出，二维每行懒加载
fn array_section(field: &ContextField, path: Vec<usize>) -> BoxedScene {
    match &field.value {
        ContextValue::NumericArray { values, .. } => {
            let title = format!("{} [{} 个元素]", field.key, values.len());
            let rows: Vec<BoxedScene> = values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    boxed(widgets::field_row(
                        format!("[{index}]"),
                        widgets::number_field(
                            *value,
                            None,
                            ValueBinding::new(path.clone(), ValueSlot::Array1D(index)),
                        ),
                    ))
                })
                .collect();
            boxed(widgets::collapsible(title, None, false, rows))
        }
        ContextValue::NumericArray2D { rows, .. } => {
            let cols = rows.first().map(|r| r.len()).unwrap_or(0);
            let title = format!("{} [{} x {}]", field.key, rows.len(), cols);
            let row_sections: Vec<BoxedScene> = (0..rows.len())
                .map(|row_index| {
                    boxed(lazy_collapsible(
                        format!("[{row_index}]"),
                        path.clone(),
                        LazyKind::Array2DRow(row_index),
                    ))
                })
                .collect();
            boxed(widgets::collapsible(title, None, false, row_sections))
        }
        _ => boxed(widgets::readonly_value("(类型不支持)")),
    }
}

/// 二维数组某一行的各列输入框
fn array2d_row_body(values: &[f64], path: &[usize], row_index: usize) -> Vec<BoxedScene> {
    values
        .iter()
        .enumerate()
        .map(|(col, value)| {
            boxed(widgets::field_row(
                format!("[{col}]"),
                widgets::number_field(
                    *value,
                    None,
                    ValueBinding::new(path.to_vec(), ValueSlot::Array2D(row_index, col)),
                ),
            ))
        })
        .collect()
}

// ============================================================
// 轨迹
// ============================================================

/// 轨迹字段：每个点一个折叠区块，内容懒加载
fn trajectory_section(field: &ContextField, path: Vec<usize>) -> impl Scene {
    let ContextValue::JointTrajectory(points) = &field.value else {
        unreachable!("调用方已按类型分组");
    };
    let axes = points.first().map(|p| p.positions.len()).unwrap_or(0);
    let title = format!("{} [{} 个点, {} 轴]", field.key, points.len(), axes);

    let sections: Vec<BoxedScene> = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            boxed(lazy_collapsible(
                format!("[{index}] t = {:.3}s", point.time_from_start),
                path.clone(),
                LazyKind::TrajPoint(index),
            ))
        })
        .collect();

    widgets::collapsible(title, None, false, sections)
}

/// 单个轨迹点的时间与关节值
fn traj_point_body(point: &TrajectoryPoint, path: &[usize], index: usize) -> Vec<BoxedScene> {
    let mut rows: Vec<BoxedScene> = vec![boxed(widgets::field_row(
        "时间",
        widgets::number_field(
            point.time_from_start,
            None,
            ValueBinding::new(path.to_vec(), ValueSlot::TrajTime(index)),
        ),
    ))];
    rows.extend(point.positions.iter().enumerate().map(|(joint, value)| {
        boxed(widgets::field_row(
            format!("关节 {joint}"),
            widgets::number_field(
                *value,
                None,
                ValueBinding::new(path.to_vec(), ValueSlot::TrajJoint(index, joint)),
            ),
        ))
    }));
    rows
}

// ============================================================
// 嵌套分组
// ============================================================

/// 嵌套分组：递归复用同一套分组结构
fn nested_section(
    field: &ContextField,
    path: Vec<usize>,
    selected: Option<&PoseTarget>,
) -> impl Scene {
    let ContextValue::NestedGroup(children) = &field.value else {
        unreachable!("调用方已按类型分组");
    };
    let title = format!("{} ({} 项)", field.key, children.len());

    let body: Vec<BoxedScene> = match children.is_empty() {
        true => vec![boxed(widgets::readonly_value("(空)"))],
        false => field_groups(children, &path, selected),
    };
    widgets::collapsible(title, None, false, body)
}

// ============================================================
// 其他类型
// ============================================================

/// 其他字段：null / 文本 / 位姿数组 / 原始 JSON
fn other_field(
    field: &ContextField,
    path: Vec<usize>,
    selected: Option<&PoseTarget>,
) -> BoxedScene {
    let key = field.key.clone();
    match &field.value {
        ContextValue::Null => {
            // 名字里带 pose 的 null 字段可以一键建成默认位姿
            let create: Vec<BoxedScene> = key
                .contains("pose")
                .then(|| {
                    boxed(widgets::button(
                        "创建位姿",
                        ButtonVariant::Normal,
                        ActionButton(AppAction::CreatePose(path.clone())),
                    ))
                })
                .into_iter()
                .collect();
            boxed(widgets::field_row(
                key,
                widgets::row(
                    6.0,
                    bsn_list![(widgets::readonly_value("null")), { create }],
                ),
            ))
        }
        ContextValue::Text(value) => boxed(widgets::field_row(
            key,
            widgets::text_field(
                value.clone(),
                TextBinding {
                    field_path: path.clone(),
                },
            ),
        )),
        ContextValue::PoseArray(poses) => {
            let title = format!("{key} [{} 个位姿]", poses.len());
            let body: Vec<BoxedScene> = match poses.is_empty() {
                true => vec![boxed(widgets::readonly_value("(空)"))],
                false => (0..poses.len())
                    .map(|index| {
                        let pose = &poses[index];
                        let parts: Vec<BoxedScene> = PosePart::ALL
                            .iter()
                            .map(|&part| boxed(pose_array_part(part, &path, index, pose)))
                            .collect();
                        let target = PoseTarget::array_element(path.clone(), index);
                        let is_selected = selected == Some(&target);
                        boxed(selectable_pose_card(
                            format!("[{index}]"),
                            target,
                            is_selected,
                            parts,
                        ))
                    })
                    .collect(),
            };
            boxed(widgets::collapsible(title, None, false, body))
        }
        ContextValue::RawJson(value) => boxed(widgets::field_row(
            key,
            widgets::readonly_value(value.to_string()),
        )),
        _ => boxed(widgets::field_row(
            key,
            widgets::readonly_value("(未知类型)"),
        )),
    }
}

/// 位姿数组中某个位姿的一个部位
fn pose_array_part(part: PosePart, path: &[usize], index: usize, pose: &RobotPose) -> impl Scene {
    let values = part.get(pose);
    let field = |comp: PoseComp| {
        boxed(widgets::number_field(
            comp.get(values),
            Some((quat_axis_token(comp), comp.label())),
            ValueBinding::new(path.to_vec(), ValueSlot::PoseArray(index, part, comp)),
        ))
    };
    let position: Vec<BoxedScene> = PoseComp::POSITION.iter().map(|&c| field(c)).collect();
    let orientation: Vec<BoxedScene> = PoseComp::ORIENTATION.iter().map(|&c| field(c)).collect();

    bsn! {
        Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            margin: {UiRect::bottom(px(4.0))},
        }
        Children [
            (widgets::subheading(part.short_label().to_string())),
            (widgets::field_row("位置 xyz", widgets::row(4.0, position))),
            (widgets::field_row("姿态 wxyz", widgets::row(4.0, orientation)))
        ]
    }
}

// ============================================================
// 懒加载折叠块
// ============================================================

/// 内容懒加载的折叠区块：展开时才构建子控件
fn lazy_collapsible(title: String, path: Vec<usize>, kind: LazyKind) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
        }
        Collapsible { open: false }
        Children [
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(6),
                    padding: {UiRect::axes(px(theme::PAD_SM), px(4.0))},
                    border_radius: {BorderRadius::all(px(theme::RADIUS_SM))},
                    width: percent(100),
                }
                super::widgets::CollapseHeader
                ThemeBackgroundColor({theme::CARD_HEADER_BG})
                Children [
                    (
                        Text("▸")
                        super::widgets::CollapseChevron
                        ThemeTextColor({theme::SECTION_TEXT})
                        TextFont { font_size: px(10.0) }
                    ),
                    (
                        Text(title)
                        ThemeTextColor({theme::SECTION_TEXT})
                        TextFont { font_size: px(12.0) }
                    )
                ]
            ),
            (
                Node {
                    display: {Display::None},
                    flex_direction: FlexDirection::Column,
                    row_gap: px(2),
                    padding: {UiRect::new(px(theme::INDENT), px(0.0), px(3.0), px(2.0))},
                    width: percent(100),
                }
                super::widgets::CollapseBody
                template_value(LazyBody { path, kind })
            )
        ]
    }
}

/// 展开懒加载区块时构建内容
fn fill_lazy_bodies(
    editor: Res<Editor>,
    sections: Query<(&Collapsible, &Children)>,
    bodies: Query<(Entity, &LazyBody), Without<LazyFilled>>,
    mut commands: Commands,
) {
    let Some(data) = &editor.data else {
        return;
    };
    for (section, children) in &sections {
        if !section.open {
            continue;
        }
        for child in children.iter() {
            let Ok((entity, lazy)) = bodies.get(child) else {
                continue;
            };
            let Some(field) = crate::model::field_at_path(&data.context_fields, &lazy.path) else {
                continue;
            };
            let content: Vec<BoxedScene> = match (&field.value, lazy.kind) {
                (ContextValue::JointTrajectory(points), LazyKind::TrajPoint(index)) => {
                    match points.get(index) {
                        Some(point) => traj_point_body(point, &lazy.path, index),
                        None => continue,
                    }
                }
                (ContextValue::NumericArray2D { rows, .. }, LazyKind::Array2DRow(index)) => {
                    match rows.get(index) {
                        Some(row) => array2d_row_body(row, &lazy.path, index),
                        None => continue,
                    }
                }
                _ => continue,
            };
            commands
                .entity(entity)
                .insert(LazyFilled)
                .queue_spawn_related_scenes::<Children>(content);
        }
    }
}

// ============================================================
// 重建与同步
// ============================================================

/// 结构变化时重建编辑器
fn rebuild_editor(
    editor: Res<Editor>,
    mut rendered: ResMut<RenderedEditor>,
    slots: Query<Entity, With<EditorSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    let Ok(slot) = slots.single() else {
        return;
    };
    if rendered.structure == Some(editor.structure_version) {
        return;
    }
    if pending.contains(slot) {
        return;
    }
    debug!(
        was = ?rendered.structure,
        now = editor.structure_version,
        has_data = editor.data.is_some(),
        "重建编辑器"
    );
    rendered.structure = Some(editor.structure_version);
    rendered.selection.clone_from(&editor.selected_pose);

    let content: Vec<BoxedScene> = match &editor.data {
        Some(data) => {
            let mut items: Vec<BoxedScene> = vec![boxed(metadata_card(data, &editor))];
            let groups = field_groups(&data.context_fields, &[], editor.selected_pose.as_ref());
            items.push(boxed(widgets::card("Context 参数", groups)));
            items
        }
        None => vec![boxed(bsn! {
            Node {
                width: percent(100),
                height: px(200),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
            }
            Children [(widgets::hint("请从左侧选择一个文件进行编辑"))]
        })],
    };

    widgets::replace_slot_children(&mut commands, slot, content);
}

/// 操作栏按状态重建
fn rebuild_action_bar(
    editor: Res<Editor>,
    session: Res<Session>,
    mut rendered: ResMut<RenderedEditor>,
    slots: Query<Entity, With<ActionBarSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    let state = (editor.data.is_some(), session.is_connected);
    if rendered.action_bar == Some(state) {
        return;
    }
    let Ok(slot) = slots.single() else {
        return;
    };
    if pending.contains(slot) {
        return;
    }
    rendered.action_bar = Some(state);
    widgets::replace_slot_children(&mut commands, slot, action_bar(state.0, state.1));
}

/// 选中位姿变化：只换配色，不重建控件
fn sync_pose_selection(
    editor: Res<Editor>,
    mut rendered: ResMut<RenderedEditor>,
    cards: Query<(Entity, Ref<PoseCard>)>,
    titles: Query<(Entity, Ref<PoseCardTitle>)>,
    mut commands: Commands,
) {
    let changed = rendered.selection != editor.selected_pose;
    rendered.selection.clone_from(&editor.selected_pose);
    let selected = editor.selected_pose.as_ref();

    for (entity, card) in &cards {
        if !changed && !card.is_added() {
            continue;
        }
        let (bg, border) = pose_card_tokens(selected == Some(&card.target));
        commands
            .entity(entity)
            .insert((ThemeBackgroundColor(bg), ThemeBorderColor(border)));
    }
    for (entity, title) in &titles {
        if !changed && !title.is_added() {
            continue;
        }
        let token = pose_title_token(selected == Some(&title.target));
        commands.entity(entity).insert(ThemeTextColor(token));
    }
}

/// 新建的数值输入：把初值推给控件
fn push_initial_values(
    inits: Query<(Entity, &NumberFieldInit, &Children)>,
    mut texts: Query<&mut EditableText>,
    mut commands: Commands,
) {
    for (entity, init, children) in &inits {
        // 数值输入根节点先生成时，其内部文本框可能尚未就绪，保留初值标记重试。
        let Some(child) = children.iter().find(|child| texts.contains(*child)) else {
            continue;
        };
        if let Ok(mut editable) = texts.get_mut(child) {
            editable.max_characters = Some(widgets::NUMBER_MAX_CHARACTERS);
        }
        let value = match init.0 {
            NumberInitValue::Zero => NumberInputValue::F64(0.0),
            NumberInitValue::F64(v) => NumberInputValue::F64(v),
            NumberInitValue::I64(v) => NumberInputValue::I64(v),
        };
        commands.trigger(UpdateNumberInput { entity, value });
        // 初值只推一次，推完摘掉标记
        commands.entity(entity).remove::<NumberFieldInit>();
    }
}

/// 数据里的数值变了（ROS2 回填）：刷新已有控件显示
fn push_refreshed_values(
    editor: Res<Editor>,
    mut rendered: ResMut<RenderedEditor>,
    validation: Res<InputValidation>,
    fields: Query<(Entity, &ValueBinding), With<FeathersNumberInput>>,
    mut commands: Commands,
) {
    if rendered.values == editor.value_version {
        return;
    }
    rendered.values = editor.value_version;
    let Some(data) = &editor.data else {
        return;
    };
    for (entity, binding) in &fields {
        // 用户仍在修正的文本由其本人决定，ROS2 回填不能用模型旧值悄悄覆盖它。
        if validation.field_is_invalid(&editor, entity) {
            continue;
        }
        if let Some(value) = read_f64(data, binding) {
            commands.trigger(UpdateNumberInput {
                entity,
                value: NumberInputValue::F64(value),
            });
        }
    }
}

// ============================================================
// 输入回写
// ============================================================

/// 元数据文本框 → 数据
fn on_meta_edit(
    change: On<TextEditChange>,
    fields: Query<(&MetaField, &EditableText)>,
    mut editor: ResMut<Editor>,
) {
    let Ok((field, editable)) = fields.get(change.event_target()) else {
        return;
    };
    let value = editable.value().to_string();
    let Some(data) = &mut editor.data else {
        return;
    };
    let target = match field {
        MetaField::MapId => &mut data.map_id,
        MetaField::TaskId => &mut data.task_id,
    };
    if *target != value {
        *target = value;
    }
}

/// context 文本框 → 数据
fn on_text_field_edit(
    change: On<TextEditChange>,
    fields: Query<(&TextBinding, &EditableText)>,
    mut editor: ResMut<Editor>,
) {
    let Ok((binding, editable)) = fields.get(change.event_target()) else {
        return;
    };
    let value = editable.value().to_string();
    let binding = binding.clone();
    if let Some(data) = &mut editor.data {
        apply_text(data, &binding, &value);
    }
}

/// f64 数值输入 → 数据
fn on_f64_change(
    change: On<ValueChange<f64>>,
    fields: Query<&ValueBinding>,
    mut editor: ResMut<Editor>,
    mut validation: ResMut<InputValidation>,
    mut status: ResMut<StatusLine>,
    mut commands: Commands,
) {
    let Ok(binding) = fields.get(change.source) else {
        return;
    };
    let binding = binding.clone();
    let value = change.value;
    if !value.is_finite() {
        validation.set_invalid(&editor, change.source, true);
        status.set("输入错误：数值超出有限浮点数范围，请修正标红数值后再保存");
        commands
            .entity(change.source)
            .insert(ThemeBorderColor(theme::STATUS_ERROR));
        return;
    }
    if let Some(data) = &mut editor.data {
        apply_f64(data, &binding, value);
    }
}

/// i64 数值输入 → 数据
fn on_i64_change(
    change: On<ValueChange<i64>>,
    fields: Query<&ValueBinding>,
    mut editor: ResMut<Editor>,
) {
    let Ok(binding) = fields.get(change.source) else {
        return;
    };
    let binding = binding.clone();
    let value = change.value;
    if let Some(data) = &mut editor.data {
        apply_i64(data, &binding, value);
    }
}

/// 复选框 → 数据
fn on_bool_change(
    change: On<ValueChange<bool>>,
    fields: Query<&BoolBinding>,
    mut editor: ResMut<Editor>,
    mut commands: Commands,
) {
    let Ok(binding) = fields.get(change.source) else {
        return;
    };
    let binding = binding.clone();
    let value = change.value;
    if let Some(data) = &mut editor.data {
        apply_bool(data, &binding, value);
    }
    // Feathers 的复选框不自己维护 Checked，勾选状态由使用方写回
    let mut entity = commands.entity(change.source);
    match value {
        true => entity.insert(Checked),
        false => entity.remove::<Checked>(),
    };
}

/// 点击位姿卡片：选中该点位
fn on_pose_card_click(
    mut click: On<Pointer<Click>>,
    cards: Query<&PoseCard>,
    parents: Query<&ChildOf>,
    mut writer: MessageWriter<AppAction>,
) {
    if click.button != bevy::picking::pointer::PointerButton::Primary {
        return;
    }
    // 点中的多半是卡片标题或里面的控件，往上找到挂了 PoseCard 的那层
    let Some(entity) = widgets::self_or_ancestor(click.entity, &parents, |e| cards.contains(e))
    else {
        return;
    };
    let Ok(card) = cards.get(entity) else {
        return;
    };
    click.propagate(false);
    writer.write(AppAction::SelectPose(card.target.clone()));
}

/// 编辑器插件
pub struct EditorPanelPlugin;

impl Plugin for EditorPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderedEditor>()
            .init_resource::<InputValidation>()
            .add_observer(on_meta_edit)
            .add_observer(on_text_field_edit)
            .add_observer(on_f64_change)
            .add_observer(on_number_text_edit)
            .add_observer(on_i64_change)
            .add_observer(on_bool_change)
            .add_observer(on_pose_card_click)
            .add_systems(
                Update,
                (
                    rebuild_editor,
                    rebuild_action_bar,
                    // 先提交旧树销毁，再查询现存控件；共享资源只限制并行，不会刷新延迟命令。
                    (
                        sync_pose_selection,
                        sync_pose_hint,
                        sync_document_source,
                        fill_lazy_bodies,
                        push_initial_values,
                        push_refreshed_values,
                        clear_stale_input_errors,
                    )
                        .after(rebuild_editor),
                )
                    .in_set(UiSet::Rebuild),
            );
    }
}

#[cfg(test)]
#[path = "editor/lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "editor/pose_selection_tests.rs"]
mod pose_selection_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input_focus::InputFocus;
    use bevy::scene::ScenePlugin;
    use bevy::text::{FontCx, LayoutCx, TextEdit, apply_text_edits};

    /// 使用真实 Feathers 场景和文本编辑流程，无窗口、网络与帧间等待。
    fn number_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
            .init_asset::<Font>()
            .init_resource::<InputFocus>()
            .init_resource::<FontCx>()
            .init_resource::<LayoutCx>()
            .init_resource::<bevy::clipboard::Clipboard>()
            .init_resource::<Editor>()
            .init_resource::<RenderedEditor>()
            .init_resource::<InputValidation>()
            .init_resource::<StatusLine>()
            .add_observer(on_number_text_edit)
            .add_observer(on_f64_change)
            .add_observer(on_i64_change)
            .add_systems(
                Update,
                (
                    push_initial_values,
                    push_refreshed_values,
                    clear_stale_input_errors,
                ),
            )
            .add_systems(PostUpdate, apply_text_edits);
        // Parley 的全选依赖字形布局；加载仓库字体让无窗口测试具备真实选区语义。
        // 空字体集合没有字形，SelectAll 会退化为空选区，不能用于验证替换编辑。
        let font = Font::from_bytes(
            include_bytes!("../../assets/fonts/SarasaTermSCNerd-Regular.ttf").to_vec(),
        );
        let mut fonts = app.world_mut().resource_mut::<FontCx>();
        let registered = fonts.collection.register_fonts(font.data, None);
        let family = fonts
            .collection
            .family_name(registered[0].0)
            .unwrap()
            .to_string();
        fonts.set_sans_serif_family(&family).unwrap();
        app
    }

    fn numeric_text(app: &mut App, field: Entity) -> Entity {
        let children = app.world().get::<Children>(field).unwrap();
        children
            .iter()
            .find(|child| app.world().get::<EditableText>(*child).is_some())
            .unwrap()
    }

    fn replace_text(app: &mut App, field: Entity, value: &str) {
        let entity = numeric_text(app, field);
        let mut editable = app.world_mut().get_mut::<EditableText>(entity).unwrap();
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert(value.to_string().into()));
        app.update();
    }

    fn has_errors(app: &App) -> bool {
        app.world()
            .resource::<InputValidation>()
            .has_errors(app.world().resource::<Editor>())
    }

    fn float_field(app: &mut App, value: f64) -> Entity {
        let data = crate::model::parse_task_graph(
            r#"{"map_id":"m","task_id":"t","config":{"context":{"speed":1.5}}}"#,
        )
        .unwrap();
        app.world_mut().resource_mut::<Editor>().load(Some(data));
        app.world_mut()
            .spawn_scene(widgets::number_field(
                value,
                None,
                ValueBinding::new(vec![0], ValueSlot::Scalar),
            ))
            .unwrap()
            .id()
    }

    #[test]
    fn 合法极小极大浮点初值和外部回填均完整显示() {
        for value in [
            1e-20,
            f64::from_bits(1),
            -f64::from_bits(1),
            f64::MAX,
            -f64::MAX,
        ] {
            let mut app = number_app();
            let field = float_field(&mut app, value);
            app.update();
            let text = numeric_text(&mut app, field);
            let editable = app.world().get::<EditableText>(text).unwrap();
            assert_eq!(
                editable.max_characters,
                Some(widgets::NUMBER_MAX_CHARACTERS)
            );
            assert_eq!(
                editable
                    .value()
                    .to_string()
                    .parse::<f64>()
                    .unwrap()
                    .to_bits(),
                value.to_bits()
            );

            let refreshed = -value;
            {
                let mut editor = app.world_mut().resource_mut::<Editor>();
                assert!(apply_f64(
                    editor.data.as_mut().unwrap(),
                    &ValueBinding::new(vec![0], ValueSlot::Scalar),
                    refreshed
                ));
                editor.mark_values_changed();
            }
            app.update();
            let refreshed_text = app
                .world()
                .get::<EditableText>(text)
                .unwrap()
                .value()
                .to_string();
            assert_eq!(
                refreshed_text
                    .parse::<f64>()
                    .unwrap_or_else(|_| panic!("回填 {refreshed:?} 后文本为 {refreshed_text:?}"))
                    .to_bits(),
                refreshed.to_bits()
            );
        }
    }

    #[test]
    fn 数值输入错误可见且阻止保存并由用户修正解除() {
        let mut app = number_app();
        let field = float_field(&mut app, 1.5);
        app.update();
        for invalid in ["1e999", "-1e999", "1e", ""] {
            replace_text(&mut app, field, invalid);
            assert!(has_errors(&app), "应拦截 {invalid:?}");
            assert!(
                app.world()
                    .resource::<StatusLine>()
                    .text
                    .contains("输入错误")
            );
            assert_eq!(
                app.world().get::<ThemeBorderColor>(field).unwrap().0,
                theme::STATUS_ERROR
            );
            let editor = app.world().resource::<Editor>();
            assert_eq!(
                read_f64(
                    editor.data.as_ref().unwrap(),
                    &ValueBinding::new(vec![0], ValueSlot::Scalar)
                ),
                Some(1.5)
            );
        }
        replace_text(&mut app, field, "2.25");
        let text = numeric_text(&mut app, field);
        assert!(
            !has_errors(&app),
            "修正后的文本为 {:?}",
            app.world()
                .get::<EditableText>(text)
                .unwrap()
                .value()
                .to_string()
        );
        assert_eq!(
            app.world().get::<ThemeBorderColor>(field).unwrap().0,
            theme::INPUT_BORDER
        );

        replace_text(&mut app, field, "1e999");
        {
            let mut editor = app.world_mut().resource_mut::<Editor>();
            assert!(apply_f64(
                editor.data.as_mut().unwrap(),
                &ValueBinding::new(vec![0], ValueSlot::Scalar),
                1e-20
            ));
            editor.mark_values_changed();
        }
        app.update();
        assert!(has_errors(&app));
        let text = numeric_text(&mut app, field);
        assert_eq!(
            app.world()
                .get::<EditableText>(text)
                .unwrap()
                .value()
                .to_string(),
            "1e999"
        );
        replace_text(&mut app, field, "3.25");
        assert!(!has_errors(&app));
        assert_eq!(
            app.world()
                .get::<EditableText>(text)
                .unwrap()
                .value()
                .to_string()
                .parse::<f64>()
                .unwrap(),
            3.25
        );
    }

    #[test]
    fn 删除错误控件和切换文档后不残留保存阻塞() {
        let mut app = number_app();
        let field = float_field(&mut app, 1.5);
        app.update();
        replace_text(&mut app, field, "1e999");
        app.world_mut().entity_mut(field).despawn();
        app.update();
        assert!(!has_errors(&app));

        let field = float_field(&mut app, 1.5);
        app.update();
        replace_text(&mut app, field, "1e999");
        app.world_mut().resource_mut::<Editor>().load(None);
        assert!(!has_errors(&app));
    }

    #[test]
    fn 整数范围与浮点语法按控件类型校验() {
        assert!(validate_number_text(&i64::MIN.to_string(), NumberFormat::I64).is_ok());
        assert!(validate_number_text(&i64::MAX.to_string(), NumberFormat::I64).is_ok());
        for text in [
            "9223372036854775808",
            "-9223372036854775809",
            "1.5",
            "1e2",
            "",
        ] {
            assert!(validate_number_text(text, NumberFormat::I64).is_err());
        }
        for text in ["NaN", "inf", "-inf", "1e999", ""] {
            assert!(validate_number_text(text, NumberFormat::F64).is_err());
        }
    }
}
