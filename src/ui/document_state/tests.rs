//! 保存状态回归只操作内存文档，不连接 SSH 或访问使用者配置。

use super::*;
use crate::model::{ContextValue, graph_edit::GraphCommand, parse_task_graph};
use serde_json::json;

fn document() -> Editor {
    let data = parse_task_graph(
        &json!({
            "map_id": "map", "task_id": "task",
            "config": {
                "context": {"speed": 1.5, "enabled": false},
                "nodes": [{"id": "log", "type": "log", "inputs": {"message": "初始"}}],
                "edges": [{"from": "_entry", "to": "log"}, {"from": "log", "to": "_exit"}]
            }
        })
        .to_string(),
    )
    .unwrap();
    let mut editor = Editor::default();
    editor.load_remote(
        data,
        RemoteDocument {
            connection_generation: 7,
            remote_dir: "/fixture/graphs".into(),
            filename: "task.json".into(),
        },
    );
    editor
}

fn change_graph(editor: &mut Editor, message: &str) {
    let data = editor.data.as_mut().unwrap();
    let graph = data.graph_edit.graph_key_at(&[]).unwrap();
    let node = data.graph_edit.node_key(graph, "log").unwrap();
    data.apply_graph_command(
        data.graph_edit.revision(),
        GraphCommand::SetNodeProperty {
            node,
            property: "inputs".into(),
            value: Some(json!({"message": message})),
        },
    )
    .unwrap();
}

fn change_speed(editor: &mut Editor, value: f64) {
    editor
        .data
        .as_mut()
        .unwrap()
        .context_fields
        .iter_mut()
        .find(|field| field.key == "speed")
        .unwrap()
        .value = ContextValue::Float(value);
}

fn begin(editor: &mut Editor) -> SaveTicket {
    let ticket = editor.next_save_ticket();
    assert!(editor.begin_save(ticket));
    ticket
}

#[test]
fn 图上下文和两个元数据独立参与未保存状态() {
    let mut editor = document();
    assert!(!editor.has_unsaved_changes());
    editor.data.as_mut().unwrap().map_id = "another_map".into();
    assert!(editor.has_unsaved_changes());
    editor.data.as_mut().unwrap().map_id = "map".into();
    assert!(!editor.has_unsaved_changes());
    editor.data.as_mut().unwrap().task_id = "another_task".into();
    assert!(editor.has_unsaved_changes());
    editor.data.as_mut().unwrap().task_id = "task".into();
    assert!(!editor.has_unsaved_changes());
    change_speed(&mut editor, 2.75);
    assert!(editor.has_unsaved_changes());
    change_speed(&mut editor, 1.5);
    assert!(!editor.has_unsaved_changes());
    change_graph(&mut editor, "修改流程");
    assert!(editor.has_unsaved_changes());
    let data = editor.data.as_mut().unwrap();
    data.undo_graph(data.graph_edit.revision()).unwrap();
    assert!(!editor.has_unsaved_changes());
}

#[test]
fn 保存成功只确认提交快照而不清除保存期间四类新修改() {
    for field in ["graph", "context", "task_id", "map_id"] {
        let mut editor = document();
        change_graph(&mut editor, "已提交");
        let submitted = DocumentSnapshot::capture(editor.data.as_ref().unwrap());
        let ticket = begin(&mut editor);
        match field {
            "graph" => change_graph(&mut editor, "提交之后"),
            "context" => change_speed(&mut editor, 9.25),
            "task_id" => editor.data.as_mut().unwrap().task_id = "later_task".into(),
            "map_id" => editor.data.as_mut().unwrap().map_id = "later_map".into(),
            _ => unreachable!(),
        }
        assert!(editor.confirm_save(ticket));
        assert!(editor.has_unsaved_changes(), "{field} 的后续修改不能被清除");
        let saved = editor.saved_snapshot.as_ref().unwrap();
        assert_eq!(saved.map_id, submitted.map_id);
        assert_eq!(saved.task_id, submitted.task_id);
        assert_eq!(saved.context, submitted.context);
        assert_eq!(saved.graph, submitted.graph);
        assert!(editor.pending_save.is_none());
    }
}

