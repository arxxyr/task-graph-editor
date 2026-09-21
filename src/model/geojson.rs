//! 地图 GeoJSON 导航点同步：任务图 context 的底盘位姿 → `nav_point` 特征。
//!
//! 运行时以 GeoJSON 为准，导航点按 `pose_context_key` 注入 context；任务图保存后必须同步，
//! 否则编辑器里的示教结果不会生效。这里只做纯数据合并，不碰网络。
//!
//! GeoJSON 由上游平台生成，属于别的系统：只原地替换确实变化的数字字面量，
//! 键顺序、缩进和未改动数字的写法（如整数 `0`）逐字节保留。

use std::collections::BTreeSet;
use std::ops::Range;

use serde::Deserialize;
use serde_json::value::RawValue;

use super::{ContextValue, Pose, TaskGraphData};

/// 一次同步的输入：任务图身份与全部顶层位姿字段的底盘位姿。
#[derive(Debug, Clone, PartialEq)]
pub struct GeoSyncRequest {
    pub map_id: String,
    pub task_id: String,
    /// `(context 键, 底盘位姿)`；导航点只按顶层键关联，嵌套分组内的位姿不参与。
    pub chassis_poses: Vec<(String, Pose)>,
}

impl GeoSyncRequest {
    /// 没有任何顶层位姿时返回 `None`，保存流程不必再去读地图目录。
    pub fn from_task_graph(data: &TaskGraphData) -> Option<Self> {
        let chassis_poses: Vec<_> = data
            .context_fields
            .iter()
            .filter_map(|field| match &field.value {
                ContextValue::Pose(pose) => Some((field.key.clone(), pose.chassis_pose.clone())),
                _ => None,
            })
            .collect();
        (!chassis_poses.is_empty()).then(|| Self {
            map_id: data.map_id.clone(),
            task_id: data.task_id.clone(),
            chassis_poses,
        })
    }

    fn chassis_pose(&self, key: &str) -> Option<&Pose> {
        self.chassis_poses
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, pose)| pose)
    }
}

/// 被同步的一个导航点。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SyncedPoint {
    /// 导航点标识：特征 id，缺失时退回名称或下标
    pub feature: String,
    /// 对应的 context 位姿键
    pub context_key: String,
}

/// 单个 GeoJSON 文件与任务图的比对结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeoFileChanges {
    /// 需要写回的完整新内容；`None` 表示已与任务图一致
    pub content: Option<String>,
    /// 本次被更新的导航点
    pub updated: Vec<SyncedPoint>,
    /// 带 context 键的导航点总数
    pub linked_points: usize,
    /// 无法同步的导航点及原因
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GeoFilePlan {
    /// 不是当前任务图在当前地图上的导航点文件，不处理
    Unrelated,
    Linked(GeoFileChanges),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GeoSyncError {
    #[error("不是有效的 JSON: {0}")]
    Parse(String),
    #[error("位姿 {key} 含非有限数值，拒绝写入")]
    NonFinite { key: String },
    #[error("原地替换结果与预期不一致，已放弃写入")]
    VerificationFailed,
}

/// 多个 GeoJSON 文件的同步汇总，由保存回执带回界面。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeoSyncReport {
    /// 属于当前任务图的 GeoJSON 文件数
    pub matched_files: usize,
    /// 实际写回的文件名
    pub updated_files: Vec<String>,
    /// 被更新的导航点；多个文件里的同一个点只记一次
    pub updated: BTreeSet<SyncedPoint>,
    /// 无法同步的导航点及原因
    pub notes: BTreeSet<String>,
    /// 已写入但需要留意的情况，如目录持久化未确认
    pub warnings: Vec<String>,
    /// 读取或写入失败
    pub errors: Vec<String>,
}

