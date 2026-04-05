//! SSE 事件广播器。
//!
//! 使用 tokio::sync::broadcast 将后端事件分发给所有 SSE 客户端。

use serde::Serialize;
use tokio::sync::broadcast;

use crate::events::{NotificationUpdatedPayload, TodoChangeEvent};
use crate::infrastructure::context_manager::ContextInfo;
use crate::types::config::{DetectedError, StoredNotification};
#[allow(unused_imports)]
use crate::types::domain::{FileChangeEvent, FileChangeType};
use crate::types::ssh::SshConnectionStatus;

/// 后端事件类型 — 从各事件源统一收集后广播给 SSE 客户端。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum BackendEvent {
    FileChange(FileChangeEvent),
    TodoChange(TodoChangeEvent),
    #[allow(dead_code)]
    NotificationNew(StoredNotification),
    #[allow(dead_code)]
    NotificationUpdated(NotificationUpdatedPayload),
    NotificationClicked(DetectedError),
    ContextChanged(ContextInfo),
    SshStatusChanged(SshConnectionStatus),
}

impl BackendEvent {
    /// 返回 SSE 事件名称（event: 字段）。
    pub fn event_name(&self) -> &'static str {
        match self {
            BackendEvent::FileChange(_) => "file-change",
            BackendEvent::TodoChange(_) => "todo-change",
            BackendEvent::NotificationNew(_) => "notification:new",
            BackendEvent::NotificationUpdated(_) => "notification:updated",
            BackendEvent::NotificationClicked(_) => "notification:clicked",
            BackendEvent::ContextChanged(_) => "context:changed",
            BackendEvent::SshStatusChanged(_) => "ssh:status",
        }
    }
}

/// SSE 广播器 — 持有 broadcast channel，向所有 SSE 客户端推送事件。
#[derive(Clone)]
pub struct SSEBroadcaster {
    tx: broadcast::Sender<BackendEvent>,
}

impl SSEBroadcaster {
    /// 创建新的广播器。channel 容量 1024。
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }

    /// 发送事件到所有 SSE 客户端。
    pub fn send(&self, event: BackendEvent) {
        let _ = self.tx.send(event);
    }

    /// 订阅事件流。
    pub fn subscribe(&self) -> broadcast::Receiver<BackendEvent> {
        self.tx.subscribe()
    }

    /// 返回当前活跃接收者数量。
    #[allow(dead_code)]
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_receives_event() {
        let broadcaster = SSEBroadcaster::new();
        let mut rx = broadcaster.subscribe();

        let event = BackendEvent::FileChange(FileChangeEvent {
            event_type: FileChangeType::Change,
            path: "/test/file.jsonl".to_string(),
            project_id: Some("proj".to_string()),
            session_id: Some("sess".to_string()),
            is_subagent: false,
        });
        broadcaster.send(event);

        let received = rx.blocking_recv().unwrap();
        assert!(matches!(received, BackendEvent::FileChange(_)));
    }

    #[test]
    fn event_name_matches() {
        assert_eq!(
            BackendEvent::FileChange(FileChangeEvent {
                event_type: FileChangeType::Change,
                path: "".into(),
                project_id: None,
                session_id: None,
                is_subagent: false,
            })
            .event_name(),
            "file-change"
        );
        assert_eq!(
            BackendEvent::TodoChange(TodoChangeEvent { session_id: "".into() }).event_name(),
            "todo-change"
        );
    }

    #[test]
    fn multiple_subscribers_receive() {
        let broadcaster = SSEBroadcaster::new();
        let mut rx1 = broadcaster.subscribe();
        let mut rx2 = broadcaster.subscribe();

        let event = BackendEvent::TodoChange(TodoChangeEvent {
            session_id: "s1".to_string(),
        });
        broadcaster.send(event);

        assert!(rx1.blocking_recv().is_ok());
        assert!(rx2.blocking_recv().is_ok());
    }

    #[test]
    fn receiver_count_reflects_subscribers() {
        let broadcaster = SSEBroadcaster::new();
        assert_eq!(broadcaster.receiver_count(), 0);

        let _rx1 = broadcaster.subscribe();
        assert_eq!(broadcaster.receiver_count(), 1);

        let _rx2 = broadcaster.subscribe();
        assert_eq!(broadcaster.receiver_count(), 2);
    }

    #[test]
    fn context_changed_event_name() {
        assert_eq!(
            BackendEvent::ContextChanged(ContextInfo {
                id: "local".into(),
                context_type: "local".into(),
            })
            .event_name(),
            "context:changed"
        );
    }
}
