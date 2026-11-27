//! A product the CLI cannot find while a module is not answering (`unansweredError` in `discovery.ts`).
//!
//! `GET /v1/modules` lists the modules that answered in `modules`, and the others in `unavailable` with their URL
//! and error. The Gateway knows only the URL of a module that did not answer, not its product. So when the product is
//! missing and a module is down, the error says the down module may serve it, instead of "unknown product".

use serde_json::Value;

use crate::error::CliError;
use crate::json::js_string;

/// The hint on a Gateway on this machine, where the user can start the module.
pub const LOCAL_HINT: &str = "Start the module and check that NEXUS_MODULES on Gateway lists its URL.";
/// The hint elsewhere: look, then retry.
pub const REMOTE_HINT: &str = "Run `nexus modules` to check, then run the command again.";

/// The error for `product`, missing from `known`, when `unavailable` (`[{url, error}]`) is not empty; `None` when
/// every module answered, so the caller keeps its "unknown command or product" error.
pub fn unanswered_error(product: &str, known: &[String], unavailable: &[Value], local: bool) -> Option<CliError> {
    if unavailable.is_empty() {
        return None;
    }
    let count = unavailable.len();
    let modules = if count == 1 {
        "1 module is".to_string()
    } else {
        format!("{count} modules are")
    };
    // A near miss of a listed product is most likely a typo, so the down modules come second.
    let mut lines = match closest(product, known) {
        Some(close) => vec![
            format!("Unknown product \"{product}\". Did you mean \"{close}\"?"),
            format!("Also, {modules} not answering:"),
        ],
        None => vec![format!(
            "Can't reach the {} that may serve \"{product}\": {modules} not answering.",
            if count == 1 { "module" } else { "modules" }
        )],
    };
    for entry in unavailable {
        lines.push(format!(
            "    {}  {}",
            js_string(entry.get("url")),
            js_string(entry.get("error"))
        ));
    }
    Some(CliError::hint(
        lines.join("\n"),
        if local { LOCAL_HINT } else { REMOTE_HINT },
    ))
}

/// The known product within two edits of `product`, the closest first; ties keep the listing's order.
fn closest<'a>(product: &str, known: &'a [String]) -> Option<&'a str> {
    known
        .iter()
        .map(|slug| (distance(product, slug), slug))
        .filter(|(edits, _)| *edits <= 2)
        .min_by_key(|(edits, _)| *edits)
        .map(|(_, slug)| slug.as_str())
}

/// Levenshtein distance, by characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn down(url: &str, error: &str) -> Value {
        json!({ "url": url, "error": error })
    }

    #[test]
    fn a_down_module_is_named_instead_of_unknown_product() {
        let unavailable = [down("https://review.example.test", "connection refused")];
        let error = unanswered_error("robustness", &[], &unavailable, false).expect("an error");
        assert_eq!(
            error.message,
            "Can't reach the module that may serve \"robustness\": 1 module is not answering.\n    https://review.example.test  connection refused"
        );
        assert_eq!(error.hint.as_deref(), Some(REMOTE_HINT));
        assert_eq!(error.exit_code, 1);
    }

    #[test]
    fn every_module_answering_keeps_the_unknown_product_error() {
        assert_eq!(
            unanswered_error("robustness", &["intelligence".into()], &[], false),
            None
        );
    }

    #[test]
    fn several_down_modules_are_listed_and_a_local_gateway_says_to_start_them() {
        let unavailable = [
            down("http://localhost:4001", "timed out"),
            down("http://localhost:4002", "refused"),
        ];
        let error = unanswered_error("robustness", &[], &unavailable, true).expect("an error");
        assert!(
            error
                .message
                .starts_with("Can't reach the modules that may serve \"robustness\": 2 modules")
        );
        assert!(
            error
                .message
                .ends_with("http://localhost:4001  timed out\n    http://localhost:4002  refused")
        );
        assert_eq!(error.hint.as_deref(), Some(LOCAL_HINT));
    }

    #[test]
    fn a_near_miss_of_a_listed_product_asks_did_you_mean_first() {
        let unavailable = [down("https://review.example.test", "connection refused")];
        let known = ["intelligence".to_string()];
        let error = unanswered_error("inteligence", &known, &unavailable, false).expect("an error");
        assert!(error.message.starts_with(
            "Unknown product \"inteligence\". Did you mean \"intelligence\"?\nAlso, 1 module is not answering:\n"
        ));
        assert!(
            unanswered_error("robustness", &known, &unavailable, false)
                .is_some_and(|error| !error.message.contains("Did you mean"))
        );
    }

    #[test]
    fn distance_counts_edits() {
        assert_eq!(distance("review", "review"), 0);
        assert_eq!(distance("reveiw", "review"), 2);
        assert_eq!(distance("", "abc"), 3);
    }
}
