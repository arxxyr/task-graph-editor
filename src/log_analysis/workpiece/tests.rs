use super::*;

fn line(second: u32, body: &str) -> String {
    format!(
        "2026-09-10 10:{:02}:{:02}.000000 [master_control] [0x1] [INFO] [test.cpp:1] - {body}\n",
        second / 60,
        second % 60
    )
}
fn success(second: u32, node: &str) -> String {
    line(
        second,
        &format!(
            "BehaviorTreeNode {node}: 已发布任务日志 [INFO] BehaviorTree 执行成功 - error_code: 0"
        ),
    )
}
fn cycle(start: u32, station: u8) -> String {
    [
        line(start, ANCHOR),
        success(start + 10, "normal_arm_pick_merged"),
        success(
            start + 20,
            &format!("normal_arm_leak_grab_and_place_station_{station}_merged"),
        ),
        success(start + 30, "normal_arm_put_merged"),
    ]
    .concat()
}
fn events(text: &str) -> Vec<Event> {
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| classify(line, i + 1, timestamp(line).unwrap()))
        .collect()
}
fn calculate_text(text: &str, options: &Options, statistics: StatisticsOptions) -> Vec<Cycle> {
    let mut events = events(text);
    let end = events
        .iter()
        .map(|e| e.evidence().timestamp_us)
        .max()
        .unwrap();
    process_cycles(
        &mut events,
        end,
        "P1.1",
        std::path::Path::new("fixture.log"),
        options,
        statistics,
    )
}

