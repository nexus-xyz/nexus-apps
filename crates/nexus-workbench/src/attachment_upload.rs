//! A message's files sent to the session's computer. A file of one piece or less goes whole in the message. A larger one
//! goes first in pieces ([`CHUNK_BYTES`]) and the message names it by its upload id. An [`UploadRun`] counts how much of
//! each file has gone, and the person can cancel it. A cancelled or failed send deletes what it uploaded.
//!
//! Nexus GPUI uses this.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Mutex, MutexGuard};

use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};

use crate::attachments::{CHUNK_BYTES, Chunk, FileInput, Progress};

/// The error a cancelled send returns (`CANCELLED` in `attachment-upload.ts`).
pub const CANCELLED: &str = "Sending was cancelled. Your message and files are still here.";

/// A file to send: the composer's id for it, its name and its bytes.
#[derive(Debug, Clone, Copy)]
pub struct Outgoing<'a> {
    pub id: u64,
    pub name: &'a str,
    pub bytes: &'a [u8],
}

/// How a send's upload ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Every file went; the message can name them.
    Uploaded,
    /// The person cancelled the send from file `id`'s chip.
    Cancelled(u64),
    /// A piece failed; what went up was deleted.
    Failed,
}

#[derive(Debug, Default)]
struct State {
    /// How much of each file sent in pieces has gone, from 0 to 1, by id.
    shares: BTreeMap<u64, f32>,
    cancelled: Option<u64>,
    outcome: Option<Outcome>,
}

/// One send's upload, shared between the task that sends and the screen that shows it.
#[derive(Debug, Default)]
pub struct UploadRun {
    state: Mutex<State>,
}

impl UploadRun {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How much of file `id` has gone, from 0 to 1; `None` when it is not being sent in pieces.
    pub fn share(&self, id: u64) -> Option<f32> {
        self.state().shares.get(&id).copied()
    }

    /// Stop the send from file `id`'s chip. The piece on its way finishes; no other piece goes.
    pub fn cancel(&self, id: u64) {
        let mut state = self.state();
        if state.outcome.is_none() && state.cancelled.is_none() {
            state.cancelled = Some(id);
        }
    }

    /// How the upload ended; `None` while it runs.
    pub fn outcome(&self) -> Option<Outcome> {
        self.state().outcome
    }

    fn cancelled(&self) -> Option<u64> {
        self.state().cancelled
    }

    fn begin(&self, ids: impl Iterator<Item = u64>) {
        self.state().shares.extend(ids.map(|id| (id, 0.)));
    }

    fn report(&self, id: u64, sent: usize, size: usize) {
        if let Some(share) = self.state().shares.get_mut(&id) {
            *share = (sent as f32 / size.max(1) as f32).clamp(0., 1.);
        }
    }