impl GeoSyncReport {
    /// 状态栏文案；没有关联的 GeoJSON 且无异常时返回 `None`，保存提示保持原样。
    pub fn status_text(&self) -> Option<String> {
        let mut parts = Vec::new();
        match (self.updated_files.is_empty(), self.matched_files) {
            (false, _) => {
                let keys: BTreeSet<&str> = self
                    .updated
                    .iter()
                    .map(|point| point.context_key.as_str())
                    .collect();
                parts.push(format!(
                    "已同步 {} 个地图点位到 {} 个 GeoJSON：{}",
                    self.updated.len(),
                    self.updated_files.len(),
                    keys.into_iter().collect::<Vec<_>>().join("、")
                ));
            }
            (true, matched) if matched > 0 && self.errors.is_empty() => {
                parts.push("地图点位已一致".to_owned());
            }
            (true, _) => {}
        }
        if !self.errors.is_empty() {
            parts.push(format!("地图点位同步失败：{}", self.errors.join("；")));
        }
        if !self.warnings.is_empty() {
            parts.push(format!("地图点位警告：{}", self.warnings.join("；")));
        }
        if !self.notes.is_empty() {
            let notes: Vec<&str> = self.notes.iter().map(String::as_str).collect();
            // 「注意」让状态栏按警告配色：这些点位运行时仍是旧值。
            parts.push(format!("注意，未同步：{}", notes.join("；")));
        }
        (!parts.is_empty()).then(|| parts.join("；"))
    }
}

#[derive(Deserialize)]
struct RawCollection<'a> {
    #[serde(rename = "type")]
    kind: Option<serde_json::Value>,
    properties: Option<serde_json::Value>,
    #[serde(borrow)]
    features: Option<Vec<&'a RawValue>>,
}

#[derive(Deserialize)]
struct RawFeature<'a> {
    id: Option<serde_json::Value>,
    #[serde(borrow)]
    geometry: Option<&'a RawValue>,
    #[serde(borrow)]
    properties: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct RawGeometry<'a> {
    #[serde(rename = "type")]
    kind: Option<serde_json::Value>,
    #[serde(borrow)]
    coordinates: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct RawPointProperties<'a> {
    name: Option<serde_json::Value>,
    pose_context_key: Option<serde_json::Value>,
    source_context_key: Option<serde_json::Value>,
    #[serde(borrow)]
    orientation: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct RawOrientation<'a> {
    #[serde(borrow)]
    x: &'a RawValue,
    #[serde(borrow)]
    y: &'a RawValue,
    #[serde(borrow)]
    z: &'a RawValue,
    #[serde(borrow)]
    w: &'a RawValue,
}

/// 导航点里会被同步的六个数。
#[derive(Debug, Clone, Copy)]
enum Component {
    X,
    Y,
    OrientationX,
    OrientationY,
    OrientationZ,
    OrientationW,
}

impl Component {
    /// 该分量在已解析特征中的位置，用于核对原地替换的结果。
    fn slot(self, feature: &mut serde_json::Value) -> Option<&mut serde_json::Value> {
        let (parent, member) = match self {
            Self::X | Self::Y => ("geometry", "coordinates"),
            _ => ("properties", "orientation"),
        };
        let container = feature.get_mut(parent)?.get_mut(member)?;
        match self {
            Self::X => container.get_mut(0),
            Self::Y => container.get_mut(1),
            Self::OrientationX => container.get_mut("x"),
            Self::OrientationY => container.get_mut("y"),
            Self::OrientationZ => container.get_mut("z"),
            Self::OrientationW => container.get_mut("w"),
        }
    }
}

struct NumberEdit {
    feature_index: usize,
    component: Component,
    span: Range<usize>,
    value: f64,
}

/// 借用型 `RawValue` 是输入的子切片，由地址差得到它在原文中的字节范围。
fn span_of(text: &str, part: &str) -> Option<Range<usize>> {
    let start = (part.as_ptr() as usize).checked_sub(text.as_ptr() as usize)?;
    let end = start.checked_add(part.len())?;
    (end <= text.len()).then_some(start..end)
}

