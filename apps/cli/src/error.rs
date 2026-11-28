//! Failures (`errors.ts`, `toCliError` and `errorText` in `cli.ts`): every failure ends as one short line and a fix,
//! with exit code 1, or 2 for a usage error.

use serde_json::Value;

use crate::billing::{
    CARD_CONFIRM_HINT, CHARGE_PROCESSING_HINT, CHEAPER_MODELS_HINT, CREDITS_OUT_HINT, FREE_START_HINT, PAST_DUE_HINT,
    PAYMENT_FAILED_HINT, USAGE_LIMIT_HINT,
};
use crate::safe_text::safe_text;
use crate::style::{Style, code};

/// A failure the user can act on: what went wrong, and how to fix it.
#[derive(Debug, Clone, PartialEq)]
pub struct CliError {
    pub message: String,
    pub hint: Option<String>,
    pub exit_code: i32,
    /// A stable code for scripts, in `--json`'s `error.code` (`route_unsupported`).
    pub code: Option<&'static str>,
}

impl CliError {
    pub fn new(message: impl Into<String>) -> CliError {
        CliError {
            message: message.into(),
            hint: None,
            exit_code: 1,
            code: None,
        }
    }

    pub fn hint(message: impl Into<String>, hint: impl Into<String>) -> CliError {
        CliError {
            message: message.into(),
            hint: Some(hint.into()),
            exit_code: 1,
            code: None,
        }
    }

    /// A usage error (exit 2).
    pub fn usage(message: impl Into<String>, hint: Option<&str>) -> CliError {
        CliError {
            message: message.into(),
            hint: hint.map(String::from),
            exit_code: 2,
            code: None,
        }
    }

    /// The same error with a stable `code`.
    pub fn with_code(self, code: &'static str) -> CliError {
        CliError {
            code: Some(code),
            ..self
        }
    }
}

/// Gateway's errors and transport failures come from `nexus-client`, shared with the other native clients.
pub use nexus_client::error::{CoreError, Transport, WriteConsent};

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Cli(CliError),
    /// Boxed: Gateway's error carries its details, and every `Result` stays small.
    Core(Box<CoreError>),
    Transport(Transport),
}

impl From<CliError> for Error {
    fn from(error: CliError) -> Error {
        Error::Cli(error)
    }
}

impl From<CoreError> for Error {
    fn from(error: CoreError) -> Error {
        Error::Core(Box::new(error))
    }
}

