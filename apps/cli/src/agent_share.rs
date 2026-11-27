//! `nexus agents share` (`agent-share.ts`): a Claude Code session (`nexus claude`) shared as a Nexus
//! Artifact, read from its transcript on this machine (`claude_transcript.rs`).
//!
//! The snapshot is fitted to Gateway's limits ([`fit_session_snapshot`], the kit's `fitSessionSnapshot`) and sent to
//! `POST /v1/artifacts/sessions`, with chat's audiences (`chat::share_audience`). Sharing the same session again makes
//! its next version and revokes the earlier ones (`replace`), as Nexus for desktop does. Thinking stays out unless
//! --include-thinking. Gateway hides common key and token formats and the home folder's name (`sessions/scrub.rs`);
//! other secrets in tool output, and images, stay. `--private` revokes every active version
//! (`DELETE /v1/artifacts/sessions/{key}`): Gateway keys a session by a hash of its sharer, source and id
//! (`session_key` in `nexus_state::artifacts::sessions`), and [`agent_session_key`] makes the same key here.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
#[cfg(test)]
use nexus_client::session_share::{CUT_MARK, HEADROOM, MAX_EVENTS, PER_EVENT, SESSION_BYTES};
pub use nexus_client::session_share::{Trim, cut_utf8, fit_session_snapshot};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::artifacts::audience_text;
use crate::chat::share_audience;
use crate::claude_transcript::{find_claude_transcript, is_session_id, load_claude_session};
use crate::cli::Cmd;
use crate::client::enc;
use crate::error::{CliError, Error, Result, usage};
use crate::json::{a, js_string, s};
use crate::style::{kv, pairs};

const USAGE: &str = "Usage: nexus agents share <session|latest> [--private | --org | --team T | --people a@x,b@y | --to a@x,b@y | --public [--expires 30d]] [--include-thinking] [--json]";

/// What Gateway hides and what it does not, as Nexus for desktop's share box says it.
pub const SCRUB_NOTE: &str = "Common key and token formats are hidden, and your home folder's name becomes ~. Other secrets in tool output, and anything in an image, are not.";

/// Gateway's key for a shared session: `cds_` and the first 18 bytes of SHA-256(sharer, source, id), base64url.
pub fn agent_session_key(user_id: &str, source: &str, local_id: &str) -> String {
    let digest = Sha256::new()
        .chain_update(user_id.as_bytes())
        .chain_update([0])
        .chain_update(source.as_bytes())
        .chain_update([0])
        .chain_update(local_id.as_bytes())
        .finalize();
    format!("cds_{}", URL_SAFE_NO_PAD.encode(&digest[..18]))
}