#[test]
fn 失败保留历史和未保存内容且下一次重试可独立确认() {
    let mut editor = document();
    change_graph(&mut editor, "等待保存");
    change_speed(&mut editor, 3.5);
    let old_ticket = begin(&mut editor);
    assert!(editor.fail_save(old_ticket));
    assert!(editor.has_unsaved_changes());
    assert!(editor.data.as_ref().unwrap().graph_edit.can_undo());
    assert!(editor.pending_save.is_none());
    let retry = begin(&mut editor);
    assert_eq!(retry.document_version, old_ticket.document_version);
    assert!(retry.sequence > old_ticket.sequence);
    assert!(!editor.confirm_save(old_ticket));
    assert!(!editor.fail_save(old_ticket));
    assert!(editor.pending_save.is_some());
    assert!(editor.confirm_save(retry));
    assert!(!editor.has_unsaved_changes());
    assert!(!editor.confirm_save(retry));
}

#[test]
fn 旧保存回执不能确认重载文档或清掉新请求() {
    let mut editor = document();
    change_graph(&mut editor, "旧文档修改");
    let old = begin(&mut editor);
    let other = document();
    let mut source = other.document.unwrap();
    source.connection_generation = 9;
    source.remote_dir = "/fixture/another".into();
    source.filename = "another.json".into();
    editor.load_remote(other.data.unwrap(), source.clone());
    change_graph(&mut editor, "新文档修改");
    let current = begin(&mut editor);
    assert_ne!(old.document_version, current.document_version);
    assert!(current.sequence > old.sequence);
    assert!(!editor.confirm_save(old));
    assert!(!editor.fail_save(old));
    assert!(editor.has_unsaved_changes());
    assert_eq!(editor.document.as_ref(), Some(&source));
    assert_eq!(editor.pending_save.as_ref().unwrap().ticket, current);
    assert!(editor.confirm_save(current));
    assert!(!editor.has_unsaved_changes());
}

#[test]
fn 相同文件名重载也使旧回执失效() {
    let mut editor = document();
    change_graph(&mut editor, "旧内容");
    let old = begin(&mut editor);
    let same = document();
    editor.load_remote(same.data.unwrap(), same.document.unwrap());
    change_speed(&mut editor, 8.0);
    assert!(!editor.confirm_save(old));
    assert!(editor.has_unsaved_changes());
    assert_eq!(editor.document.as_ref().unwrap().filename, "task.json");
}

#[test]
fn 来源变化且代次不变时仍拒绝保存回执() {
    for kind in ["connection", "directory", "filename"] {
        let mut editor = document();
        change_graph(&mut editor, "未保存");
        let ticket = begin(&mut editor);
        let source = editor.document.as_mut().unwrap();
        match kind {
            "connection" => source.connection_generation += 1,
            "directory" => source.remote_dir = "/fixture/new".into(),
            "filename" => source.filename = "new.json".into(),
            _ => unreachable!(),
        }
        let expected = source.clone();
        assert!(!editor.confirm_save(ticket));
        assert!(editor.has_unsaved_changes());
        assert_eq!(editor.document.as_ref(), Some(&expected));
    }
}

#[test]
fn 撤销回保存内容清标记但不撤销上下文或实际文件改名() {
    let mut editor = document();
    change_graph(&mut editor, "保存点");
    change_speed(&mut editor, 4.5);
    editor.data.as_mut().unwrap().task_id = "renamed".into();
    let ticket = begin(&mut editor);
    assert!(editor.confirm_save(ticket));
    editor.document.as_mut().unwrap().filename = "renamed.json".into();
    let source = editor.document.clone();
    assert!(!editor.has_unsaved_changes());
    change_graph(&mut editor, "保存点之后");
    let data = editor.data.as_mut().unwrap();
    data.undo_graph(data.graph_edit.revision()).unwrap();
    assert!(!editor.has_unsaved_changes());
    let data = editor.data.as_mut().unwrap();
    data.undo_graph(data.graph_edit.revision()).unwrap();
    assert!(editor.has_unsaved_changes());
    assert_eq!(editor.document, source);
    let data = editor.data.as_mut().unwrap();
    assert_eq!(data.task_id, "renamed");
    assert_eq!(
        data.context_fields
            .iter()
            .find(|field| field.key == "speed")
            .unwrap()
            .value,
        ContextValue::Float(4.5)
    );
    data.redo_graph(data.graph_edit.revision()).unwrap();
    assert!(!editor.has_unsaved_changes());
}

