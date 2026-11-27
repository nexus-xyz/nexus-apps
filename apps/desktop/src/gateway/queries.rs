//! Saved reads and local writes share bounded freshness and credential guards.
use super::{Gateway, Verb};
use nexus_client::Client;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Clone)]
pub(super) struct ReadVersion {
    pub epoch: Arc<AtomicU64>,
    pending: tokio::sync::watch::Sender<u64>,
}

impl ReadVersion {
    async fn ready(&self) -> Result<(), tokio::sync::watch::error::RecvError> {
        let mut pending = self.pending.subscribe();
        loop {
            let active = *pending.borrow_and_update();
            if active == 0 {
                return Ok(());
            }
            pending.changed().await?;
        }
    }
    async fn snapshot(&self, cache: &AtomicU64) -> Result<(u64, u64), tokio::sync::watch::error::RecvError> {
        loop {
            let version = self.epoch.load(Ordering::SeqCst);
            self.ready().await?;
            let cache_version = cache.load(Ordering::SeqCst);
            if self.epoch.load(Ordering::SeqCst) == version {
                return Ok((cache_version, version));
            }
        }
    }
}

pub(super) struct Reply {
    pub answer: nexus_client::Result<Value>,
    pub version: (Arc<AtomicU64>, u64),
    pub cache_version: u64,
}
#[derive(Clone)]
struct Call {
    client: Client,
    verb: Verb,
    path: String,
    body: Option<Value>,
    auth: Arc<AtomicU64>,
    expected: u64,
    cache: Arc<crate::warm_cache::WarmCache>,
    cache_version: Arc<AtomicU64>,
    version: ReadVersion,
    block_reads: bool,
    finished: Arc<AtomicBool>,
}
impl Call {
    fn invalidate(&self) {
        if self.auth.load(Ordering::SeqCst) == self.expected {
            self.cache
                .invalidate_versioned(&self.client, &self.cache_version, Some(&self.version.epoch));
        }
    }
    fn failed(&self, message: &str) -> Reply {
        Reply {
            answer: Err(nexus_client::Transport::Other(message.into()).into()),
            version: (self.version.epoch.clone(), self.version.epoch.load(Ordering::SeqCst)),
            cache_version: self.cache_version.load(Ordering::SeqCst),
        }
    }
    fn begin(&self) {
        if self.block_reads {
            self.version.pending.send_modify(|count| *count += 1);
        }
        self.invalidate();
    }
    fn finish(&self) {
        if self.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        self.invalidate();
        if self.block_reads {
            self.version
                .pending
                .send_modify(|count| *count = count.saturating_sub(1));
        }
    }
    async fn execute(self) -> Reply {
        if self.auth.load(Ordering::SeqCst) != self.expected {
            if self.verb != Verb::Get {
                self.finish();
            }
            return self.failed("The signed-in account changed.");
        }
        let (cache_version, version) = if self.verb == Verb::Get {
            match self.version.snapshot(&self.cache_version).await {
                Ok(stamp) => stamp,
                Err(_) => return self.failed("The pending change ended unexpectedly."),
            }
        } else {
            (
                self.cache_version.load(Ordering::SeqCst),
                self.version.epoch.load(Ordering::SeqCst),
            )
        };
        if self.auth.load(Ordering::SeqCst) != self.expected {
            if self.verb != Verb::Get {
                self.finish();
            }
            return self.failed("The signed-in account changed.");
        }
        let body = self.body.clone().unwrap_or_else(|| json!({}));
        let answer = match self.verb {
            Verb::Get => self.client.get(&self.path).await,
            Verb::Post => self.client.post(&self.path, body).await,
            Verb::Put => self.client.put(&self.path, body).await,
            Verb::Patch => self.client.patch(&self.path, body).await,
            Verb::Delete => self.client.delete(&self.path).await,
        };
        if self.verb != Verb::Get {
            self.finish();
        }
        if self.auth.load(Ordering::SeqCst) != self.expected {
            return self.failed("The signed-in account changed.");
        }
        if self.verb == Verb::Get {
            if let (Some(key), Ok(value)) = (crate::warm_cache::key(&self.client, &self.path), &answer) {
                self.cache.put_guarded(&key, value, || {
                    self.auth.load(Ordering::SeqCst) == self.expected
                        && self.cache_version.load(Ordering::SeqCst) == cache_version
                });
            }
        }
        Reply {
            answer,
            version: (self.version.epoch.clone(), version),
            cache_version,
        }
    }
}

/// Direct streaming/native actions use the same invalidation, including early exits.
pub(super) struct WriteContext {
    call: Call,
    finished: bool,
}
impl WriteContext {
    pub fn client(&self) -> Client {
        self.call.client.clone()
    }
    pub fn scope(&self) -> (&Arc<AtomicU64>, u64) {
        (&self.call.auth, self.call.expected)
    }
    pub fn finish(&mut self) {
        if !self.finished {
            self.call.finish();
            self.finished = true;
        }
    }
}
impl Drop for WriteContext {
    fn drop(&mut self) {
        self.finish();
    }
}

