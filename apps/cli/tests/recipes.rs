//! `nexus agents setup <agent>` (Integrations Capabilities C1): it prints what the agent's generated docs page says, in
//! the page's order, limits first, as the TypeScript CLI does (`agent-routes.test.ts`). A recipe that changes without
//! its page, or a CLI that drifts from either, fails here.

use std::collections::HashMap;

use nexus_cli::recipes::{RUNS_FILE, limits, recipes, runs_of};
use nexus_cli::{Capture, Captured, Context, run_cli};
use serde_json::Value;

const CORE: &str = "https://core.example";

/// The committed pages, by recipe id.
const PAGES: [(&str, &str); 2] = [
    (
        "claude-code",
        include_str!("../../../packages/kit/recipes/pages/claude-code.mdx"),
    ),
    ("codex", include_str!("../../../packages/kit/recipes/pages/codex.mdx")),
];

struct Run {
    code: i32,
    out: Vec<String>,
    err: String,
}

fn setup(argv: &[&str]) -> Run {
    let dir = std::env::temp_dir().join(format!("nexus-setup-{}-{}", std::process::id(), argv.join("-")));
    std::fs::create_dir_all(&dir).expect("dir");
    let config_path = dir.join("config.json");
    std::fs::write(&config_path, format!("{{\"url\":\"{CORE}\"}}")).expect("config");
    let capture = Capture(std::rc::Rc::new(Captured::default()));
    let ctx = Context {
        config_path,
        io: Box::new(capture.clone()),
        color: false,
        env: HashMap::new(),
        poll_ms: Some(0),
        hostname: None,
        cwd: dir.clone(),
        interactive: false,
        exe_path: None,
        columns: None,
        picker: false,
    };
    let argv: Vec<String> = argv.iter().map(|arg| arg.to_string()).collect();
    let code = run_cli(&argv, &ctx);
    let _ = std::fs::remove_dir_all(&dir);
    Run {
        code,
        out: capture.0.out.borrow().clone(),
        err: capture.0.err.borrow().join("\n"),
    }
}

/// The page's setup, line by line without its markup: the title, the status, the sections until "How it is tested".
fn page_lines(id: &str) -> Vec<String> {
    let page = PAGES
        .iter()
        .find(|(page, _)| *page == id)
        .expect("a page for every recipe")
        .1;
    let mut parts = page.splitn(3, "---\n");
    let (_, front, body) = (
        parts.next(),
        parts.next().expect("front matter"),
        parts.next().expect("body"),
    );
    let title = front
        .lines()
        .find_map(|line| line.strip_prefix("title: "))
        .expect("title");
    let title: String = serde_json::from_str(title).expect("a quoted title");
    let body = &body[..body.find("## How it is tested").expect("the tests section")];
    std::iter::once(title)
        .chain(body.lines().map(str::trim).filter_map(|line| {
            let markup =
                line.starts_with("```") || line == "</Callout>" || (line.starts_with("{/*") && line.ends_with("*/}"));
            if markup || line.is_empty() {
                return None;
            }
            let line = line
                .strip_prefix("<Callout title=\"")
                .and_then(|l| l.strip_suffix("\">"))
                .unwrap_or(line);
            let line = line
                .strip_prefix("## ")
                .or_else(|| line.strip_prefix("- "))
                .unwrap_or(line);
            Some(
                line.replace("**Owner or admin.**", "Owner or admin.")
                    .replace("%CORE_URL%", CORE),
            )
        }))
        .collect()
}

