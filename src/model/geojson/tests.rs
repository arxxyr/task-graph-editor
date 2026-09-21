use super::*;
use crate::model::{Orientation, Position, parse_task_graph};

// 合成夹具：沿用上游生成文件的排版（两空格缩进、type/properties/features 顺序、占位点用整数），
// 不含任何真实工厂或网络信息。
const FIXTURE: &str = r#"{
  "type": "FeatureCollection",
  "properties": {
    "map_code": "map-1",
    "map_id": "map-1",
    "source_task_id": "demo_task",
    "workstation_name": "工位1"
  },
  "features": [
    {
      "type": "Feature",
      "id": "WS-01",
      "geometry": {
        "type": "Polygon",
        "coordinates": [[[0, 0], [4, 0], [4, 3], [0, 0]]]
      },
      "properties": {
        "feature_type": "workstation",
        "name": "工位1"
      }
    },
    {
      "type": "Feature",
      "id": "WS-01-pick",
      "geometry": {
        "type": "Point",
        "coordinates": [
          1.5,
          0.25
        ]
      },
      "properties": {
        "feature_type": "nav_point",
        "name": "取料点",
        "orientation": {
          "x": 0.0,
          "y": 0.0,
          "z": 0.6,
          "w": 0.8
        },
        "pose_context_key": "pick_point",
        "source_context_key": "legacy_point"
      }
    },
    {
      "type": "Feature",
      "id": "WS-01-wait",
      "geometry": {
        "type": "Point",
        "coordinates": [
          -1,
          2,
          0.5
        ]
      },
      "properties": {
        "feature_type": "nav_point",
        "name": "等待点",
        "orientation": {
          "x": 0,
          "y": 0,
          "z": 0,
          "w": 1
        },
        "source_context_key": "origin_point"
      }
    },
    {
      "type": "Feature",
      "id": "WS-01-transport-wait",
      "geometry": {
        "type": "Point",
        "coordinates": [
          -1,
          2
        ]
      },
      "properties": {
        "feature_type": "nav_point",
        "name": "搬运等待点",
        "orientation": {
          "x": 0,
          "y": 0,
          "z": 0,
          "w": 1
        },
        "source_context_key": "origin_point"
      }
    },
    {
      "type": "Feature",
      "id": "WS-01-line-raw",
      "geometry": {
        "type": "Point",
        "coordinates": [
          18.326,
          25.025000000000002
        ]
      },
      "properties": {
        "feature_type": "nav_point",
        "name": "line_raw_1",
        "orientation": {
          "x": 0,
          "y": 0,
          "z": 0,
          "w": 1
        }
      }
    }
  ]
}
"#;

fn pose(x: f64, y: f64, qz: f64, qw: f64) -> Pose {
    Pose {
        position: Position { x, y, z: 0.0 },
        orientation: Orientation {
            w: qw,
            x: 0.0,
            y: 0.0,
            z: qz,
        },
    }
}

fn request(poses: &[(&str, Pose)]) -> GeoSyncRequest {
    GeoSyncRequest {
        map_id: "map-1".into(),
        task_id: "demo_task".into(),
        chassis_poses: poses
            .iter()
            .map(|(key, pose)| ((*key).to_owned(), pose.clone()))
            .collect(),
    }
}

/// 与夹具完全一致的三个位姿。
fn consistent() -> Vec<(&'static str, Pose)> {
    vec![
        ("pick_point", pose(1.5, 0.25, 0.6, 0.8)),
        ("origin_point", pose(-1.0, 2.0, 0.0, 1.0)),
    ]
}

fn linked(text: &str, request: &GeoSyncRequest) -> GeoFileChanges {
    match plan_file_sync(text, request).unwrap() {
        GeoFilePlan::Linked(changes) => changes,
        GeoFilePlan::Unrelated => panic!("应识别为当前任务图的导航点文件"),
    }
}

#[test]
fn 已一致时不写入且整数写法不算差异() {
    let changes = linked(FIXTURE, &request(&consistent()));
    assert_eq!(changes.content, None);
    assert!(changes.updated.is_empty());
    assert!(changes.notes.is_empty());
    assert_eq!(changes.linked_points, 3);
}

#[test]
fn 只替换变化的数字其余内容逐字节保留() {
    let mut poses = consistent();
    poses[0].1 = pose(1.75, 0.25, 0.6, 0.8);
    let changes = linked(FIXTURE, &request(&poses));
    assert_eq!(
        changes.content.as_deref(),
        Some(FIXTURE.replacen("1.5,", "1.75,", 1).as_str())
    );
    assert_eq!(
        changes.updated,
        [SyncedPoint {
            feature: "WS-01-pick".into(),
            context_key: "pick_point".into(),
        }]
    );
}

