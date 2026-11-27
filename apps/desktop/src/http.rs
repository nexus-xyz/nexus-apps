//! GPUI's HTTP client, so `img(url)` loads a remote image (the organization's logo, and images in Markdown). GPUI
//! starts with a client that fails every request. This one sends through reqwest on a tokio runtime of its own, as
//! `gateway.rs` does: GPUI runs its own executor, and reqwest needs tokio.

use std::sync::Arc;

use futures::AsyncReadExt;
use futures::channel::oneshot;
use gpui_kit::http_client::{AsyncBody, HttpClient, Response, Url, http};

pub struct Http {
    client: reqwest::Client,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl Http {
    /// After `nexus_client::install_tls`. `None` when the runtime or the client fails: GPUI keeps its own client, and remote images do not load.
    pub fn new() -> Option<Arc<Http>> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("nexus-http")
            .enable_all()
            .build()
            .ok()?;
        let client = reqwest::Client::builder()
            .user_agent(crate::gateway::user_agent())
            .build()
            .ok()?;
        Some(Arc::new(Http {
            client,
            runtime: Arc::new(runtime),
        }))
    }
}

impl HttpClient for Http {
    fn user_agent(&self) -> Option<&http::HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        request: http::Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let (sender, receiver) = oneshot::channel();
        let (parts, mut body) = request.into_parts();
        self.runtime.spawn(async move {
            let result = async {
                let mut bytes = Vec::new();
                body.read_to_end(&mut bytes).await?;
                let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())?;
                let mut outgoing = client.request(method, parts.uri.to_string());
                for (name, value) in &parts.headers {
                    outgoing = outgoing.header(name.as_str(), value.as_bytes());
                }
                if !bytes.is_empty() {
                    outgoing = outgoing.body(bytes);
                }
                let answer = outgoing.send().await?;
                let mut response = http::Response::builder().status(answer.status().as_u16());
                for (name, value) in answer.headers() {
                    response = response.header(name.as_str(), value.as_bytes());
                }
                let body = answer.bytes().await?.to_vec();
                Ok::<_, anyhow::Error>(response.body(AsyncBody::from(body))?)
            }
            .await;
            let _ = sender.send(result);
        });
        Box::pin(async move { receiver.await.map_err(|_| anyhow::anyhow!("the request was dropped"))? })
    }
}