/// A confirmed artifact operation keeps its captured account across navigation or sign-in changes.
pub(super) struct FrozenWriteContext {
    gateway: Gateway,
    client: Client,
    path: String,
    version: ReadVersion,
}
impl Drop for FrozenWriteContext {
    fn drop(&mut self) {
        // Account switches clear the registry. Invalidate the current stamp for this captured scope too.
        let current = self.gateway.read_version(&self.client, &self.path);
        self.gateway.inner.cache.invalidate_versioned(
            &self.client,
            &self.gateway.inner.cache_revision,
            Some(&current.epoch),
        );
        self.version
            .pending
            .send_modify(|count| *count = count.saturating_sub(1));
    }
}

impl Gateway {
    pub(super) fn begin_frozen_write(&self, client: Client, path: &str) -> FrozenWriteContext {
        let version = self.read_version(&client, path);
        version.pending.send_modify(|count| *count += 1);
        self.inner
            .cache
            .invalidate_versioned(&client, &self.inner.cache_revision, Some(&version.epoch));
        FrozenWriteContext {
            gateway: self.clone(),
            client,
            path: path.into(),
            version,
        }
    }

    pub(super) fn read_version(&self, client: &Client, path: &str) -> ReadVersion {
        let limit = if path.starts_with("/v1/chat/sessions/") { 5 } else { 3 };
        let family = path
            .split('?')
            .next()
            .unwrap_or(path)
            .split('/')
            .take(limit)
            .collect::<Vec<_>>()
            .join("/");
        let key = crate::warm_cache::key(client, &family).unwrap_or_else(|| format!("{}:{family}", self.inner.url));
        let mut versions = self.inner.read_versions.lock().expect("read versions");
        versions
            .entry(key)
            .or_insert_with(|| ReadVersion {
                epoch: Arc::new(AtomicU64::new(0)),
                pending: tokio::sync::watch::channel(0).0,
            })
            .clone()
    }
    fn prepare(&self, verb: Verb, path: String, body: Option<Value>) -> Call {
        let (client, expected) = {
            let client = self.inner.client.lock().expect("client lock");
            (client.clone(), self.inner.cache_epoch.load(Ordering::SeqCst))
        };
        let version = self.read_version(&client, &path);
        let block_reads = verb != Verb::Get && !path.starts_with("/v1/chat/");
        let call = Call {
            client,
            verb,
            path,
            body,
            auth: self.inner.cache_epoch.clone(),
            expected,
            cache: self.inner.cache.clone(),
            cache_version: self.inner.cache_revision.clone(),
            version,
            block_reads,
            finished: Arc::new(AtomicBool::new(false)),
        };
        if verb != Verb::Get {
            call.begin();
        }
        call
    }
    /// Read a sidebar chat ahead with the credential captured before the task can wait.
    pub fn preload_session(&self, id: String) {
        let path = format!("/v1/chat/sessions/{}", nexus_client::url::enc(&id));
        let call = self.prepare(Verb::Get, path, None);
        self.inner.runtime.spawn(async move {
            let saved =
                crate::warm_cache::key(&call.client, &call.path).is_some_and(|key| call.cache.get(&key).is_some());
            if !saved {
                call.execute().await;
            }
        });
    }