#[test]
fn 净周期按等待并集扣除且四阶段精确加总() {
    let text = cycle(0, 1) + &line(5, WAIT_START) + &line(15, WAIT_END) + &line(60, ANCHOR);
    let rows = calculate_text(&text, &Options::default(), StatisticsOptions::default());
    let c = &rows[0];
    assert!(c.included);
    assert_eq!(c.raw_us, Some(60_000_000));
    assert_eq!(c.wait_us, 10_000_000);
    assert_eq!(c.net_us, Some(50_000_000));
    assert_eq!(c.stages_us, [5_000_000, 5_000_000, 10_000_000, 30_000_000]);
    assert_eq!(c.stage_wait_us, [5_000_000, 5_000_000, 0, 0]);
    assert!(!rows[1].included);
    let no_deduct = calculate_text(
        &text,
        &Options {
            deduct_wait: false,
            ..Default::default()
        },
        StatisticsOptions::default(),
    );
    assert_eq!(no_deduct[0].net_us, Some(60_000_000));
}
#[test]
fn 暂停跨周期且重复标记不重复计时() {
    let text = cycle(0, 1)
        + &line(50, "TaskGraphExecutor: 用户请求暂停任务 task")
        + &line(55, "TaskGraphExecutor: 用户请求暂停任务 task")
        + &cycle(60, 2)
        + &line(65, "TaskGraphExecutor: 恢复任务 task")
        + &line(120, ANCHOR);
    let rows = calculate_text(&text, &Options::default(), StatisticsOptions::default());
    assert!(!rows[0].included);
    assert!(!rows[1].included);
    assert_eq!(rows[1].pauses.len(), 1);
    assert_eq!(
        rows[1].pauses[0].observed_end_us - rows[1].pauses[0].start.timestamp_us,
        15_000_000
    );
    let included = calculate_text(
        &text,
        &Options::default(),
        StatisticsOptions {
            include_paused: true,
            include_errors: false,
        },
    );
    assert!(included[0].included && included[1].included);
}
#[test]
fn 执行失败不靠检测文字或零错误码猜测() {
    for body in [
        "BehaviorTree 执行成功 - error_code: 0",
        "气密检测未通过，检测失败",
        "arm_move response: result code=0 msg=成功",
        "业务说明：[ERROR] 是日志级别",
    ] {
        assert!(
            !classify(&line(0, body), 1, timestamp(&line(0, body)).unwrap())
                .is_some_and(|e| e.error),
            "{body}"
        );
    }
    for body in [
        "arm_move response: result code=13020003 msg=轨迹执行失败",
        "BehaviorTreeNode a: 执行失败 - error_code: 1",
        "ROS2ActionNode[a] 结果: FAILURE",
        "ROS2ActionNode[a]: 已发布任务日志 [ERROR] 失败(code=1)",
    ] {
        assert!(
            classify(&line(0, body), 1, timestamp(&line(0, body)).unwrap())
                .unwrap()
                .error,
            "{body}"
        );
    }
    for severity in ["ERROR", "FATAL", "CRITICAL"] {
        let text = line(0, "主控执行故障").replacen("[INFO]", &format!("[{severity}]"), 1);
        assert!(classify(&text, 1, timestamp(&text).unwrap()).unwrap().error);
    }
}
#[test]
fn 错误开关与恢复轮重试归属独立() {
    let text = cycle(0, 1)
        + &line(59, "TaskGraphExecutor: 重试节点 normal_nav_pick")
        + &cycle(60, 2)
        + &line(75, "arm_move response: result code=10")
        + &line(120, ANCHOR);
    let rows = calculate_text(&text, &Options::default(), StatisticsOptions::default());
    assert_eq!(rows[1].interventions.len(), 1);
    assert_eq!(rows[1].errors.len(), 1);
    assert_eq!(rows[1].reasons.len(), 2);
    let rows = calculate_text(
        &text,
        &Options {
            include_interventions: true,
            ..Default::default()
        },
        StatisticsOptions {
            include_errors: true,
            include_paused: true,
        },
    );
    assert!(rows[1].included);
}
#[test]
fn 边界错误归下一轮且成功重复不能强行纳入() {
    let text = cycle(0, 1)
        + &line(60, "arm_move response: result code=5")
        + &cycle(60, 2)
        + &success(100, "normal_arm_put_merged")
        + &line(120, ANCHOR);
    let rows = calculate_text(
        &text,
        &Options {
            include_interventions: true,
            ..Default::default()
        },
        StatisticsOptions {
            include_errors: true,
            include_paused: true,
        },
    );
    assert!(rows[0].errors.is_empty());
    assert_eq!(rows[1].errors.len(), 1);
    assert!(!rows[1].included);
    assert!(!rows[1].structural_reasons.is_empty());
}
#[test]
fn 等待重复线程不匹配或取消均不能猜测扣除() {
    for extra in [
        line(5, WAIT_START) + &line(7, WAIT_START) + &line(15, WAIT_END),
        line(5, WAIT_START) + &line(15, WAIT_END).replace("[0x1]", "[0x2]"),
        line(5, WAIT_START) + &line(9, "TaskGraphExecutor: 取消执行 task") + &line(15, WAIT_END),
    ] {
        let text = cycle(0, 1) + &extra + &line(60, ANCHOR);
        let rows = calculate_text(&text, &Options::default(), StatisticsOptions::default());
        assert_eq!(rows[0].wait_us, 0);
        assert!(rows[0].waits.iter().all(|w| !w.reliable));
    }
}
#[test]
fn 整数微秒不受本地时区影响() {
    let ts = timestamp("2026-09-10 00:00:00.123456 [master_control] [INFO] test").unwrap();
    assert_eq!(beijing(ts), "2026-09-10 00:00:00.123456");
    assert_eq!(
        timestamp(&format!(
            "[INFO] [{}.{:06}] test",
            ts / 1_000_000,
            ts % 1_000_000
        )),
        Some(ts)
    );
    let text = cycle(0, 1).replace("2026-09-10 10:", "2026-09-09 23:")
        + &line(0, ANCHOR).replace("10:", "00:");
    let rows = calculate_text(
        &text,
        &Options {
            date: "2026-09-10".into(),
            ..Default::default()
        },
        StatisticsOptions::default(),
    );
    assert_eq!(rows.len(), 1);
    assert!(rows[0].end.is_none());
}
#[test]
fn 进程重启与末尾半行不形成伪周期并按源去重() {
    let root = super::super::create_run(&std::env::temp_dir()).unwrap();
    let path = root.join("one.log");
    let copy = root.join("copy.log");
    let text = format!(
        "=== SPR Logger Started at test\n=== Log file: /logs/master_control_1.log\n{}=== SPR Logger Started at next\n{}{}",
        cycle(0, 1),
        cycle(60, 2),
        line(120, ANCHOR).trim_end()
    );
    fs::write(&path, &text).unwrap();
    fs::write(&copy, &text).unwrap();
    let request = AnalysisRequest {
        inputs: vec![path, copy],
        output: root.clone(),
        statistics: StatisticsOptions::default(),
        workpiece: None,
    };
    let data = calculate(&request, &Options::default()).unwrap();
    assert_eq!(data.sources.len(), 1);
    assert_eq!(data.duplicates.len(), 1);
    assert_eq!(data.cycles.len(), 2);
    assert!(data.cycles.iter().all(|c| c.end.is_none()));
    assert!(data.sources[0].ignored_tail);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn 重复源只选完整副本且冲突报错() {
    let root = super::super::create_run(&std::env::temp_dir()).unwrap();
    let a = root.join("a.log");
    let b = root.join("b.log");
    let header = "=== SPR Logger Started at test\n=== Log file: /logs/one.log\n";
    fs::write(&a, format!("{header}{}", cycle(0, 1))).unwrap();
    fs::write(&b, format!("{header}{}{}", cycle(0, 1), line(60, ANCHOR))).unwrap();
    let (sources, duplicates) = read_sources(&[a.clone(), b.clone()]).unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].0, b);
    assert_eq!(duplicates.len(), 1);
    fs::write(&b, format!("{header}{}", cycle(0, 2))).unwrap();
    assert!(read_sources(&[a, b]).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn 交互报告保留排除证据且转义日志中的脚本标签() {
    let root = super::super::create_run(&std::env::temp_dir()).unwrap();
    let input = root.join("fixture.log");
    fs::write(
        &input,
        cycle(0, 1)
            + &line(
                5,
                "arm_move response: result code=1 </script><script>alert(1)</script>",
            )
            + &line(60, ANCHOR),
    )
    .unwrap();
    let request = AnalysisRequest {
        inputs: vec![input],
        output: root.clone(),
        statistics: StatisticsOptions::default(),
        workpiece: Some(Options::default()),
    };
    let result = super::super::analyze(&request).unwrap();
    assert_eq!(result.reports[0].files.len(), 4);
    let data: Dataset =
        serde_json::from_slice(&fs::read(root.join("workpiece/cycles.json")).unwrap()).unwrap();
    assert!(!data.cycles[0].included);
    assert!(data.cycles[0].errors[0].text.contains("</script>"));
    assert!(!data.cycles[1].included);
    let html = fs::read_to_string(root.join("workpiece/interactive.html")).unwrap();
    assert!(!html.contains("/*DATA*/ null"));
    assert!(!html.contains("</script><script>alert"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn 重叠线程等待取并集且跨轮只扣各自交集() {
    let waits = line(5, WAIT_START)
        + &line(25, WAIT_END)
        + &line(10, WAIT_START).replace("[0x1]", "[0x2]")
        + &line(30, WAIT_END).replace("[0x1]", "[0x2]");
    let rows = calculate_text(
        &(cycle(0, 1) + &waits + &line(60, ANCHOR)),
        &Options::default(),
        StatisticsOptions::default(),
    );
    assert_eq!(rows[0].wait_us, 25_000_000);
    assert_eq!(rows[0].net_us, Some(35_000_000));
    let rows = calculate_text(
        &(cycle(0, 1)
            + &line(55, WAIT_START)
            + &cycle(60, 2)
            + &line(65, WAIT_END)
            + &line(120, ANCHOR)),
        &Options::default(),
        StatisticsOptions::default(),
    );
    assert_eq!(rows[0].wait_us, 5_000_000);
    assert_eq!(rows[1].wait_us, 5_000_000);
}

#[test]
fn 模板不得生成虚假事件且错误消息可以带引号() {
    let template = line(
        0,
        "LogNode[test] - 初始化完成: level='error', message='用户请求暂停任务，执行失败'",
    );
    assert!(classify(&template, 1, timestamp(&template).unwrap()).is_none());
    let error = line(0, "arm_move response: result code=10 message='失败'");
    assert!(
        classify(&error, 1, timestamp(&error).unwrap())
            .unwrap()
            .error
    );
    let no_location = "2026-09-10 10:00:00.000000 [master_control] [ERROR] 硬件故障";
    assert!(
        classify(no_location, 1, timestamp(no_location).unwrap())
            .unwrap()
            .error
    );
}