#[test]
fn 同一个键的多个导航点全部更新并保留第三维与未变分量() {
    let mut poses = consistent();
    poses[1].1 = pose(-1.25, 2.0, -0.702795321682847, -0.7113923468630365);
    let changes = linked(FIXTURE, &request(&poses));
    let content = changes.content.unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    for index in [2, 3] {
        let feature = &json["features"][index];
        assert_eq!(feature["geometry"]["coordinates"][0], -1.25);
        assert_eq!(
            feature["properties"]["orientation"]["z"],
            -0.702795321682847
        );
        assert_eq!(
            feature["properties"]["orientation"]["w"],
            -0.7113923468630365
        );
    }
    // 九位以上小数按最短往返形式写出，不丢精度。
    assert!(content.contains("\"z\": -0.702795321682847,\n"));
    assert!(content.contains("\"w\": -0.7113923468630365\n"));
    // y 仍是原来的整数写法，第三维坐标没有被碰。
    assert!(content.contains("          -1.25,\n          2,\n          0.5\n"));
    assert_eq!(json["features"][2]["geometry"]["coordinates"][2], 0.5);
    let updated: Vec<&str> = changes
        .updated
        .iter()
        .map(|point| point.feature.as_str())
        .collect();
    assert_eq!(updated, ["WS-01-wait", "WS-01-transport-wait"]);
    // 没有键的占位点与工位多边形保持原样。
    assert_eq!(json["features"][4]["geometry"]["coordinates"][0], 18.326);
    assert_eq!(
        json["features"][0],
        serde_json::from_str::<serde_json::Value>(FIXTURE).unwrap()["features"][0]
    );
}

#[test]
fn 注入键优先于生成来源键() {
    let mut poses = consistent();
    poses.push(("legacy_point", pose(9.0, 9.0, 0.0, 1.0)));
    assert_eq!(linked(FIXTURE, &request(&poses)).content, None);

    // 旧版文件没有注入键，退回生成来源键。
    let legacy = FIXTURE.replacen("        \"pose_context_key\": \"pick_point\",\n", "", 1);
    poses[2].1 = pose(9.0, 0.25, 0.6, 0.8);
    let content = linked(&legacy, &request(&poses)).content.unwrap();
    assert_eq!(content, legacy.replacen("1.5,", "9.0,", 1));

    // 注入键指向的位姿不存在时不拿来源键顶替，避免把别的位姿写进去。
    let changes = linked(FIXTURE, &request(&poses[1..]));
    assert_eq!(changes.content, None);
    assert_eq!(changes.notes, ["WS-01-pick：任务图没有位姿 pick_point"]);
}

#[test]
fn 其他任务或其他地图的文件不处理() {
    let mut other_task = request(&consistent());
    other_task.task_id = "another_task".into();
    assert_eq!(
        plan_file_sync(FIXTURE, &other_task).unwrap(),
        GeoFilePlan::Unrelated
    );

    let mut other_map = request(&consistent());
    other_map.map_id = "map-2".into();
    assert_eq!(
        plan_file_sync(FIXTURE, &other_map).unwrap(),
        GeoFilePlan::Unrelated
    );

    for text in [
        r#"{"type":"Feature","properties":{"source_task_id":"demo_task"}}"#,
        r#"{"type":"FeatureCollection","properties":{"source_task_id":"demo_task"}}"#,
        r#"{"type":"FeatureCollection","features":[]}"#,
        "[]",
    ] {
        assert_eq!(
            plan_file_sync(text, &request(&consistent())).unwrap(),
            GeoFilePlan::Unrelated,
            "{text}"
        );
    }
}

#[test]
fn 缺少地图标识时按所在目录信任() {
    let text = FIXTURE
        .replace("    \"map_code\": \"map-1\",\n", "")
        .replace("    \"map_id\": \"map-1\",\n", "");
    assert_eq!(linked(&text, &request(&consistent())).linked_points, 3);
}