    fn finish(&self, outcome: Outcome) {
        let mut state = self.state();
        state.shares.clear();
        state.outcome = Some(outcome);
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `files` as a message to session `thread_id` takes them (`sendableFiles`): a file of one piece or less whole, a larger
/// one first uploaded in pieces through `chunk` for that session only, then named by its upload id. A new session's id
/// is [`crate::creation_thread_id`] of its creation request. `run` counts the pieces and stops at a cancel. A cancelled
/// or failed send deletes what it uploaded through `cancel`, and returns [`CANCELLED`] or the piece's error.
pub async fn send_files<C, CF, X, XF>(
    thread_id: &str,
    files: &[Outgoing<'_>],
    run: &UploadRun,
    mut chunk: C,
    mut cancel: X,
) -> Result<Vec<FileInput>, String>
where
    C: FnMut(Chunk) -> CF,
    CF: Future<Output = Result<Progress, String>>,
    X: FnMut(String) -> XF,
    XF: Future<Output = Result<(), String>>,
{
    run.begin(
        files
            .iter()
            .filter(|file| file.bytes.len() > CHUNK_BYTES)
            .map(|file| file.id),
    );
    let mut started = Vec::new();
    let sent = upload(thread_id, files, run, &mut chunk, &mut started).await;
    let outcome = match (&sent, run.cancelled()) {
        (_, Some(id)) => Outcome::Cancelled(id),
        (Ok(_), None) => Outcome::Uploaded,
        (Err(_), None) => Outcome::Failed,
    };
    if outcome != Outcome::Uploaded {
        for id in started {
            let _ = cancel(id).await;
        }
    }
    run.finish(outcome);
    match outcome {
        Outcome::Cancelled(_) => Err(CANCELLED.to_string()),
        Outcome::Uploaded | Outcome::Failed => sent,
    }
}

async fn upload<C, CF>(
    thread_id: &str,
    files: &[Outgoing<'_>],
    run: &UploadRun,
    chunk: &mut C,
    started: &mut Vec<String>,
) -> Result<Vec<FileInput>, String>
where
    C: FnMut(Chunk) -> CF,
    CF: Future<Output = Result<Progress, String>>,
{
    let stopped = || run.cancelled().map_or(Ok(()), |_| Err(CANCELLED.to_string()));
    let mut sent = Vec::new();
    for file in files {
        let size = file.bytes.len();
        if size <= CHUNK_BYTES {
            sent.push(FileInput {
                name: file.name.to_string(),
                data: Some(STANDARD.encode(file.bytes)),
                upload: None,
            });
            continue;
        }
        stopped()?;
        let upload_id = uuid::Uuid::new_v4().to_string();
        let sha256 = sha256_hex(file.bytes);
        started.push(upload_id.clone());
        for offset in (0..size).step_by(CHUNK_BYTES) {
            stopped()?;
            let end = (offset + CHUNK_BYTES).min(size);
            chunk(Chunk {
                upload_id: upload_id.clone(),
                size: size as u64,
                offset: offset as u64,
                data: STANDARD.encode(&file.bytes[offset..end]),
                sha256: Some(sha256.clone()),
                thread_id: Some(thread_id.to_string()),
            })
            .await?;
            run.report(file.id, end, size);
        }
        stopped()?;
        sent.push(FileInput {
            name: file.name.to_string(),
            data: None,
            upload: Some(upload_id),
        });
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn block<T>(future: impl Future<Output = T>) -> T {
        futures::executor::block_on(future)
    }

    fn ok() -> Result<Progress, String> {
        Ok(Progress {
            received: 0,
            done: false,
        })
    }

    #[test]
    fn a_small_file_goes_whole_and_shows_no_progress() {
        let run = UploadRun::default();
        let bytes = vec![7u8; 10];
        let files = [Outgoing {
            id: 1,
            name: "a.txt",
            bytes: &bytes,
        }];
        let sent = block(send_files(
            "t_1",
            &files,
            &run,
            |_| async { ok() },
            |_| async { Ok(()) },
        ))
        .expect("the files upload");
        assert_eq!(sent[0].data.as_deref(), Some(STANDARD.encode(&bytes).as_str()));
        assert!(sent[0].upload.is_none());
        assert_eq!(run.share(1), None);
        assert_eq!(run.outcome(), Some(Outcome::Uploaded));
    }

    /// Each piece moves the file's share by its bytes, ending at 1; the shares clear when the upload ends.
    #[test]
    fn a_large_file_reports_its_share_piece_by_piece() {
        let run = UploadRun::default();
        let bytes = vec![1u8; CHUNK_BYTES * 2 + CHUNK_BYTES / 2];
        let files = [Outgoing {
            id: 9,
            name: "big.bin",
            bytes: &bytes,
        }];
        let seen = RefCell::new(Vec::new());
        let sent = block(send_files(
            "t_1",
            &files,
            &run,
            |piece| {
                seen.borrow_mut().push((piece.offset, run.share(9)));
                async { ok() }
            },
            |_| async { Ok(()) },
        ))
        .expect("the file uploads");
        let seen = seen.into_inner();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], (0, Some(0.)));
        assert_eq!(seen[1], (CHUNK_BYTES as u64, Some(0.4)));
        assert_eq!(seen[2], (2 * CHUNK_BYTES as u64, Some(0.8)));
        assert!(sent[0].upload.is_some() && sent[0].data.is_none());
        assert_eq!(run.share(9), None);
        assert_eq!(run.outcome(), Some(Outcome::Uploaded));
    }

    /// A cancel stops before the next piece, deletes the upload it started, and names the chip it came from.
    #[test]
    fn a_cancel_stops_the_next_piece_and_deletes_the_upload() {
        let run = UploadRun::default();
        let small = vec![0u8; 5];
        let bytes = vec![2u8; CHUNK_BYTES * 3];
        let files = [
            Outgoing {
                id: 1,
                name: "a.txt",
                bytes: &small,
            },
            Outgoing {
                id: 2,
                name: "big.bin",
                bytes: &bytes,
            },
        ];
        let pieces = RefCell::new(Vec::new());
        let deleted = RefCell::new(Vec::new());
        let result = block(send_files(
            "t_1",
            &files,
            &run,
            |piece| {
                pieces.borrow_mut().push(piece.upload_id.clone());
                if pieces.borrow().len() == 1 {
                    run.cancel(2);
                }
                async { ok() }
            },
            |id| {
                deleted.borrow_mut().push(id);
                async { Ok(()) }
            },
        ));
        assert_eq!(result.expect_err("the send fails"), CANCELLED);
        assert_eq!(pieces.borrow().len(), 1);
        assert_eq!(*deleted.borrow(), *pieces.borrow());
        assert_eq!(run.outcome(), Some(Outcome::Cancelled(2)));
        assert_eq!(run.share(2), None);
    }

    /// A cancel after the upload ended changes nothing: the message already names its files.
    #[test]
    fn a_cancel_after_the_upload_is_ignored() {
        let run = UploadRun::default();
        let bytes = vec![3u8; CHUNK_BYTES + 1];
        let files = [Outgoing {
            id: 4,
            name: "big.bin",
            bytes: &bytes,
        }];
        block(send_files(
            "t_1",
            &files,
            &run,
            |_| async { ok() },
            |_| async { Ok(()) },
        ))
        .expect("the files upload");
        run.cancel(4);
        assert_eq!(run.outcome(), Some(Outcome::Uploaded));
    }

    #[test]
    fn a_failed_piece_deletes_every_upload_it_started() {
        let run = UploadRun::default();
        let first = vec![1u8; CHUNK_BYTES + 1];
        let second = vec![2u8; CHUNK_BYTES + 1];
        let files = [
            Outgoing {
                id: 1,
                name: "one.bin",
                bytes: &first,
            },
            Outgoing {
                id: 2,
                name: "two.bin",
                bytes: &second,
            },
        ];
        let calls = RefCell::new(0);
        let deleted = RefCell::new(Vec::new());
        let result = block(send_files(
            "t_1",
            &files,
            &run,
            |_| {
                *calls.borrow_mut() += 1;
                let failed = *calls.borrow() == 3;
                async move {
                    if failed {
                        Err("The disk is full.".to_string())
                    } else {
                        ok()
                    }
                }
            },
            |id| {
                deleted.borrow_mut().push(id);
                async { Ok(()) }
            },
        ));
        assert_eq!(result.expect_err("the send fails"), "The disk is full.");
        assert_eq!(deleted.borrow().len(), 2);
        assert_eq!(run.outcome(), Some(Outcome::Failed));
    }
}