/// What the fit and the transcript left out, as one line; `None` when nothing was (`leftOutLine` in the TypeScript).
pub fn left_out_line(trimmed: Trim, skipped: usize) -> Option<String> {
    let plural = |count: usize, one: &str, many: &str| format!("{count} {}", if count == 1 { one } else { many });
    let parts: Vec<String> = [
        (trimmed.outputs > 0).then(|| format!("{} cut short", plural(trimmed.outputs, "tool result", "tool results"))),
        (trimmed.inputs > 0).then(|| {
            format!(
                "{} cut short",
                plural(trimmed.inputs, "string in a tool's input", "strings in tools' inputs")
            )
        }),
        (trimmed.images > 0).then(|| format!("{} left out", plural(trimmed.images, "image", "images"))),
        (trimmed.events > 0).then(|| format!("the last {} left out", plural(trimmed.events, "event", "events"))),
        (skipped > 0).then(|| {
            format!(
                "{} of a kind Nexus does not read skipped",
                plural(skipped, "transcript record", "transcript records")
            )
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| format!("To fit: {}.", parts.join(", ")))
}

pub fn agent_share_command(args: &[String], cmd: &Cmd) -> Result<i32> {
    let style = cmd.style;
    let Some(which) = args.first().filter(|which| !which.is_empty()) else {
        return usage(
            USAGE,
            Some("Give a Claude Code session's id (claude --resume lists them), or latest: this folder's newest."),
        );
    };
    for flag in ["allow-continue", "include-code"] {
        if cmd.has(flag) {
            return usage(
                format!("--{flag} is for chat sessions: readers cannot continue a coding session."),
                Some(USAGE),
            );
        }
    }
    let Some(audience) = share_audience(cmd, USAGE, &["include-thinking"])? else {
        return unshare(which, cmd);
    };

    let include_thinking = cmd.has("include-thinking");
    let (transcript, path) = load_claude_session(which, cmd.ctx, include_thinking)?;
    let (session, images, trimmed) = fit_session_snapshot(&transcript.session, &transcript.images).map_err(|bytes| {
        CliError::hint(
            format!(
                "This session is {:.1} MB even with tool results cut short, and a shared session may be at most 4 MB.",
                bytes as f64 / 1_048_576.0
            ),
            "Share a shorter session: Gateway takes at most 4 MB.",
        )
    })?;
    let session_id = js_string(session.get("id"));
    let mut body = audience;
    body["replace"] = json!(true);
    if include_thinking {
        body["includeThinking"] = json!(true);
    }
    body["session"] = session;
    if !images.is_empty() {
        body["images"] = Value::Array(images);
    }
    let artifact = cmd.core.post("/v1/artifacts/sessions", body)?;
    if cmd.json {
        return Ok(cmd.print(&json!({
            "artifact": artifact,
            "sessionId": session_id,
            "transcript": path.to_string_lossy(),
            "skipped": transcript.skipped,
            "trimmed": {
                "outputs": trimmed.outputs, "inputs": trimmed.inputs, "images": trimmed.images, "events": trimmed.events,
            },
        })));
    }
    let aid = js_string(artifact.get("id"));
    let audience = artifact.get("audience").unwrap_or(&Value::Null);
    let kind = s(audience, "kind").unwrap_or("");
    let whom = if kind == "external" {
        format!(
            "outside your organization with {}",
            a(audience, "people")
                .iter()
                .map(|person| js_string(Some(person)))
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        format!("with {}", audience_text(audience))
    };
    cmd.out(format!(
        "{} Shared {} {whom} (version {}).",
        style.ok,
        style.bold(&js_string(artifact.get("title"))),
        js_string(artifact.get("version"))
    ));
    let mut rows = vec![
        kv("Artifact", aid.clone()),
        kv(
            "Open",
            match artifact.get("publicUrl").filter(|value| !value.is_null()) {
                Some(public) => js_string(Some(public)),
                None => js_string(artifact.get("url")),
            },
        ),
    ];
    if kind == "public" {
        rows.push(kv(
            "Expires",
            match artifact.get("expiresAt").filter(|value| !value.is_null()) {
                Some(at) => js_string(Some(at)),
                None => "never".into(),
            },
        ));
    }
    if kind == "external" {
        rows.push(kv(
            "Sign-in",
            "readers open it signed in with that email; anyone else can ask you for access",
        ));
    }
    rows.push(kv("Session", session_id.clone()));
    rows.push(kv(
        "Thinking",
        if include_thinking {
            "included"
        } else {
            "left out (--include-thinking keeps it)"
        },
    ));
    if kind == "external" {
        rows.push(kv("Access", style.cyan(&format!("nexus artifacts access {aid}"))));
    }
    rows.push(kv(
        "Stop",
        style.cyan(&format!("nexus agents share {session_id} --private")),
    ));
    cmd.lines(pairs(style, &rows));
    if let Some(left_out) = left_out_line(trimmed, transcript.skipped) {
        cmd.out(style.dim(&left_out));
    }
    cmd.out(style.dim(SCRUB_NOTE));
    Ok(0)
}

/// `--private`: every active version of the session's artifact revoked, so only its sharer can read it.
fn unshare(which: &str, cmd: &Cmd) -> Result<i32> {
    // `latest` is this folder's newest transcript; an id is taken as it is, so a session shared from another machine
    // can be stopped too.
    let id = if which == "latest" {
        find_claude_transcript(which, cmd.ctx)?.0
    } else if is_session_id(which) {
        which.to_string()
    } else {
        return usage(
            format!("{which} is not a Claude Code session id."),
            Some("Give the session's id (claude --resume lists them), or latest."),
        );
    };
    let me = cmd.core.get("/v1/tenants/me")?;
    let Some(user) = me.get("user").and_then(|user| s(user, "id")) else {
        return Err(CliError::hint(
            "Only a person's sessions are shared: this sign-in is not a person's.",
            "Sign in as yourself: nexus auth login.",
        )
        .into());
    };
    let key = agent_session_key(user, "claude-code", &id);
    let result = match cmd.core.delete(&format!("/v1/artifacts/sessions/{}", enc(&key))) {
        Ok(result) => result,
        // 404: you have no share of this session, so there is nothing to stop.
        Err(Error::Core(error)) if error.status == 404 => json!({ "revoked": [] }),
        Err(error) => return Err(error),
    };
    let mut revoked: Vec<Value> = a(&result, "revoked")
        .iter()
        .map(|item| json!({ "id": item.get("id"), "version": item.get("version") }))
        .collect();
    revoked.sort_by(|x, y| {
        let version = |value: &Value| value["version"].as_f64().unwrap_or(0.0);
        version(x).total_cmp(&version(y))
    });
    if cmd.json {
        return Ok(cmd.print(&json!({ "sessionId": id, "visibility": "private", "revoked": revoked })));
    }
    if revoked.is_empty() {
        cmd.out(format!("{} {id} was not shared; only you can read it.", cmd.style.ok));
        return Ok(0);
    }
    let versions = revoked
        .iter()
        .map(|item| format!("v{}", js_string(item.get("version"))))
        .collect::<Vec<_>>()
        .join(", ");
    cmd.out(format!(
        "{} Only you can read {id} now: revoked {versions}.",
        cmd.style.ok
    ));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_key_is_gateways() {
        // The same vector is pinned in `agent-share.test.ts`; `session_key` in nexus_state makes it.
        assert_eq!(
            agent_session_key("usr_ada", "claude-code", "5f0c2b1e-0000-4000-8000-000000000001"),
            "cds_pufGTF4Z30oI_ddAvGCsKjNr"
        );
    }

    #[test]
    fn cuts_at_a_character_boundary() {
        assert_eq!(cut_utf8("héllo", 2), "h");
        assert_eq!(cut_utf8("héllo", 3), "hé");
        assert_eq!(cut_utf8("hi", 10), "hi");
    }

    #[test]
    fn fits_long_results_and_drops_images_past_the_limits() {
        let long = "x".repeat(200 * 1024);
        let mut events = vec![json!({ "kind": "user", "text": "go", "images": ["img_1", "img_missing"] })];
        for i in 0..30 {
            events.push(json!({ "kind": "tool", "id": format!("t{i}"), "name": "Bash", "input": { "command": "ls" } }));
            events.push(json!({ "kind": "toolResult", "id": format!("t{i}"), "output": long }));
        }
        let session = json!({ "source": "claude-code", "id": "s", "events": events });
        let images = vec![json!({ "id": "img_1", "data": "AAAA" })];
        let (fitted, kept, trimmed) = fit_session_snapshot(&session, &images).expect("fits");
        assert_eq!(kept, images);
        assert_eq!(fitted["events"][0]["images"], json!(["img_1"]));
        assert_eq!(fitted["events"][2]["output"].as_str().map(str::len), Some(64 * 1024));
        assert_eq!(fitted["events"][2]["truncated"], json!(true));
        assert_eq!(
            trimmed,
            Trim {
                outputs: 30,
                images: 1,
                ..Trim::default()
            }
        );
        assert!(fitted.to_string().len() <= SESSION_BYTES - HEADROOM - PER_EVENT * 61);
        let big = vec![json!({ "id": "img_1", "data": "A".repeat(8 * 1024 * 1024) })];
        let (_, kept, trimmed) = fit_session_snapshot(&session, &big).expect("fits");
        assert!(kept.is_empty());
        assert_eq!(trimmed.images, 2);

        let huge = json!({ "source": "claude-code", "id": "s", "events": vec![json!({ "kind": "user", "text": "y".repeat(5 * 1024 * 1024) })] });
        assert!(fit_session_snapshot(&huge, &[]).is_err());
    }

    /// The kit's `fitSessionSnapshot` keeps the same rules: these mirror its tests in `session-share.test.ts`.
    #[test]
    fn keeps_the_newest_events_as_the_kit_does() {
        let events: Vec<Value> = (0..MAX_EVENTS + 5)
            .map(|i| json!({ "kind": "notice", "text": format!("n{i}") }))
            .collect();
        let session = json!({ "source": "claude-code", "id": "s", "events": events });
        let (fitted, _, trimmed) = fit_session_snapshot(&session, &[]).expect("fits");
        assert_eq!(trimmed.events, 5);
        assert_eq!(fitted["events"][0]["text"], json!("n5"));
        assert_eq!(
            fitted["events"][MAX_EVENTS - 1]["text"],
            json!(format!("n{}", MAX_EVENTS + 4))
        );
    }

    #[test]
    fn cuts_tool_inputs_only_when_results_are_not_enough_and_marks_them() {
        let content = "w".repeat(32 * 1024);
        let mut events = vec![
            json!({ "kind": "user", "text": "build it" }),
            json!({ "kind": "tool", "id": "tw", "name": "Write", "input": { "content": content } }),
        ];
        for i in 0..100 {
            events.push(json!({ "kind": "toolResult", "id": format!("r{i}"), "output": "x".repeat(64 * 1024) }));
        }
        let session = json!({ "source": "claude-code", "id": "s", "events": events });
        let (fitted, _, trimmed) = fit_session_snapshot(&session, &[]).expect("fits");
        assert_eq!(fitted["events"][1]["input"]["content"], json!(content));
        assert_eq!((trimmed.outputs, trimmed.inputs), (100, 0));

        let wide = "w".repeat(64 * 1024);
        let writes: Vec<Value> = (0..80)
            .map(|i| json!({ "kind": "tool", "id": format!("w{i}"), "name": "Write", "input": { "file_path": "a.ts", "content": wide } }))
            .collect();
        let session = json!({ "source": "claude-code", "id": "s", "events": writes });
        let (fitted, _, trimmed) = fit_session_snapshot(&session, &[]).expect("fits");
        let cut = fitted["events"][0]["input"]["content"].as_str().unwrap_or_default();
        assert!(cut.ends_with(CUT_MARK), "a cut input is marked");
        assert!(cut.len() < wide.len());
        assert_eq!(
            fitted["events"][0]["input"]["file_path"],
            json!("a.ts"),
            "short strings stay whole and unmarked"
        );
        assert_eq!(trimmed.inputs, 80);
    }

    #[test]
    fn says_what_was_left_out() {
        assert_eq!(left_out_line(Trim::default(), 0), None);
        assert_eq!(
            left_out_line(
                Trim {
                    outputs: 2,
                    inputs: 1,
                    images: 1,
                    events: 0
                },
                3
            )
            .as_deref(),
            Some(
                "To fit: 2 tool results cut short, 1 string in a tool's input cut short, 1 image left out, 3 transcript records of a kind Nexus does not read skipped."
            )
        );
    }
}