/// serde 派生的结构体同样接受数组写法；GeoJSON 的成员必须是对象，其他形式一律不认。
fn parse_object<'a, T: Deserialize<'a>>(text: &'a str) -> Option<T> {
    match text.starts_with('{') {
        true => serde_json::from_str(text).ok(),
        false => None,
    }
}

fn number_of(raw: &RawValue) -> Option<f64> {
    serde_json::from_str::<serde_json::Number>(raw.get())
        .ok()?
        .as_f64()
}

fn text_of(value: Option<&serde_json::Value>) -> Option<&str> {
    value?.as_str().filter(|text| !text.is_empty())
}

/// 当前位置与目标不同才产生替换；`0` 与 `0.0` 视为相同，不改写原有整数写法。
fn push_edit(
    edits: &mut Vec<NumberEdit>,
    text: &str,
    feature_index: usize,
    component: Component,
    raw: &RawValue,
    value: f64,
) -> Option<()> {
    if number_of(raw)? != value {
        edits.push(NumberEdit {
            feature_index,
            component,
            span: span_of(text, raw.get())?,
            value,
        });
    }
    Some(())
}

/// 收集一个导航点需要替换的数字；结构不符合预期时返回原因，整点不动。
fn point_edits(
    text: &str,
    feature_index: usize,
    geometry: Option<&RawValue>,
    orientation: Option<&RawValue>,
    pose: &Pose,
) -> Result<Vec<NumberEdit>, &'static str> {
    let geometry: RawGeometry = geometry
        .and_then(|raw| parse_object(raw.get()))
        .ok_or("缺少 geometry")?;
    if text_of(geometry.kind.as_ref()) != Some("Point") {
        return Err("几何不是 Point");
    }
    let coordinates: Vec<&RawValue> = geometry
        .coordinates
        .and_then(|raw| serde_json::from_str(raw.get()).ok())
        .ok_or("coordinates 不是数组")?;
    let [x, y, ..] = coordinates.as_slice() else {
        return Err("coordinates 不足两个分量");
    };
    let orientation: RawOrientation = orientation
        .and_then(|raw| parse_object(raw.get()))
        .ok_or("缺少 orientation 的 x/y/z/w")?;

    let mut edits = Vec::new();
    let targets = [
        (Component::X, *x, pose.position.x),
        (Component::Y, *y, pose.position.y),
        (Component::OrientationX, orientation.x, pose.orientation.x),
        (Component::OrientationY, orientation.y, pose.orientation.y),
        (Component::OrientationZ, orientation.z, pose.orientation.z),
        (Component::OrientationW, orientation.w, pose.orientation.w),
    ];
    for (component, raw, value) in targets {
        push_edit(&mut edits, text, feature_index, component, raw, value)
            .ok_or("坐标或姿态不是数字")?;
    }
    Ok(edits)
}

fn is_finite(pose: &Pose) -> bool {
    let (p, q) = (&pose.position, &pose.orientation);
    [p.x, p.y, q.x, q.y, q.z, q.w]
        .into_iter()
        .all(f64::is_finite)
}

fn feature_label(feature: &RawFeature, name: Option<&serde_json::Value>, index: usize) -> String {
    match (&feature.id, text_of(name)) {
        (Some(serde_json::Value::String(id)), _) if !id.is_empty() => id.clone(),
        (Some(serde_json::Value::Number(id)), _) => id.to_string(),
        (_, Some(name)) => name.to_owned(),
        _ => format!("features[{index}]"),
    }
}