#[test]
fn 任务图缺少位姿或结构异常时整点不动并说明原因() {
    let only_pick = request(&[("pick_point", pose(3.0, 4.0, 0.6, 0.8))]);
    let changes = linked(FIXTURE, &only_pick);
    assert_eq!(
        changes.notes,
        [
            "WS-01-wait：任务图没有位姿 origin_point",
            "WS-01-transport-wait：任务图没有位姿 origin_point",
        ]
    );
    assert_eq!(changes.updated.len(), 1);

    let broken = FIXTURE
        .replacen("\"type\": \"Point\"", "\"type\": \"LineString\"", 1)
        .replacen("\"w\": 1\n", "\"w\": \"1\"\n", 1);
    let mut poses = consistent();
    poses[0].1 = pose(3.0, 4.0, 0.6, 0.8);
    poses[1].1 = pose(5.0, 6.0, 0.0, 1.0);
    let changes = linked(&broken, &request(&poses));
    assert_eq!(
        changes.notes,
        [
            "WS-01-pick：几何不是 Point",
            "WS-01-wait：坐标或姿态不是数字",
        ]
    );
    // 出问题的点一个数字都不写，其余正常的点照常同步。
    let json: serde_json::Value = serde_json::from_str(&changes.content.unwrap()).unwrap();
    assert_eq!(json["features"][1]["geometry"]["coordinates"][0], 1.5);
    assert_eq!(json["features"][2]["geometry"]["coordinates"][0], -1);
    assert_eq!(json["features"][3]["geometry"]["coordinates"][0], 5.0);
}

#[test]
fn 缺少姿态对象时给出原因() {
    let text = FIXTURE.replacen("\"orientation\": {", "\"heading\": {", 1);
    let mut poses = consistent();
    poses[0].1 = pose(3.0, 4.0, 0.6, 0.8);
    let changes = linked(&text, &request(&poses));
    assert_eq!(changes.notes, ["WS-01-pick：缺少 orientation 的 x/y/z/w"]);
    assert_eq!(changes.content, None);
}

#[test]
fn 数组写法的成员不被当成对象() {
    // serde 派生的结构体也接受按位置排列的数组，这里必须拒绝，否则会替换到错误的位置。
    let text = FIXTURE.replacen(
        "      \"geometry\": {\n        \"type\": \"Point\",\n        \"coordinates\": [\n          1.5,\n          0.25\n        ]\n      },",
        "      \"geometry\": [\"Point\", [1.5, 0.25]],",
        1,
    );
    assert_ne!(text, FIXTURE);
    let mut poses = consistent();
    poses[0].1 = pose(3.0, 4.0, 0.6, 0.8);
    let changes = linked(&text, &request(&poses));
    assert_eq!(changes.notes, ["WS-01-pick：缺少 geometry"]);
    assert_eq!(changes.content, None);
}

#[test]
fn 非有限数值与无效文本被拒绝() {
    let mut poses = consistent();
    poses[0].1 = pose(f64::NAN, 0.25, 0.6, 0.8);
    assert_eq!(
        plan_file_sync(FIXTURE, &request(&poses)),
        Err(GeoSyncError::NonFinite {
            key: "pick_point".into()
        })
    );
    poses[0].1 = pose(1.5, 0.25, f64::INFINITY, 0.8);
    assert!(plan_file_sync(FIXTURE, &request(&poses)).is_err());
    assert!(matches!(
        plan_file_sync("{ 不是 JSON", &request(&consistent())),
        Err(GeoSyncError::Parse(_))
    ));
}

#[test]
fn 标识缺失时退回名称和下标() {
    let text = FIXTURE
        .replacen("      \"id\": \"WS-01-pick\",\n", "", 1)
        .replacen("      \"id\": \"WS-01-wait\",\n", "      \"id\": 7,\n", 1)
        .replacen("      \"id\": \"WS-01-transport-wait\",\n", "", 1)
        .replacen("        \"name\": \"搬运等待点\",\n", "", 1);
    let changes = linked(
        &text,
        &request(&[
            ("pick_point", pose(2.0, 0.25, 0.6, 0.8)),
            ("origin_point", pose(-2.0, 2.0, 0.0, 1.0)),
        ]),
    );
    let labels: Vec<&str> = changes
        .updated
        .iter()
        .map(|point| point.feature.as_str())
        .collect();
    assert_eq!(labels, ["取料点", "7", "features[3]"]);
}

