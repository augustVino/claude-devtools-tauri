//! SSE 事件路由处理器。
//!
//! 将 tokio broadcast channel 转换为 Axum SSE 流。

use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::http::sse::SSEBroadcaster;
use crate::http::state::HttpState;

/// SSE 端点 — 客户端通过 GET /api/events 订阅实时事件。
pub async fn sse_handler(
    State(state): State<HttpState>,
) -> Sse<impl futures::Stream<Item = Result<Event, axum::Error>>> {
    let rx = state.broadcaster.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        async move {
            match result {
                Ok(event) => {
                    let name = event.event_name();
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    Some(Ok(Event::default().event(name).data(data)))
                }
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                    log::warn!("SSE broadcast receiver lagged, skipped {} messages", n);
                    None
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(30)))
}