    pub(super) fn begin_write(&self, path: &str) -> WriteContext {
        WriteContext {
            call: self.prepare(Verb::Post, path.into(), None),
            finished: false,
        }
    }
    pub(super) fn request_result(
        &self,
        verb: Verb,
        path: String,
        body: Option<Value>,
    ) -> impl std::future::Future<Output = Reply> + 'static + use<> {
        let call = self.prepare(verb, path, body);
        let mut task = self.inner.runtime.spawn(call.clone().execute());
        let runtime = self.inner.runtime.clone();
        async move {
            // A write can finish while a GET waits on either executor. Re-read, at most twice.
            let mut retries = 2;
            loop {
                let result = match task.await {
                    Ok(result) => result,
                    Err(error) => {
                        if verb != Verb::Get {
                            call.finish();
                        }
                        return call.failed(&error.to_string());
                    }
                };
                if call.auth.load(Ordering::SeqCst) != call.expected {
                    return call.failed("The signed-in account changed.");
                }
                if verb == Verb::Get && result.cache_version != call.cache_version.load(Ordering::SeqCst) {
                    if retries == 0 {
                        return call.failed("The data changed while refreshing. Try again.");
                    }
                    retries -= 1;
                    task = runtime.spawn(call.clone().execute());
                    continue;
                }
                return result;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{pin_mut, poll};
    #[test]
    fn a_queued_chat_preload_does_not_use_a_later_sign_in() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        listener.set_nonblocking(true).expect("nonblocking");
        let (gateway, _) = Gateway::at(
            runtime.handle().clone(),
            &format!("http://{}", listener.local_addr().unwrap()),
        );
        gateway.set_token(Some("first".into()));
        gateway.preload_session("first-sign-in-chat".into());
        gateway.set_token(Some("second".into()));
        runtime.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(20)).await });
        assert!(matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
    }

    #[test]
    fn a_typed_write_keeps_the_write_guards_of_request() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (gateway, _) = Gateway::at(runtime.handle().clone(), "http://127.0.0.1:9");
        gateway.set_token(Some("test-token".into()));
        let version = gateway.read_version(&gateway.client(), "/v1/billing");
        let before = version.epoch.load(Ordering::SeqCst);
        let answer = runtime.block_on(gateway.request_typed(Verb::Post, "/v1/billing/confirm-payment", None));
        assert!(answer.is_err(), "nothing listens there");
        // The write moved Billing's reads on, so a saved `/v1/billing` is not shown as current.
        assert!(version.epoch.load(Ordering::SeqCst) > before);
    }
    #[test]
    fn a_read_waits_for_all_local_writes_and_releases_on_drop() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (gateway, _) = Gateway::at(runtime.handle().clone(), "http://gateway.invalid");
        gateway.set_token(Some("test-token".into()));
        let version = gateway.read_version(&gateway.client(), "/v1/orgs/settings");
        let first = gateway.begin_write("/v1/orgs/settings");
        let second = gateway.begin_write("/v1/orgs/settings");
        runtime.block_on(async {
            let ready = version.snapshot(&gateway.inner.cache_revision);
            pin_mut!(ready);
            assert!(poll!(ready.as_mut()).is_pending());
            drop(first);
            assert!(poll!(ready.as_mut()).is_pending());
            drop(second);
            let std::task::Poll::Ready(Ok((_, stamp))) = poll!(ready.as_mut()) else {
                panic!("read must resume after writes")
            };
            assert_eq!(stamp, 4, "the read must not retain the revision from before its wait");
        });
        assert_eq!(version.epoch.load(Ordering::SeqCst), 4);
    }
    #[test]
    fn frozen_artifact_writes_wait_and_invalidate_the_captured_account_after_switching_back() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (gateway, _) = Gateway::at(runtime.handle().clone(), "http://gateway.invalid");
        gateway.set_token(Some("one".into()));
        let captured = gateway.client();
        let key = crate::warm_cache::key(&captured, "/v1/artifacts?limit=50").expect("key");
        let version = gateway.read_version(&captured, "/v1/artifacts");
        let write = gateway.begin_frozen_write(captured.clone(), "/v1/artifacts");
        runtime.block_on(async {
            let ready = version.ready();
            pin_mut!(ready);
            assert!(poll!(ready.as_mut()).is_pending());
        });
        gateway.set_token(Some("two".into()));
        let other_key = crate::warm_cache::key(&gateway.client(), "/v1/artifacts?limit=50").expect("other key");
        gateway
            .inner
            .cache
            .put_guarded(&other_key, &json!({"other":true}), || true);
        gateway.set_token(Some("one".into()));
        let current = gateway.read_version(&gateway.client(), "/v1/artifacts");
        let before = current.epoch.load(Ordering::SeqCst);
        gateway.inner.cache.put_guarded(&key, &json!({"old":true}), || true);
        drop(write);
        assert!(gateway.inner.cache.get(&key).is_none());
        assert_eq!(gateway.inner.cache.get(&other_key), Some(json!({"other":true})));
        assert!(current.epoch.load(Ordering::SeqCst) > before);
        runtime
            .block_on(version.ready())
            .expect("old wait released on completion");
    }
    #[test]
    fn restoring_the_same_credential_does_not_invalidate_other_windows() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (gateway, _) = Gateway::at(runtime.handle().clone(), "http://gateway.invalid");
        gateway.set_token(Some("test-token".into()));
        let epoch = gateway.inner.cache_epoch.load(Ordering::SeqCst);
        gateway.set_token(Some("test-token".into()));
        assert_eq!(gateway.inner.cache_epoch.load(Ordering::SeqCst), epoch);
        gateway.set_token(None);
        assert_ne!(gateway.inner.cache_epoch.load(Ordering::SeqCst), epoch);
    }
}

impl Gateway {
    pub(super) fn request_batch(
        &self,
        paths: Vec<String>,
    ) -> impl std::future::Future<Output = Result<Vec<Reply>, String>> + 'static + use<> {
        let expected = self.inner.cache_epoch.load(Ordering::SeqCst);
        let mut pending = paths
            .iter()
            .map(|path| self.request_result(Verb::Get, path.clone(), None))
            .collect::<Vec<_>>();
        let gateway = self.clone();
        async move {
            let mut retries = 2;
            loop {
                let replies = futures::future::join_all(pending).await;
                if gateway.inner.cache_epoch.load(Ordering::SeqCst) != expected {
                    return Err("The signed-in account changed.".into());
                }
                let current = gateway.inner.cache_revision.load(Ordering::SeqCst);
                if replies.iter().all(|reply| reply.cache_version == current) {
                    return Ok(replies);
                }
                if retries == 0 {
                    return Err("The data changed while refreshing. Try again.".into());
                }
                retries -= 1;
                pending = paths
                    .iter()
                    .map(|path| gateway.request_result(Verb::Get, path.clone(), None))
                    .collect();
            }
        }
    }
}
