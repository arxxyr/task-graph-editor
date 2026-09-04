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
    ButtonVariant, FeathersNumberInput, NumberInputValue, UpdateNumberInput,
};
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemeToken};
use bevy::feathers::tokens;
use bevy::prelude::*;
use bevy::text::{EditableText, TextEditChange};
use bevy::ui::Checked;
use bevy::ui_widgets::ValueChange;

use crate::model::{ContextField, ContextValue, RobotPose, TaskGraphData, TrajectoryPoint};

use super::binding::{
    BoolBinding, PoseComp, PosePart, TextBinding, ValueBinding, ValueSlot, apply_bool, apply_f64,
    apply_i64, apply_text, read_f64,
};
use super::connect::ActionButton;
use super::shell::{ActionBarSlot, EditorSlot};
use super::theme;
use super::widgets::{self, BoxedScene, Collapsible, NumberFieldInit, NumberInitValue, boxed};
use super::worker_bridge::AppAction;
use super::{Editor, Session, UiSet};

/// 元数据字段
#[derive(Component, Clone, Copy, Default, PartialEq, Eq)]
enum MetaField {
    /// 地图 ID
    #[default]
    MapId,
    /// 任务 ID
    TaskId,
}

/// 位姿卡片，记录自身的索引路径（用于选中）
#[derive(Component, Clone, Default)]
struct PoseCard {
    /// 字段索引路径
    path: Vec<usize>,
}

