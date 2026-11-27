//! Ciphertext-free notifications for the native viewer. Dropping the stream cancels its frozen-account request.
use super::Gateway;
use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, unbounded},
};
use serde_json::Value;

#[derive(Debug)]
pub(crate) enum News {
    Open,
    Event(Value),
    Closed,
    Denied,
}

pub(crate) struct SessionStream {
    pub events: UnboundedReceiver<News>,
    stop: Option<tokio::task::AbortHandle>,
}
#[cfg(test)]
impl SessionStream {
    /// A transport-free stream for the native viewer's asynchronous lifecycle fixtures.
    pub(crate) fn fixture(events: UnboundedReceiver<News>) -> Self {
        Self { events, stop: None }
    }
}

impl Drop for SessionStream {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            stop.abort();
        }
    }
}

impl Gateway {
    pub(crate) fn session_stream(&self, thread: &str) -> SessionStream {
        let client = self.client();
        let path = format!("/v1/threads/{}/stream?after=-1", nexus_client::url::enc(thread));
        let (out, events) = unbounded();
        let task = self.inner.runtime.spawn(async move {
            let headers = [("accept".to_string(), "text/event-stream".to_string())];
            match client.stream(nexus_client::Method::GET, &path, &headers, None).await {
                Ok((200, _, body)) => {
                    let _ = out.unbounded_send(News::Open);
                    let mut decoder = nexus_client::sse::Decoder::new();
                    let mut body = std::pin::pin!(body);
                    while let Some(piece) = body.next().await {
                        let Ok(piece) = piece else { break };
                        for event in decoder.push(&piece) {
                            let Ok(event) = event else {
                                return;
                            };
                            if out.unbounded_send(News::Event(event)).is_err() {
                                return;
                            }
                        }
                    }
                    let _ = out.unbounded_send(News::Closed);
                }
                Ok((401 | 403 | 404 | 410, _, _)) => {
                    let _ = out.unbounded_send(News::Denied);
                }
                _ => {
                    let _ = out.unbounded_send(News::Closed);
                }
            }
        });
        SessionStream {
            events,
            stop: Some(task.abort_handle()),
        }
    }
}
