//! The built-in `nexus` server's own tools that the app answers, as data: their names, limits and `tools/list`
//! definitions. The pull request watch (`nexus-pull-requests`) and `request_secret` (`nexus-desktop-mcp`) re-export
//! them; `nexus-host` lists and relays them on a person's own server, where no desktop crate builds.

use std::time::Duration;

use serde_json::{Value, json};

use crate::mcp_servers::SECRET_WAIT;

/// The two tools of the built-in `nexus` MCP server that reach the pull request watch.
pub const PULL_WATCH_TOOL_NAMES: [&str; 2] = ["watch_pull_request", "unwatch_pull_request"];
/// How many times a session's agent is woken on its own, for the session's whole life.
pub const MAX_PULL_WAKES: u32 = 3;

/// The pull request tools' definitions, for the server's `tools/list`.
pub fn pull_watch_tool_definitions() -> Vec<Value> {
    let target = json!({"type": "object", "properties": {
        "url": {"type": "string", "description": "The pull request's GitHub URL, or the merge request's GitLab URL. Required for shared-folder sessions unless number is given; worktree sessions default to their branch."},
        "number": {"type": "integer", "minimum": 1, "description": "The pull request's number in this session's repository, when no url."},
    }, "additionalProperties": false});
    vec![
        json!({"name": PULL_WATCH_TOOL_NAMES[0],
            "description": format!("Watch this session's pull request, and get a message when a check fails, all checks pass, \
                a review or comment arrives, a merge conflict appears, or it merges or closes (which ends the watch). The \
                message comes as a new turn after the current one, never in the middle of it. At most {MAX_PULL_WAKES} \
                messages per session, counted with the person's own. Sessions on a remote machine, and computers \
                without the GitHub CLI (gh), or the GitLab CLI (glab) for a GitLab repository, are not watched."),
            "inputSchema": target,
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}}),
        json!({"name": PULL_WATCH_TOOL_NAMES[1],
            "description": "Stop the watch this session started with watch_pull_request. The person's own \
                notifications go on.",
            "inputSchema": target,
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}}),
    ]
}

/// `request_secret`'s name in the built-in server.
pub const SECRET_TOOL_NAME: &str = "request_secret";
/// How long a written secret file lives at most.
pub const SECRET_TTL: Duration = Duration::from_secs(15 * 60);
/// The longest secret name and reason a call may give.
pub const SECRET_MAX_NAME: usize = 80;
pub const SECRET_MAX_REASON: usize = 500;

/// `request_secret`'s definition, for the server's `tools/list`.
pub fn secret_tool_definition() -> Value {
    json!({"name": SECRET_TOOL_NAME,
        "description": format!("Ask the person for a secret (an API key, a password, a token) in a private card in this \
            session, instead of asking them to paste it into the chat. Wait for the answer, at most {} minutes. The \
            value never enters this conversation: you get the path of a file that holds it, readable only by this user, \
            deleted when this agent run ends or {} minutes after it is written. Read it only inside a shell command, \
            for example `API_KEY=\"$(cat <path>)\" the-command` or a tool's --from-file option. Never print it, echo \
            it, or copy it into files in the repository. Not available in sessions on a cloud machine.",
            SECRET_WAIT.as_secs() / 60, SECRET_TTL.as_secs() / 60),
        "inputSchema": {"type": "object", "properties": {
            "name": {"type": "string", "maxLength": SECRET_MAX_NAME, "description": "What the secret is, as the person knows it: \"OpenAI API key\", \"STRIPE_SECRET_KEY\"."},
            "reason": {"type": "string", "maxLength": SECRET_MAX_REASON, "description": "Why you need it, in one sentence the person reads before they answer."},
            "delivery": {"type": "string", "enum": ["file"], "description": "How you get it: \"file\", the path of a file that holds it (the only way, and the default)."},
        }, "required": ["name", "reason"], "additionalProperties": false},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}})
}