#[test]
fn 无来源清空和非法票据不能开始保存或残留脏状态() {
    let mut editor = document();
    let valid = editor.next_save_ticket();
    assert!(!editor.begin_save(SaveTicket {
        sequence: valid.sequence + 1,
        ..valid
    }));
    assert!(!editor.begin_save(SaveTicket {
        document_version: valid.document_version + 1,
        ..valid
    }));
    assert_eq!(editor.next_save_ticket(), valid);
    editor.document = None;
    assert!(!editor.begin_save(valid));
    editor.load(None);
    assert!(!editor.has_unsaved_changes());
    assert!(!editor.begin_save(editor.next_save_ticket()));
    assert!(!editor.confirm_save(valid));
    assert!(editor.pending_save.is_none());
}

#[test]
fn 异步切换比较发起内容而非保存点所以已确认放弃的旧修改仍可切换() {
    let mut editor = document();
    change_graph(&mut editor, "已确认放弃的旧草稿");
    change_speed(&mut editor, 8.0);
    assert!(editor.has_unsaved_changes());
    let input = DocumentInputSnapshot::default();
    let ticket = editor.begin_read(7, input.clone());
    assert_eq!(editor.finish_read(ticket, 7, &input), Some(true));
    assert!(editor.pending_read.is_none());
}

#[test]
fn 异步切换期间所有可写内容和来源的新变化都会保留() {
    for changed in ["graph", "context", "task_id", "map_id", "source", "input"] {
        let mut editor = document();
        let mut input = DocumentInputSnapshot::default();
        let ticket = editor.begin_read(7, input.clone());
        match changed {
            "graph" => change_graph(&mut editor, "等待期间修改"),
            "context" => change_speed(&mut editor, 9.0),
            "task_id" => editor.data.as_mut().unwrap().task_id = "later".into(),
            "map_id" => editor.data.as_mut().unwrap().map_id = "later".into(),
            "source" => editor.document.as_mut().unwrap().remote_dir = "/later".into(),
            "input" => input.invalid_revision += 1,
            _ => unreachable!(),
        }
        let before = editor.data.as_ref().map(DocumentSnapshot::capture);
        assert_eq!(
            editor.finish_read(ticket, 7, &input),
            Some(false),
            "{changed}"
        );
        assert!(editor.pending_read.is_none());
        assert!(before == editor.data.as_ref().map(DocumentSnapshot::capture));
    }
}

#[test]
fn 异步切换票据在同名重载后不复用且旧回执不消费新请求() {
    let mut editor = document();
    let input = DocumentInputSnapshot::default();
    let old = editor.begin_read(7, input.clone());
    let reloaded = document();
    editor.load_remote(reloaded.data.unwrap(), reloaded.document.unwrap());
    let new = editor.begin_read(7, input.clone());
    assert_ne!(old.document_version, new.document_version);
    assert!(new.sequence > old.sequence);
    assert_eq!(editor.finish_read(old, 7, &input), None);
    assert_eq!(editor.pending_read.as_ref().unwrap().ticket, new);
    assert_eq!(editor.finish_read(new, 7, &input), Some(true));
}

#[test]
fn 异步切换连接代次改变拒绝回执但完成原请求清理() {
    let mut editor = document();
    let input = DocumentInputSnapshot::default();
    let ticket = editor.begin_read(7, input.clone());
    assert_eq!(editor.finish_read(ticket, 8, &input), Some(false));
    assert!(editor.pending_read.is_none());
    assert!(editor.data.is_some());
}