/// 位姿卡片的标题文字（选中时变色）
#[derive(Component, Clone, Default)]
struct PoseCardTitle {
    /// 字段索引路径
    path: Vec<usize>,
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
    /// 已渲染的操作栏状态（有数据、已连接、忙碌、选中位姿）
    action_bar: Option<(bool, bool, bool, bool)>,
    /// 已渲染的选中路径
    selection: Option<Vec<usize>>,
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
fn action_bar(has_data: bool, connected: bool, busy: bool, has_pose: bool) -> Vec<BoxedScene> {
    if !has_data || !connected {
        return Vec::new();
    }

    let idle = !busy;
    let mut items: Vec<BoxedScene> = vec![boxed(widgets::button_enabled(
        "应用到远程文件",
        ButtonVariant::Primary,
        idle,
        ActionButton(AppAction::SaveToRemote),
    ))];

    // 三个 ROS2 取数按钮还要求先选中一个位姿点位
    let can_fetch = idle && has_pose;
    let fetches = [
        ("获取底盘位姿", AppAction::FetchChassisPose),
        ("获取头部关节", AppAction::FetchHeadJoints),
        ("获取腰部关节", AppAction::FetchWaistJoints),
    ];
    for (label, action) in fetches {
        items.push(boxed(widgets::button_enabled(
            label,
            ButtonVariant::Normal,
            can_fetch,
            ActionButton(action),
        )));
    }

    if !has_pose {
        items.push(boxed(widgets::hint("← 需先选中一个位姿点位")));
    }
    items
}

// ============================================================
// 元数据
// ============================================================

/// 元数据卡片
fn metadata_card(data: &TaskGraphData) -> impl Scene {
    widgets::card(
        "元数据",
        bsn_list![
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
    selected: Option<&[usize]>,
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
            ContextValue::NumericArray(_) | ContextValue::NumericArray2D(_) => &mut arrays,
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
                let is_selected = selected == Some(path.as_slice());
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
            .map(|&i| other_field(&fields[i], child_path(path_prefix, i)))
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

    let (bg, border) = pose_card_tokens(selected);
    let card_marker = PoseCard { path: path.clone() };
    let title_marker = PoseCardTitle { path };
    let key = field.key.clone();

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
        Children [(widgets::collapsible_titled(key, title_marker, None, false, parts))]
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
        ContextValue::NumericArray(values) => {
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
        ContextValue::NumericArray2D(rows) => {
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
    selected: Option<&[usize]>,
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
fn other_field(field: &ContextField, path: Vec<usize>) -> BoxedScene {
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
                        boxed(widgets::collapsible(
                            format!("[{index}]"),
                            None,
                            false,
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
                (ContextValue::NumericArray2D(rows), LazyKind::Array2DRow(index)) => {
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
    mut commands: Commands,
) {
    let Ok(slot) = slots.single() else {
        return;
    };
    if rendered.structure == Some(editor.structure_version) {
        return;
    }
    debug!(
        was = ?rendered.structure,
        now = editor.structure_version,
        has_data = editor.data.is_some(),
        "重建编辑器"
    );
    rendered.structure = Some(editor.structure_version);
    rendered.selection.clone_from(&editor.selected_pose_path);

    let content: Vec<BoxedScene> = match &editor.data {
        Some(data) => {
            let mut items: Vec<BoxedScene> = vec![boxed(metadata_card(data))];
            let groups = field_groups(
                &data.context_fields,
                &[],
                editor.selected_pose_path.as_deref(),
            );
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

    commands
        .entity(slot)
        .despawn_related::<Children>()
        .queue_spawn_related_scenes::<Children>(content);
}

/// 操作栏按状态重建
fn rebuild_action_bar(
    editor: Res<Editor>,
    session: Res<Session>,
    mut rendered: ResMut<RenderedEditor>,
    slots: Query<Entity, With<ActionBarSlot>>,
    mut commands: Commands,
) {
    let state = (
        editor.data.is_some(),
        session.is_connected,
        session.is_busy(),
        editor.has_pose_selection(),
    );
    if rendered.action_bar == Some(state) {
        return;
    }
    let Ok(slot) = slots.single() else {
        return;
    };
    rendered.action_bar = Some(state);
    commands
        .entity(slot)
        .despawn_related::<Children>()
        .queue_spawn_related_scenes::<Children>(action_bar(state.0, state.1, state.2, state.3));
}

/// 选中位姿变化：只换配色，不重建控件
fn sync_pose_selection(
    editor: Res<Editor>,
    mut rendered: ResMut<RenderedEditor>,
    cards: Query<(Entity, &PoseCard)>,
    titles: Query<(Entity, &PoseCardTitle)>,
    mut commands: Commands,
) {
    if rendered.selection == editor.selected_pose_path {
        return;
    }
    rendered.selection.clone_from(&editor.selected_pose_path);
    let selected = editor.selected_pose_path.as_deref();

    for (entity, card) in &cards {
        let (bg, border) = pose_card_tokens(selected == Some(card.path.as_slice()));
        commands
            .entity(entity)
            .insert((ThemeBackgroundColor(bg), ThemeBorderColor(border)));
    }
    for (entity, title) in &titles {
        let token = pose_title_token(selected == Some(title.path.as_slice()));
        commands.entity(entity).insert(ThemeTextColor(token));
    }
}

/// 新建的数值输入：把初值推给控件
fn push_initial_values(
    inits: Query<(Entity, &NumberFieldInit), Added<NumberFieldInit>>,
    mut commands: Commands,
) {
    for (entity, init) in &inits {
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
) {
    let Ok(binding) = fields.get(change.source) else {
        return;
    };
    let binding = binding.clone();
    let value = change.value;
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
    mut writer: MessageWriter<AppAction>,
) {
    let Ok(card) = cards.get(click.entity) else {
        return;
    };
    click.propagate(false);
    writer.write(AppAction::SelectPose(card.path.clone()));
}

/// 编辑器插件
pub struct EditorPanelPlugin;

impl Plugin for EditorPanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderedEditor>()
            .add_observer(on_meta_edit)
            .add_observer(on_text_field_edit)
            .add_observer(on_f64_change)
            .add_observer(on_i64_change)
            .add_observer(on_bool_change)
            .add_observer(on_pose_card_click)
            .add_systems(
                Update,
                (
                    rebuild_editor,
                    rebuild_action_bar,
                    sync_pose_selection,
                    fill_lazy_bodies,
                    push_initial_values,
                    push_refreshed_values,
                )
                    .in_set(UiSet::Rebuild),
            );
    }
}