impl From<nexus_client::Error> for Error {
    fn from(error: nexus_client::Error) -> Error {
        match error {
            nexus_client::Error::Core(error) => Error::Core(error),
            nexus_client::Error::Transport(error) => Error::Transport(error),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A usage error as an `Error`.
pub fn usage<T>(message: impl Into<String>, hint: Option<&str>) -> Result<T> {
    Err(Error::Cli(CliError::usage(message, hint)))
}

/// A provider refused the provider key the call ran on: not the caller's Nexus key, so no sign-in fixes it.
const UPSTREAM_AUTH_HINT: &str = "The provider refused its provider key, not your Nexus API key. If it is your organization's own key, an owner or admin can replace it with `nexus keys`; otherwise tell your Nexus admin.";
/// The provider account behind the call's key is out of credit: not the caller's Nexus budget.
const UPSTREAM_BILLING_HINT: &str = "The provider account is out of credit, not your Nexus budget. If it is your organization's own key, top up that account with the provider; otherwise tell your Nexus admin.";
/// The provider refused this model for the call's key: the key may still serve other models, and it is not the
/// caller's Nexus key.
const UPSTREAM_FORBIDDEN_HINT: &str = "The provider refused this model for its provider key, not for your Nexus API key. Try another model; if it is your organization's own key, check that key's model access with the provider; otherwise tell your Nexus admin.";
/// Gateway told the refused person and everyone who manages budgets (`notified` on the 402).
const NOTIFIED: &str = "Your organization's owners and admins were notified.";
const UPSTREAM_OVERLOADED_HINT: &str = "The provider has no capacity now. Try again in a minute.";

/// Who can raise the budget that refused a call, and whether Gateway told them.
pub fn budget_hint(error: &CoreError) -> String {
    let details = error.details.as_ref();
    let level = details.and_then(|details| details.get("level")).and_then(Value::as_str);
    let notified = details.and_then(|details| details.get("notified")) == Some(&Value::Bool(true));
    let hint = if level == Some("person") || error.message.starts_with("person limit") {
        "Check `nexus budgets show me`. Your monthly limit resets on the 1st (UTC); an owner, or an admin an owner allows, can raise it."
    } else if level == Some("team") || error.message.starts_with("team limit") {
        "Check `nexus budget show team/<slug>`. An owner or admin can raise the team's cap."
    } else {
        "Check `nexus spend`. Each organization has a budget; its owner or an admin can raise it."
    };
    if notified {
        format!("{hint} {NOTIFIED}")
    } else {
        hint.to_string()
    }
}

/// Whether Gateway's error details hold `key: value` (`error.nexus`).
fn detail_is(error: &CoreError, key: &str, value: &str) -> bool {
    error
        .details
        .as_ref()
        .and_then(|details| details.get(key))
        .and_then(Value::as_str)
        == Some(value)
}

/// Whether Gateway's refusal points to Billing (`next: ["billing", …]`): this Gateway bills.
fn offers_billing(error: &CoreError) -> bool {
    let next = error
        .details
        .as_ref()
        .and_then(|details| details.get("next"))
        .and_then(Value::as_array);
    next.is_some_and(|next| next.iter().any(|step| step.as_str() == Some("billing")))
}

/// A write refused for want of the write scope: what it needs, and the command that asks the vendor for it. Nothing
/// retries the write; the person runs it again once they allowed it.
fn write_consent_error(consent: &WriteConsent) -> CliError {
    let needs = if consent.scopes.is_empty() {
        "its write scopes".to_string()
    } else {
        consent.scopes.join(", ")
    };
    CliError::hint(
        format!(
            "{} account {} was connected to read only: this write needs {needs}. Nothing was written.",
            consent.connector, consent.account
        ),
        format!(
            "Allow writes with `{}`, open the link it prints, then run this command again.",
            consent.grant_command()
        ),
    )
}

/// Any failure as a short message and a fix.
pub fn to_cli_error(error: &Error, url: &str) -> CliError {
    match error {
        Error::Cli(error) => error.clone(),
        Error::Core(error) => {
            // Sign-in paused for the auth cutover: say how long to wait, and do not suggest an immediate retry.
            if let Some(message) = error.cutover_paused_message() {
                return CliError::new(message).with_code("cutover_paused");
            }
            let message = error.message.clone();
            match error.code.as_deref().unwrap_or("") {
                "cli_too_old" => {
                    return CliError::hint(
                        message,
                        format!(
                            "Not installed with Nexus's installer? Reinstall: `curl -fsSL {}/install.sh | bash`.",
                            url.strip_suffix('/').unwrap_or(url)
                        ),
                    );
                }
                "budget_exceeded" => return CliError::hint(message, budget_hint(error)),
                "payment_past_due" => return CliError::hint(message, PAST_DUE_HINT),
                "usage_limit_reached" => return CliError::hint(message, USAGE_LIMIT_HINT),
                "payment_failed" => {
                    let confirm = detail_is(error, "reason", "authentication_required");
                    return CliError::hint(
                        message,
                        if confirm {
                            CARD_CONFIRM_HINT
                        } else {
                            PAYMENT_FAILED_HINT
                        },
                    );
                }
                "credit_exhausted" => return CliError::hint(message, CREDITS_OUT_HINT),
                "charge_processing" => return CliError::hint(message, CHARGE_PROCESSING_HINT),
                "subsidy_exhausted" if detail_is(error, "freeStart", "cheaper_models_only") => {
                    return CliError::hint(message, CHEAPER_MODELS_HINT);
                }
                // Only where this Gateway bills: its refusal's `next` names Billing.
                "subsidy_exhausted" if offers_billing(error) => return CliError::hint(message, FREE_START_HINT),
                "upstream_billing" => return CliError::hint(message, UPSTREAM_BILLING_HINT),
                "upstream_auth" => return CliError::hint(message, UPSTREAM_AUTH_HINT),
                "upstream_forbidden" => return CliError::hint(message, UPSTREAM_FORBIDDEN_HINT),
                "upstream_overloaded" => return CliError::hint(message, UPSTREAM_OVERLOADED_HINT),
                "write_consent_required" => {
                    if let Some(consent) = error.write_consent() {
                        return write_consent_error(&consent);
                    }
                }
                "module_unavailable" if is_local(url) => {
                    return CliError::hint(
                        message,
                        "Start the module and check that NEXUS_MODULES on Gateway lists its URL.",
                    );
                }
                "module_unavailable" => {
                    return CliError::hint(
                        message,
                        "Try again in a minute; `nexus modules` lists what this Gateway serves.",
                    );
                }
                // Core's 429 carries `rate_limited`; the OpenAI shape carries `rate_limit_error`.
                "rate_limited" | "rate_limit_error" => {
                    return CliError::hint(message, "Wait about a minute and try again.");
                }
                // Too many requests in flight: Gateway answers with `Retry-After: 1`.
                "concurrency_limit" => return CliError::hint(message, "Retry in a few seconds."),
                _ => {}
            }
            match error.status {
                426 => CliError::hint(message, "Run `nexus update`."),
                401 => CliError::hint(
                    format!("Nexus Gateway at {url} did not accept your API key."),
                    format!(
                        "Run `nexus auth login --url {url}` to sign in again (in CI: `--with-token`, with an API key on stdin)."
                    ),
                ),
                502 | 504 => CliError::hint(
                    message,
                    "A module or provider behind Gateway did not answer; check that every service is running (`nexus modules`).",
                ),
                _ => CliError::new(message),
            }
        }
        Error::Transport(Transport::Unreachable(cause)) if cause.to_lowercase().contains("certificate") => {
            CliError::hint(
                format!("Nexus Gateway at {url} sent a TLS certificate this computer does not trust."),
                "A proxy or DNS filter on your network may be intercepting HTTPS; try another network, or ask your IT team to allow this Gateway.",
            )
        }
        Error::Transport(Transport::Unreachable(_)) if is_local(url) => CliError::hint(
            format!("Cannot reach Nexus Gateway at {url}."),
            "Is it running? Start your local Gateway, or point the CLI elsewhere with `nexus auth login --url <core>`.",
        ),
        Error::Transport(Transport::Unreachable(_)) => CliError::hint(
            format!("Cannot reach Nexus Gateway at {url}."),
            "Check the URL and your network connection, or point the CLI elsewhere with `nexus auth login --url <core>`.",
        ),
        Error::Transport(Transport::Timeout) => CliError::hint(
            format!("Nexus Gateway at {url} did not answer in time."),
            "Try again; if it persists, check Gateway's logs.",
        ),
        Error::Transport(Transport::Other(message)) => CliError::new(message.clone()),
    }
}

/// A Gateway on this machine: only then do hints tell the person to start it.
pub fn is_local(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(String::from))
        .is_some_and(|host| matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0"))
}

/// The error as the person reads it. A message can carry text that another person wrote (a name, a path), and a terminal
/// runs what it reads, so each control and invisible character shows as its code (`safe_text.rs`).
pub fn error_text(style: &Style, error: &CliError) -> String {
    let mut text = format!(
        "{} {} {}",
        style.fail,
        style.red(&style.bold("Error:")),
        code(style, &safe_text(&error.message))
    );
    if let Some(hint) = &error.hint {
        text.push('\n');
        text.push_str(&hint_text(style, hint));
    }
    text
}

pub(crate) fn hint_text(style: &Style, hint: &str) -> String {
    format!("  {} {}", style.arrow, code(style, &safe_text(hint)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_error(status: u16, message: &str, code: &str) -> CoreError {
        CoreError {
            status,
            message: message.into(),
            code: Some(code.into()),
            interval: None,
            details: None,
        }
    }

    fn core(status: u16, message: &str, code: &str) -> Error {
        Error::Core(Box::new(core_error(status, message, code)))
    }

    #[test]
    fn a_paused_sign_in_says_how_long_to_wait() {
        let url = "https://gateway.test";
        let mut paused = core_error(503, "sign-in is paused", "cutover_paused");
        paused.interval = Some(600.0);
        let error = to_cli_error(&Error::Core(Box::new(paused)), url);
        assert_eq!(
            (error.message.as_str(), error.hint, error.code),
            (
                "Sign-in is paused for maintenance; try again in 10 minutes.",
                None,
                Some("cutover_paused")
            )
        );
        // Any other 503 reads as before.
        let other = to_cli_error(&core(503, "Gateway is draining", "draining"), url);
        assert_eq!((other.message.as_str(), other.code), ("Gateway is draining", None));
    }

    #[test]
    fn names_who_can_fix_a_usage_limit_or_a_failed_charge_after_the_fact() {
        let url = "http://core.test";
        let limit = to_cli_error(
            &core(402, "this month's usage limit is reached", "usage_limit_reached"),
            url,
        );
        assert_eq!(limit.hint.as_deref(), Some(crate::billing::USAGE_LIMIT_HINT));
        let failed = to_cli_error(&core(402, "the last charge failed", "payment_failed"), url);
        assert_eq!(failed.hint.as_deref(), Some(crate::billing::PAYMENT_FAILED_HINT));
        // A charge still going through stops the card until it is paid.
        let charging = to_cli_error(&core(402, "a charge is still going through", "charge_processing"), url);
        assert_eq!(charging.hint.as_deref(), Some(CHARGE_PROCESSING_HINT));
        // The same words as every app: the billing glossary's, then the CLI's action.
        let fixture: Value =
            serde_json::from_str(include_str!("../../../packages/kit/src/fixtures/billing-words.json"))
                .expect("fixture");
        let words = &fixture["chargeProcessing"];
        let text = |key: &str| words[key].as_str().expect("a string").to_string();
        assert_eq!(
            CHARGE_PROCESSING_HINT,
            format!("{}. {} {}", text("title"), text("body"), text("cliAction"))
        );
        // Credits short of a call: the glossary's refusal title first, as every app says it.
        let credits = fixture["refusals"]
            .as_array()
            .expect("refusals")
            .iter()
            .find(|row| row["code"] == "credit_exhausted")
            .and_then(|row| row["title"].as_str())
            .expect("credit_exhausted's title");
        assert!(
            CREDITS_OUT_HINT.starts_with(&format!("{credits}. An owner buys Nexus credits")),
            "{CREDITS_OUT_HINT}"
        );
    }

    #[test]
    fn names_the_fix_for_each_reason_nexus_cannot_pay() {
        // A card the bank wants confirmed, credits out, and the free start used up or for cheaper models only.
        let url = "http://core.test";
        let with = |code: &str, details: Value| {
            Error::Core(Box::new(CoreError {
                details: Some(details),
                ..core_error(402, "refused", code)
            }))
        };
        let confirm = to_cli_error(
            &with(
                "payment_failed",
                serde_json::json!({ "reason": "authentication_required" }),
            ),
            url,
        );
        assert_eq!(confirm.hint.as_deref(), Some(CARD_CONFIRM_HINT));
        assert_eq!(
            to_cli_error(&core(402, "refused", "credit_exhausted"), url)
                .hint
                .as_deref(),
            Some(CREDITS_OUT_HINT)
        );
        let used = to_cli_error(
            &with(
                "subsidy_exhausted",
                serde_json::json!({ "next": ["billing", "own_key"] }),
            ),
            url,
        );
        assert_eq!(used.hint.as_deref(), Some(FREE_START_HINT));
        // A Gateway that does not bill names no billing command.
        assert_eq!(to_cli_error(&core(402, "refused", "subsidy_exhausted"), url).hint, None);
        let cheaper = to_cli_error(
            &with(
                "subsidy_exhausted",
                serde_json::json!({ "freeStart": "cheaper_models_only" }),
            ),
            url,
        );
        assert_eq!(cheaper.hint.as_deref(), Some(CHEAPER_MODELS_HINT));
    }

    #[test]
    fn names_the_fix_for_budgets_modules_and_keys() {
        let url = "http://core.test";
        let budget = to_cli_error(&core(402, "this call may cost up to $0.01", "budget_exceeded"), url);
        assert!(budget.hint.as_deref().is_some_and(|hint| hint.contains("nexus spend")));
        let team = to_cli_error(
            &Error::Core(Box::new(CoreError {
                details: Some(serde_json::json!({ "level": "team", "remainingUsd": 0.0 })),
                ..core_error(402, "this call may cost up to $0.01", "budget_exceeded")
            })),
            url,
        );
        let hint = team.hint.as_deref().unwrap_or_default();
        assert!(hint.contains("nexus budget show team/<slug>") && !hint.contains("nexus spend"));
        let billing = to_cli_error(
            &core(502, "Anthropic: credit balance is too low", "upstream_billing"),
            url,
        );
        let hint = billing.hint.as_deref().unwrap_or_default();
        assert!(
            hint.contains("not your Nexus budget") && !hint.contains("nexus spend"),
            "{hint}"
        );
        let auth = to_cli_error(&core(502, "Anthropic: invalid x-api-key", "upstream_auth"), url);
        let hint = auth.hint.as_deref().unwrap_or_default();
        assert!(
            hint.contains("not your Nexus API key") && !hint.contains("nexus auth login"),
            "{hint}"
        );
        let forbidden = to_cli_error(
            &core(502, "Anthropic: no access to this model", "upstream_forbidden"),
            url,
        );
        let hint = forbidden.hint.as_deref().unwrap_or_default();
        assert!(
            hint.contains("not for your Nexus API key")
                && hint.contains("another model")
                && !hint.contains("nexus spend")
                && !hint.contains("nexus auth login"),
            "{hint}"
        );
        let module = to_cli_error(
            &core(502, "Nexus Review did not answer: fetch failed", "module_unavailable"),
            url,
        );
        assert_eq!(module.message, "Nexus Review did not answer: fetch failed");
        assert!(
            module
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("nexus modules") && !hint.contains("local Gateway"))
        );
        let local = to_cli_error(
            &core(502, "Nexus Review did not answer: fetch failed", "module_unavailable"),
            "http://localhost:3100",
        );
        assert!(local.hint.as_deref().is_some_and(|hint| hint.contains("NEXUS_MODULES")));
        let key = to_cli_error(&core(401, "invalid API key", "authentication_error"), url);
        assert_eq!(
            key.message,
            "Nexus Gateway at http://core.test did not accept your API key."
        );
        let old = to_cli_error(&core(426, "Run `nexus update`.", "cli_too_old"), url);
        assert_eq!(
            old.hint.as_deref(),
            Some("Not installed with Nexus's installer? Reinstall: `curl -fsSL http://core.test/install.sh | bash`.")
        );
        let style = Style::new(false);
        assert_eq!(
            error_text(&style, &CliError::hint("Bad.", "Fix it.")),
            "✕ Error: Bad.\n  → Fix it."
        );
    }

    #[test]
    fn a_budget_refusal_says_who_can_raise_it_and_whether_they_were_told() {
        let refused = |level: &str, notified: Option<bool>| {
            let mut details = serde_json::json!({ "level": level });
            if let Some(notified) = notified {
                details["notified"] = notified.into();
            }
            let error = Error::Core(Box::new(CoreError {
                details: Some(details),
                ..core_error(402, "this call may cost up to $0.01", "budget_exceeded")
            }));
            to_cli_error(&error, "http://core.test").hint.unwrap_or_default()
        };
        let told = "Your organization's owners and admins were notified.";
        for (level, raise) in [
            ("person", "an owner, or an admin an owner allows, can raise it."),
            ("team", "An owner or admin can raise the team's cap."),
            ("organization", "its owner or an admin can raise it."),
        ] {
            let hint = refused(level, Some(true));
            assert!(hint.ends_with(&format!("{raise} {told}")), "{level}: {hint}");
            for untold in [refused(level, None), refused(level, Some(false))] {
                assert!(
                    untold.ends_with(raise) && !untold.contains("notified"),
                    "{level}: {untold}"
                );
            }
        }
    }

    #[test]
    fn names_the_fix_for_the_gateways_rate_limits() {
        // Core's body carries `code` = `stable_code()` (`core_body` in nexus-gateway): `rate_limited` for a 429 with no
        // code of its own (`nexus_base::error::default_code`); `to_cli_error` matches on it.
        let limited = |message: &str, code: &str| {
            let body = serde_json::json!({ "error": { "message": message, "code": code } });
            Error::Core(Box::new(nexus_client::error::core_error(429, &body)))
        };
        let per_minute = limited("more than 120 requests a minute; slow down", "rate_limited");
        let in_flight = limited("4 requests in flight", "concurrency_limit");
        let openai = core(429, "slow down", "rate_limit_error");
        let minute = Some("Wait about a minute and try again.");
        for (error, code, hint) in [
            (per_minute, "rate_limited", minute),
            (in_flight, "concurrency_limit", Some("Retry in a few seconds.")),
            (openai, "rate_limit_error", minute),
        ] {
            let Error::Core(core) = &error else { unreachable!() };
            assert_eq!(core.code.as_deref(), Some(code));
            assert_eq!(to_cli_error(&error, "http://core.test").hint.as_deref(), hint, "{code}");
        }
    }

    /// Gateway's `error.nexus` for 403 `write_consent_required` (the shape kit's `agent-writes.test.ts` reads).
    fn consent_details() -> Value {
        serde_json::json!({
            "connector": "microsoft-todo",
            "account": "ada@acme.test",
            "scopes": ["Tasks.ReadWrite"],
            "consent": { "method": "POST", "path": "/v1/connections/microsoft-todo/oauth", "body": { "scope": "user", "access": "write" } },
        })
    }

    fn consent_refusal(details: Value) -> Error {
        Error::Core(Box::new(CoreError {
            details: Some(details),
            ..core_error(
                403,
                "microsoft-todo was connected to read only: its holder allows writes at the vendor first",
                "write_consent_required",
            )
        }))
    }

    #[test]
    fn a_write_that_needs_consent_names_the_account_its_scopes_and_the_command_that_allows_it() {
        let url = "http://core.test";
        let failure = to_cli_error(&consent_refusal(consent_details()), url);
        assert_eq!(
            failure.message,
            "microsoft-todo account ada@acme.test was connected to read only: this write needs Tasks.ReadWrite. Nothing was written."
        );
        assert_eq!(
            failure.hint.as_deref(),
            Some(
                "Allow writes with `nexus connect microsoft-todo --write`, open the link it prints, then run this command again."
            )
        );
        let mut org = consent_details();
        org["consent"]["body"]["scope"] = "org".into();
        let hint = to_cli_error(&consent_refusal(org), url).hint.unwrap_or_default();
        assert!(hint.contains("`nexus connect microsoft-todo --org --write`"), "{hint}");
    }

    #[test]
    fn a_consent_request_other_than_the_contracts_call_keeps_gateways_message() {
        let base = consent_details();
        let with = |pointer: &str, value: Value| {
            let mut details = base.clone();
            *details.pointer_mut(pointer).expect("field") = value;
            details
        };
        for details in [
            with("/consent/path", "/v1/connections/gmail/oauth".into()),
            with(
                "/consent/path",
                "https://evil.test/v1/connections/microsoft-todo/oauth".into(),
            ),
            with("/consent/method", "DELETE".into()),
            with("/consent/body/access", "read".into()),
            with("/consent/body/scope", "team".into()),
            with("/connector", "../admin".into()),
            with("/scopes", serde_json::json!([1])),
            with("/account", Value::Null),
            with("/consent", Value::Null),
        ] {
            assert_eq!(nexus_client::error::write_consent_of(&details), None, "{details}");
            let failure = to_cli_error(&consent_refusal(details), "http://core.test");
            assert!(
                failure.message.starts_with("microsoft-todo was connected to read only") && failure.hint.is_none(),
                "{failure:?}"
            );
        }
        // No details, or the code on another status: the message as Gateway gave it.
        let bare = Error::Core(Box::new(core_error(403, "refused", "write_consent_required")));
        assert_eq!(to_cli_error(&bare, "http://core.test"), CliError::new("refused"));
        let Error::Core(mut other) = consent_refusal(consent_details()) else {
            unreachable!()
        };
        other.status = 400;
        assert!(to_cli_error(&Error::Core(other), "http://core.test").hint.is_none());
    }

    #[test]
    fn names_dev_commands_only_for_a_local_gateway() {
        let down = || {
            Error::Transport(Transport::Unreachable(
                "error sending request: Connection refused".into(),
            ))
        };
        let hint = |url| to_cli_error(&down(), url).hint.unwrap_or_default();
        assert!(hint("http://localhost:3100").contains("Start your local Gateway"));
        assert!(!hint("https://gateway.example.test").contains("local Gateway"));
        let tls = Error::Transport(Transport::Unreachable(
            "error sending request: client error (Connect): invalid peer certificate: UnknownIssuer".into(),
        ));
        assert!(
            to_cli_error(&tls, "https://gateway.example.test")
                .message
                .contains("certificate")
        );
    }
}