/// 替换后的文本必须与「在解析结果上做同样修改」完全相同，否则放弃写入。
fn verified_splice(text: &str, mut edits: Vec<NumberEdit>) -> Result<String, GeoSyncError> {
    let parse = |text: &str| {
        serde_json::from_str::<serde_json::Value>(text)
            .map_err(|error| GeoSyncError::Parse(error.to_string()))
    };
    let mut expected = parse(text)?;
    let mut content = text.to_owned();
    // 从后往前替换，前面的字节范围不受影响。
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.span.start));
    for edit in &edits {
        let number =
            serde_json::Number::from_f64(edit.value).ok_or(GeoSyncError::VerificationFailed)?;
        content.replace_range(edit.span.clone(), &number.to_string());
        let slot = expected
            .get_mut("features")
            .and_then(|features| features.get_mut(edit.feature_index))
            .and_then(|feature| edit.component.slot(feature))
            .ok_or(GeoSyncError::VerificationFailed)?;
        *slot = serde_json::Value::Number(number);
    }
    match parse(&content) {
        Ok(actual) if actual == expected => Ok(content),
        _ => Err(GeoSyncError::VerificationFailed),
    }
}

/// 比对一个 GeoJSON 文件，给出把关联导航点同步为任务图底盘位姿所需的新内容。
///
/// 关联键优先取 `pose_context_key`（运行时注入 context 的目标），缺失时退回生成来源
/// `source_context_key`；同一个键可以对应多个导航点，全部更新。
pub fn plan_file_sync(text: &str, request: &GeoSyncRequest) -> Result<GeoFilePlan, GeoSyncError> {
    let parse_error = |error: serde_json::Error| GeoSyncError::Parse(error.to_string());
    if !text.trim_start().starts_with('{') {
        // 合法但不是对象的 JSON 与本任务无关；语法错误照常报告。
        return serde_json::from_str::<serde::de::IgnoredAny>(text)
            .map(|_| GeoFilePlan::Unrelated)
            .map_err(parse_error);
    }
    let collection: RawCollection = serde_json::from_str(text).map_err(parse_error)?;
    let properties = collection.properties.as_ref();
    let property = |key: &str| text_of(properties.and_then(|value| value.get(key)));
    let same_map = ["map_id", "map_code"]
        .into_iter()
        .filter_map(property)
        .all(|map| map == request.map_id);
    let related = text_of(collection.kind.as_ref()) == Some("FeatureCollection")
        && property("source_task_id") == Some(request.task_id.as_str())
        && same_map;
    let Some(features) = collection.features.filter(|_| related) else {
        return Ok(GeoFilePlan::Unrelated);
    };

    let mut changes = GeoFileChanges::default();
    let mut edits = Vec::new();
    for (index, raw) in features.into_iter().enumerate() {
        // 不是对象的元素无从判断是否关联，保持原样。
        let Some(feature) = parse_object::<RawFeature>(raw.get()) else {
            continue;
        };
        let Some(point) = feature
            .properties
            .and_then(|raw| parse_object::<RawPointProperties>(raw.get()))
        else {
            continue;
        };
        let Some(key) = text_of(point.pose_context_key.as_ref())
            .or_else(|| text_of(point.source_context_key.as_ref()))
        else {
            continue;
        };
        changes.linked_points += 1;
        let label = feature_label(&feature, point.name.as_ref(), index);
        let Some(pose) = request.chassis_pose(key) else {
            changes.notes.push(format!("{label}：任务图没有位姿 {key}"));
            continue;
        };
        if !is_finite(pose) {
            return Err(GeoSyncError::NonFinite {
                key: key.to_owned(),
            });
        }
        match point_edits(text, index, feature.geometry, point.orientation, pose) {
            Ok(point_edits) if point_edits.is_empty() => {}
            Ok(point_edits) => {
                edits.extend(point_edits);
                changes.updated.push(SyncedPoint {
                    feature: label,
                    context_key: key.to_owned(),
                });
            }
            Err(reason) => changes.notes.push(format!("{label}：{reason}")),
        }
    }
    if !edits.is_empty() {
        changes.content = Some(verified_splice(text, edits)?);
    }
    Ok(GeoFilePlan::Linked(changes))
}

#[cfg(test)]
mod tests;
