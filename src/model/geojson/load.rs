//! 打开任务图时用地图导航点覆盖显示值，原始 JSON 保留为保存合并基线。

use std::collections::BTreeMap;

use super::*;

/// 先验证全部文件，再一次性更新；冲突或损坏时不留下部分覆盖的点位。
pub fn apply_geojson_points(
    data: &mut TaskGraphData,
    files: &[(String, String)],
) -> Result<usize, String> {
    let Some(request) = GeoSyncRequest::from_task_graph(data) else {
        return Ok(0);
    };
    let mut points = BTreeMap::<String, (Pose, String)>::new();
    for (name, text) in files {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|error| format!("{name}：地图解析失败：{error}"))?;
        let properties = &value["properties"];
        if value["type"].as_str() != Some("FeatureCollection")
            || properties["source_task_id"].as_str() != Some(&request.task_id)
            || ["map_id", "map_code"].into_iter().any(|key| {
                properties[key]
                    .as_str()
                    .is_some_and(|map| map != request.map_id)
            })
        {
            continue;
        }
        let features = value["features"]
            .as_array()
            .ok_or_else(|| format!("{name}：地图缺少 features 数组"))?;
        for feature in features {
            let properties = &feature["properties"];
            let Some(key) = properties["pose_context_key"]
                .as_str()
                .or_else(|| properties["source_context_key"].as_str())
            else {
                continue;
            };
            let Some(original) = request.chassis_pose(key) else {
                continue;
            };
            let invalid = || format!("{name}：导航点 {key} 的坐标或姿态无效，已停止加载");
            if feature["geometry"]["type"].as_str() != Some("Point") {
                return Err(invalid());
            }
            let coordinates = feature["geometry"]["coordinates"]
                .as_array()
                .ok_or_else(invalid)?;
            let number = |value: Option<&serde_json::Value>| {
                value
                    .and_then(serde_json::Value::as_f64)
                    .filter(|n| n.is_finite())
                    .ok_or_else(invalid)
            };
            // 地图同步协议只覆盖平面 x/y 和四元数；底盘高度仍保留任务图的值。
            let mut pose = original.clone();
            pose.position.x = number(coordinates.first())?;
            pose.position.y = number(coordinates.get(1))?;
            let orientation = &properties["orientation"];
            pose.orientation.x = number(orientation.get("x"))?;
            pose.orientation.y = number(orientation.get("y"))?;
            pose.orientation.z = number(orientation.get("z"))?;
            pose.orientation.w = number(orientation.get("w"))?;
            if let Some((previous, source)) = points.get(key)
                && previous != &pose
            {
                return Err(format!(
                    "地图点位 {key} 存在冲突（{source}、{name}），已停止加载"
                ));
            }
            points.insert(key.to_owned(), (pose, name.clone()));
        }
    }
    for field in &mut data.context_fields {
        if let ContextValue::Pose(robot) = &mut field.value
            && let Some((pose, _)) = points.get(&field.key)
        {
            robot.chassis_pose = pose.clone();
        }
    }
    Ok(points.len())
}
