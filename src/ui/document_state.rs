//! 已保存内容快照与请求回执绑定，图历史不回滚实际远端文件身份。

use serde_json::Value;

use crate::model::{ContextField, TaskGraphData};
use crate::worker::{DocumentReadTicket, SaveTicket};

use super::{Editor, RemoteDocument};

#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq)]
pub(super) struct DocumentSnapshot {
    map_id: String,
    task_id: String,
    context: Vec<ContextField>,
    graph: [Option<Value>; 3],
}

impl DocumentSnapshot {
    pub(super) fn capture(data: &TaskGraphData) -> Self {
        Self {
            map_id: data.map_id.clone(),
            task_id: data.task_id.clone(),
            context: data.context_fields.clone(),
            graph: ["nodes", "edges", "entry_point"]
                .map(|key| data.graph_edit.current_config().get(key).cloned()),
        }
    }

    fn matches(&self, data: &TaskGraphData) -> bool {
        self.map_id == data.map_id
            && self.task_id == data.task_id
            && self.context == data.context_fields
            && ["nodes", "edges", "entry_point"]
                .into_iter()
                .zip(&self.graph)
                .all(|(key, saved)| data.graph_edit.current_config().get(key) == saved.as_ref())
    }
}

pub(super) struct PendingSave {
    ticket: SaveTicket,
    source: RemoteDocument,
    content: DocumentSnapshot,
}

/// 未提交文本不在 TaskGraphData 中，必须连同图草稿和非法数字文本的代次一起保护。
#[derive(Clone, Default, PartialEq)]
pub(super) struct DocumentInputSnapshot {
    graph: super::graph_edit::GraphDraftSnapshot,
    invalid_revision: u64,
    pending_input: bool,
}

impl DocumentInputSnapshot {
    pub(super) fn capture(
        graph: Option<&super::graph_edit::GraphEditing>,
        validation: &super::editor::InputValidation,
    ) -> Self {
        Self {
            graph: graph.map_or_else(Default::default, |graph| graph.draft_snapshot()),
            invalid_revision: validation.invalid_revision(),
            pending_input: false,
        }
    }

    pub(super) fn with_pending_input(mut self, pending: bool) -> Self {
        self.pending_input = pending;
        self
    }
}

pub(super) struct PendingDocumentRead {
    ticket: DocumentReadTicket,
    source: Option<RemoteDocument>,
    content: Option<DocumentSnapshot>,
    input: DocumentInputSnapshot,
}

impl Editor {
    pub(super) fn begin_read(
        &mut self,
        connection_generation: u64,
        input: DocumentInputSnapshot,
    ) -> DocumentReadTicket {
        self.read_sequence += 1;
        let ticket = DocumentReadTicket {
            document_version: self.document_version,
            connection_generation,
            sequence: self.read_sequence,
        };
        self.pending_read = Some(PendingDocumentRead {
            ticket,
            source: self.document.clone(),
            content: self.data.as_ref().map(DocumentSnapshot::capture),
            input,
        });
        ticket
    }

    /// None 是不属于当前请求的回执，不能清掉新请求的忙碌态；Some(false) 保留新编辑。
    pub(super) fn finish_read(
        &mut self,
        ticket: DocumentReadTicket,
        connection_generation: u64,
        input: &DocumentInputSnapshot,
    ) -> Option<bool> {
        if !self
            .pending_read
            .as_ref()
            .is_some_and(|pending| pending.ticket == ticket)
        {
            return None;
        }
        let pending = self.pending_read.take()?;
        Some(
            ticket.document_version == self.document_version
                && ticket.connection_generation == connection_generation
                && self.document == pending.source
                && pending.content == self.data.as_ref().map(DocumentSnapshot::capture)
                && pending.input == *input,
        )
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.data.as_ref().is_some_and(|data| {
            self.saved_snapshot
                .as_ref()
                .is_none_or(|saved| !saved.matches(data))
        })
    }

    pub(super) fn next_save_ticket(&self) -> SaveTicket {
        SaveTicket {
            document_version: self.document_version,
            sequence: self.save_sequence + 1,
        }
    }

    pub(super) fn begin_save(&mut self, ticket: SaveTicket) -> bool {
        let (Some(data), Some(source)) = (&self.data, &self.document) else {
            return false;
        };
        if ticket != self.next_save_ticket() {
            return false;
        }
        self.save_sequence = ticket.sequence;
        self.pending_save = Some(PendingSave {
            ticket,
            source: source.clone(),
            content: DocumentSnapshot::capture(data),
        });
        true
    }

    /// 先校验来源和代次，再确认当时提交的内容；保存期间的新修改继续保持未保存。
    pub(super) fn confirm_save(&mut self, ticket: SaveTicket) -> bool {
        let valid = self.pending_save.as_ref().is_some_and(|pending| {
            pending.ticket == ticket
                && ticket.document_version == self.document_version
                && self.document.as_ref() == Some(&pending.source)
        });
        if !valid {
            return false;
        }
        if let Some(pending) = self.pending_save.take() {
            self.saved_snapshot = Some(pending.content);
        }
        true
    }

    pub(super) fn fail_save(&mut self, ticket: SaveTicket) -> bool {
        if !self
            .pending_save
            .as_ref()
            .is_some_and(|pending| pending.ticket == ticket)
        {
            return false;
        }
        self.pending_save = None;
        true
    }
}