fn cli_lines(out: &[String]) -> Vec<String> {
    out.iter()
        .map(|line| line.trim())
        .map(|line| {
            line.strip_prefix("· ")
                .or_else(|| line.strip_prefix("- "))
                .unwrap_or(line)
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

#[test]
fn each_agent_prints_what_its_generated_page_says_in_its_order() {
    let all = recipes().expect("the recipes parse");
    assert_eq!(
        all.len(),
        PAGES.len(),
        "a page for every recipe, and a recipe for every page"
    );
    for recipe in &all {
        let run = setup(&["agents", "setup", recipe.agent.as_str()]);
        assert_eq!(run.code, 0, "{}: {}", recipe.agent, run.err);
        assert_eq!(cli_lines(&run.out), page_lines(&recipe.id), "{}", recipe.agent);
        assert!(run.out.iter().all(|line| !line.to_lowercase().contains("works with")));
    }
}

#[test]
fn the_limits_come_before_any_step() {
    for recipe in recipes().expect("the recipes parse") {
        let run = setup(&["agents", "setup", recipe.agent.as_str()]);
        let steps = run.out.iter().position(|line| line == "Set up and run").expect("steps");
        for limit in limits(&recipe) {
            let at = run
                .out
                .iter()
                .position(|line| line.ends_with(&limit))
                .expect("the limit is printed");
            assert!(at < steps, "{}: {limit}", recipe.agent);
        }
    }
}

#[test]
fn json_carries_the_same_limits_and_the_commands_for_this_gateway() {
    for recipe in recipes().expect("the recipes parse") {
        let run = setup(&["agents", "setup", recipe.agent.as_str(), "--json"]);
        assert_eq!(run.code, 0, "{}", run.err);
        let answer: Value = serde_json::from_str(&run.out.join("\n")).expect("json");
        assert_eq!(answer["limits"], serde_json::json!(limits(&recipe)));
        assert_eq!(answer["claim"], "unclaimed");
        assert!(!answer.to_string().contains("%CORE_URL%"));
        assert_eq!(
            answer["steps"][0]["command"],
            format!("curl -fsSL {CORE}/install.sh | bash")
        );
        let ids: Vec<&str> = answer["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .filter_map(|step| step["id"].as_str())
            .collect();
        let expected: Vec<&str> = recipe.steps.iter().map(|step| step.id.as_str()).collect();
        assert_eq!(ids, expected);
        let keys: Vec<&String> = answer.as_object().expect("object").keys().collect();
        assert_eq!(
            keys,
            [
                "recipe", "agent", "title", "claim", "status", "limits", "steps", "verify", "changes"
            ]
        );
    }
}

#[test]
fn an_agent_without_a_recipe_or_no_agent_is_a_usage_error() {
    let none = setup(&["agents", "setup", "cursor"]);
    assert_eq!(none.code, 2);
    assert!(none.out.is_empty());
    assert!(none.err.contains("\"cursor\" has no recipe."), "{}", none.err);
    assert!(none.err.contains("Recipes: claude, codex."), "{}", none.err);
    let missing = setup(&["agents", "setup"]);
    assert_eq!(missing.code, 2);
    assert!(
        missing
            .err
            .contains("Name an agent: nexus agents setup <claude|codex>."),
        "{}",
        missing.err
    );
}

#[test]
fn the_binary_carries_every_run_and_no_ledger_id_and_says_unclaimed_without_the_register() {
    let all = recipes().expect("the recipes parse");
    // `kit/recipes/runs.json` holds the launchers' runs (`carriedRuns`): each one parses, and none names a ledger call
    // or an agent session. The kit's test checks the file against the private records.
    let file: Vec<Value> = serde_json::from_str(RUNS_FILE).expect("kit/recipes/runs.json");
    assert!(!file.is_empty());
    let carried: usize = all.iter().map(|recipe| runs_of(recipe).len()).sum();
    assert_eq!(
        carried,
        file.len(),
        "a run in kit/recipes/runs.json that does not parse or names no carried recipe"
    );
    for id in ["spd_", "sess_", "firstCall", "agentSession"] {
        assert!(!RUNS_FILE.contains(id), "kit/recipes/runs.json names {id}");
    }
    for recipe in &all {
        let run = setup(&["agents", "setup", recipe.agent.as_str(), "--json"]);
        let answer: Value = serde_json::from_str(&run.out.join("\n")).expect("json");
        // A passing run alone is not a claim: the register's row is null.
        assert!(recipe.claim.register_row.is_none());
        assert_eq!(answer["claim"], "unclaimed");
        assert!(
            answer["status"]
                .as_str()
                .is_some_and(|status| !status.starts_with("Tested")),
            "{}",
            answer["status"]
        );
    }
}
