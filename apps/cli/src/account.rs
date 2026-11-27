//! `nexus auth delete-account` (`account.ts`): delete your own account on Gateway. On a terminal the person
//! types DELETE to confirm; without one, `--confirm DELETE` says it. `--dry-run` says what would go and changes
//! nothing. Gateway refuses while
//! you own a company organization, and with an API key. Once the account is gone, the saved sign-in is forgotten, as
//! `nexus auth logout` forgets it.

use serde_json::{Value, json};

use crate::Context;
use crate::billing::dollars;
use crate::cli::cli_client;
use crate::config::Config;
use crate::error::{CliError, Result, usage};
use crate::json::{a, js_string, pretty, s, t};
use crate::style::Style;

/// What the person types to confirm, as Gateway asks for it (`account_deletion::CONFIRMATION`).
const CONFIRMATION: &str = "DELETE";

/// The organizations the person leaves, by name, joined for a sentence.
fn left(answer: &Value) -> Option<String> {
    let names: Vec<String> = a(answer, "left")
        .iter()
        .map(|org| {
            s(org, "name")
                .map(String::from)
                .unwrap_or_else(|| js_string(org.get("id")))
        })
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// What the deletion takes, as one sentence for a person.
fn takes(answer: &Value) -> String {
    let mut parts = vec!["your sign-in, your email and your name".to_string()];
    if answer.get("personalOrg").is_some_and(Value::is_string) {
        parts.push(if t(answer, "planEnded") {
            "your personal organization and its plan".into()
        } else {
            "your personal organization".into()
        });
    }
    if let Some(names) = left(answer) {
        parts.push(format!("your place in {names}"));
    }
    let last = parts.pop().unwrap_or_default();
    format!("{} and {last}", parts.join(", "))
}

/// What the deletion does with money, as sentences that follow [`takes`]; empty when nothing is owed or lost.
fn money(answer: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(unpaid) = s(answer, "unpaidUsageUsd") {
        parts.push(format!(
            " You owe {} for usage: Nexus charges it to your card on file first, and writes off what the card does not pay.",
            dollars(unpaid)
        ));
    }
    if let Some(credits) = s(answer, "creditsLostUsd") {
        parts.push(format!(
            " Your {} of Nexus credits is lost; nothing refunds it.",
            dollars(credits)
        ));
    }
    parts.concat()
}

/// Delete the signed-in person's account; `forget` drops the saved sign-in once Gateway has deleted it.
pub fn delete_account(
    has: impl Fn(&str) -> bool,
    config: &Config,
    ctx: &Context,
    style: &Style,
    json: bool,
    confirm: Option<&str>,
    forget: impl FnOnce() -> Result<()>,
) -> Result<i32> {
    if confirm.is_some_and(|typed| typed != CONFIRMATION) {
        return usage(
            format!("--confirm takes {CONFIRMATION}, in capitals. Nothing changed."),
            Some("Run `nexus auth delete-account --confirm DELETE` to delete your account."),
        );
    }
    let Some(token) = config.api_key.as_deref() else {
        return usage(
            "Not signed in.",
            Some("Sign in with `nexus auth login`, then run `nexus auth delete-account`."),
        );
    };
    let core = cli_client(&config.url, Some(token));
    let out = |line: String| ctx.io.out(&line);
    let dry = core.post("/v1/auth/account/delete", json!({ "dryRun": true }))?;
    if has("dry-run") {
        if json {
            out(pretty(&dry));
            return Ok(0);
        }
        out(format!(
            "{} Dry run: nothing changed. Deleting your account would delete {}.{} Your organizations' spend and audit trail stay.",
            style.warn,
            takes(&dry),
            money(&dry)
        ));
        return Ok(0);
    }
    if t(&dry, "signInNeeded") {
        return Err(CliError::hint(
            "Sign in again to delete your account: a sign-in from the last 10 minutes deletes it.",
            "Run `nexus auth login`, then `nexus auth delete-account` within 10 minutes.",
        )
        .into());
    }
    if confirm.is_none() {
        if !ctx.interactive || json {
            return usage(
                format!("This deletes {} for good.{}", takes(&dry), money(&dry)),
                Some("Run `nexus auth delete-account --confirm DELETE` to go ahead."),
            );
        }
        let question = format!(
            "This deletes {} for good.{} Type {CONFIRMATION} to confirm: ",
            takes(&dry),
            money(&dry)
        );
        if !ctx.io.write(&question) {
            out(question);
        }
        if ctx.io.read_line().unwrap_or_default().trim() != CONFIRMATION {
            out(style.dim("Nothing changed."));
            return Ok(1);
        }
    }
    let deleted = core.post("/v1/auth/account/delete", json!({ "confirm": CONFIRMATION }))?;
    forget()?;
    if json {
        let mut value = deleted;
        if let Value::Object(map) = &mut value {
            map.insert("loggedOut".into(), json!(true));
        }
        out(pretty(&value));
        return Ok(0);
    }
    out(format!(
        "{} Your account is deleted, and you are signed out of {}.",
        style.ok, config.url
    ));
    Ok(0)
}