#[test]
fn 请求只收集顶层位姿字段的底盘位姿() {
    let pose_text = "{\\\"chassis_pose\\\":{\\\"position\\\":{\\\"x\\\":1.0,\\\"y\\\":2.0,\\\"z\\\":0.0},\\\"orientation\\\":{\\\"w\\\":0.8,\\\"x\\\":0.0,\\\"y\\\":0.0,\\\"z\\\":0.6}},\\\"head_pose\\\":{\\\"position\\\":{\\\"x\\\":0.0,\\\"y\\\":0.0,\\\"z\\\":0.0},\\\"orientation\\\":{\\\"w\\\":1.0,\\\"x\\\":0.0,\\\"y\\\":0.0,\\\"z\\\":0.0}},\\\"waist_pose\\\":{\\\"position\\\":{\\\"x\\\":0.5,\\\"y\\\":0.3,\\\"z\\\":0.0},\\\"orientation\\\":{\\\"w\\\":1.0,\\\"x\\\":0.0,\\\"y\\\":0.0,\\\"z\\\":0.0}}}";
    let text = format!(
        r#"{{"map_id":"map-1","task_id":"demo_task","config":{{"context":{{
            "pick_point":"{pose_text}","speed":1.5,"stations":{{"inner_pose":"{pose_text}"}}
        }}}}}}"#
    );
    let data = parse_task_graph(&text).unwrap();
    let request = GeoSyncRequest::from_task_graph(&data).unwrap();
    assert_eq!(request.map_id, "map-1");
    assert_eq!(request.task_id, "demo_task");
    assert_eq!(
        request.chassis_poses,
        [("pick_point".to_owned(), pose(1.0, 2.0, 0.6, 0.8))]
    );

    let without_pose =
        parse_task_graph(r#"{"map_id":"m","task_id":"t","config":{"context":{"speed":1}}}"#)
            .unwrap();
    assert_eq!(GeoSyncRequest::from_task_graph(&without_pose), None);
}

#[test]
fn 汇总文案区分已同步已一致失败与未同步() {
    assert_eq!(GeoSyncReport::default().status_text(), None);

    let mut report = GeoSyncReport {
        matched_files: 2,
        ..Default::default()
    };
    assert_eq!(report.status_text().as_deref(), Some("地图点位已一致"));

    report.updated_files = vec!["a.geojson".into(), "b.geojson".into()];
    for feature in ["WS-01-wait", "WS-01-transport-wait"] {
        report.updated.insert(SyncedPoint {
            feature: feature.into(),
            context_key: "origin_point".into(),
        });
    }
    assert_eq!(
        report.status_text().as_deref(),
        Some("已同步 2 个地图点位到 2 个 GeoJSON：origin_point")
    );

    report.errors.push("b.geojson：权限不足".into());
    report.warnings.push("目录持久化确认失败".into());
    report.notes.insert("WS-01-pick：几何不是 Point".into());
    let text = report.status_text().unwrap();
    assert!(text.contains("地图点位同步失败：b.geojson：权限不足"));
    assert!(text.contains("地图点位警告：目录持久化确认失败"));
    assert!(text.contains("未同步：WS-01-pick：几何不是 Point"));

    let failed = GeoSyncReport {
        matched_files: 1,
        errors: vec!["读取失败".into()],
        ..Default::default()
    };
    assert_eq!(
        failed.status_text().as_deref(),
        Some("地图点位同步失败：读取失败")
    );
}

/// 用真实样例核对：`TASK_GRAPH_REAL_FILE=<任务图副本> TGE_GEOJSON_FILE=<geojson 副本>
/// cargo test 真实地图点位与任务图一致且改动最小 -- --ignored --nocapture`，只读本地副本。
#[test]
#[ignore = "需要本地的真实任务图与 GeoJSON 副本"]
fn 真实地图点位与任务图一致且改动最小() {
    let read = |name: &str| {
        let path = std::env::var(name).unwrap_or_else(|_| panic!("请设置 {name}"));
        std::fs::read_to_string(path).unwrap()
    };
    let data = parse_task_graph(&read("TASK_GRAPH_REAL_FILE")).unwrap();
    let text = read("TGE_GEOJSON_FILE");
    let mut request = GeoSyncRequest::from_task_graph(&data).unwrap();
    let changes = linked(&text, &request);
    println!(
        "关联导航点 {} 个；需更新 {:?}；未同步 {:?}",
        changes.linked_points, changes.updated, changes.notes
    );

    // 改动第一个被关联的位姿，确认只有对应数字发生变化。
    let first = &mut request.chassis_poses[0];
    first.1.position.x += 0.125;
    let (key, x) = (first.0.clone(), first.1.position.x);
    let before: serde_json::Value = serde_json::from_str(&text).unwrap();
    let Some(content) = linked(&text, &request).content else {
        println!("位姿 {key} 没有关联的导航点");
        return;
    };
    let after: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(after["properties"], before["properties"]);
    let changed_lines = text
        .lines()
        .zip(content.lines())
        .filter(|(old, new)| old != new)
        .count();
    println!("位姿 {key} 的 x 改为 {x}：{changed_lines} 行发生变化");
    assert_eq!(text.lines().count(), content.lines().count());
}
