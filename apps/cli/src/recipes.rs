//! Recipes (the web kit's `recipes.ts`): the `nexus-recipe/1` files under `kit/recipes/`,
//! carried in the binary with their runs (`kit/recipes/runs.json`, `carriedRuns`), and what `nexus agents setup` prints
//! from them. The binary carries the runs' summary, not the records in `kit/recipes/runs/`: those hold production ledger
//! ids and stay private. The TypeScript checks every recipe and run when the kit builds (`recipeProblems`, `runProblems`), and
//! `tests/recipes.rs` holds this output to the generated docs page.

use serde::{Deserialize, Serialize};

use nexus_client::agent_entries::Agent;

/// The files, in `RECIPES`' order.
const FILES: [&str; 2] = [
    include_str!("../../../packages/kit/recipes/claude-code.json"),
    include_str!("../../../packages/kit/recipes/codex.json"),
];

/// The launchers' runs, in `RUNS`' order (oldest first), as `carriedRuns` writes them.
pub const RUNS_FILE: &str = include_str!("../../../packages/kit/recipes/runs.json");

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recipe {
    pub id: String,
    /// The catalog's agent, by its id: a recipe for an agent the catalog does not name does not parse.
    pub agent: Agent,
    pub title: String,
    pub launch: Launch,
    pub git_repo: Option<GitRepo>,
    pub limits: Vec<String>,
    pub steps: Vec<Step>,
    pub verify: Verify,
    pub test: Test,
    pub claim: Claim,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub base_url_path: String,
    pub credential_variable: String,
    pub removed_variables: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepo {
    pub outside_flag: String,
}

/// A step as the recipe has it, and as `--json` prints it: `note` is set only on output.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Step {
    pub id: String,
    pub who: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Verify {
    pub text: String,
    pub command: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Test {
    pub features: Vec<String>,
}

/// A run (`nexus-recipe-run/1`), the fields the page shows.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub recipe: String,
    pub date: String,
    pub tool_release: String,
    pub cli_release: String,
    pub seconds_to_first_call: Option<f64>,
    pub steps: Vec<RunStep>,
    pub result: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunStep {
    pub id: String,
    pub result: String,
}

/// `CONDITIONAL_STEPS` (`recipe-runs.ts`): the only steps a backing run may skip.
const CONDITIONAL_STEPS: [&str; 1] = ["provider_key"];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub state: String,
    pub register_row: Option<String>,
}

/// Every recipe this CLI carries, or what is wrong with the first that does not parse.
pub fn recipes() -> Result<Vec<Recipe>, String> {
    FILES
        .iter()
        .map(|file| serde_json::from_str(file).map_err(|error| error.to_string()))
        .collect()
}

/// The recipe's runs, oldest first; none when the file does not parse (the kit's check writes it).
pub fn runs_of(recipe: &Recipe) -> Vec<Run> {
    serde_json::from_str::<Vec<Run>>(RUNS_FILE)
        .unwrap_or_default()
        .into_iter()
        .filter(|run| run.recipe == recipe.id)
        .collect()
}

/// The agent's name in the catalog (`Claude Code`), as the page says it.
fn name(recipe: &Recipe) -> String {
    recipe.agent.entry().identity.name.to_string()
}

/// `recipeLimits`: the recipe's own, then what its fields imply.
pub fn limits(recipe: &Recipe) -> Vec<String> {
    let mut limits = recipe.limits.clone();
    if let Some(git) = &recipe.git_repo {
        limits.push(format!(
            "{} refuses to start outside a git repository. Run it from inside one, or add `{}` at the end of the command.",
            name(recipe),
            git.outside_flag
        ));
    }
    limits
}

/// `minutes` (`recipes.ts`): `4m 12s`, or `48s` under a minute, the total rounded first so it never prints 60 seconds.
fn minutes(seconds: f64) -> String {
    let total = seconds.round() as u64;
    if total < 60 {
        format!("{total}s")
    } else {
        format!("{}m {:02}s", total / 60, total % 60)
    }
}

/// `backingRun`: the latest run, if it passed with only conditional steps skipped. A later failing run drops the claim.
fn backing_run(runs: &[Run]) -> Option<&Run> {
    runs.last().filter(|run| {
        run.result == "pass"
            && run.steps.iter().all(|step| {
                step.result == "done" || (step.result == "skipped" && CONDITIONAL_STEPS.contains(&step.id.as_str()))
            })
    })
}

/// `recipeClaim`: `tested` needs a backing run and the claims register's row, so until the register exists every page
/// says unclaimed.
pub fn claim(recipe: &Recipe) -> &'static str {
    let runs = runs_of(recipe);
    if recipe.claim.state == "tested" && recipe.claim.register_row.is_some() && backing_run(&runs).is_some() {
        "tested"
    } else {
        "unclaimed"
    }
}

/// `recipeStatus`: what the recorded runs back, and nothing more.
pub fn status(recipe: &Recipe) -> String {
    let runs = runs_of(recipe);
    let Some(last) = backing_run(&runs) else {
        return "No recorded run backs this page yet, so it makes no claim about the tool.".into();
    };
    let seen = format!(
        "{} on {}, {} {} and nexus {}, reached a first call in {}",
        last.id,
        last.date,
        name(recipe),
        last.tool_release,
        last.cli_release,
        minutes(last.seconds_to_first_call.unwrap_or_default())
    );
    if claim(recipe) == "tested" {
        format!("Tested: the latest run, {seen}.")
    } else {
        format!(
            "A recorded run, {seen}, but no claims register backs this page yet, so it makes no claim about the tool."
        )
    }
}

/// `recipeStepNote`: the `run` step of a tool that needs a git repository.
pub fn step_note(recipe: &Recipe, step: &Step) -> Option<String> {
    // A command that already carries the flag needs no note.
    let git = recipe.git_repo.as_ref().filter(|git| {
        step.id == "run"
            && !step
                .command
                .as_deref()
                .is_some_and(|command| command.contains(&git.outside_flag))
    })?;
    Some(format!(
        "Outside a git repository, add `{}` at the end.",
        git.outside_flag
    ))
}

/// `recipeChanges`: no file, and the variables the launcher sets and removes for one run.
pub fn changes(recipe: &Recipe) -> String {
    let name = name(recipe);
    let launch = &recipe.launch;
    let mut text = format!(
        "For each run with `--api`, Nexus points {name} at your Gateway's `{}` with your Nexus sign-in as `{}`, and changes no file.",
        launch.base_url_path, launch.credential_variable
    );
    if !launch.removed_variables.is_empty() {
        let removed: Vec<String> = launch
            .removed_variables
            .iter()
            .map(|variable| format!("`{variable}`"))
            .collect();
        text.push_str(&format!(
            " It removes {} from {name}'s environment, so its calls can only go through Nexus.",
            removed.join(", ")
        ));
    }
    text
}

/// `recipeSetup`'s steps and check: `%CORE_URL%` is the Gateway URL with no trailing slash, and each step's note set.
pub fn setup(recipe: &Recipe, core_url: &str) -> (Vec<Step>, Verify) {
    let url = |text: &str| text.replace("%CORE_URL%", core_url.trim_end_matches('/'));
    let steps = recipe
        .steps
        .iter()
        .map(|step| Step {
            command: step.command.as_deref().map(url),
            note: step_note(recipe, step),
            ..step.clone()
        })
        .collect();
    let verify = Verify {
        text: recipe.verify.text.clone(),
        command: url(&recipe.verify.command),
    };
    (steps, verify)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each carried recipe's agent has a Gateway route, since a recipe runs on that route. An agent id the catalog does
    /// not name does not parse.
    #[test]
    fn recipes_name_the_catalogs_agents() {
        for recipe in recipes().expect("the recipes parse") {
            assert!(recipe.agent.entry().billing.gateway.is_some(), "{}", recipe.id);
        }
        let mut unknown: serde_json::Value = serde_json::from_str(FILES[0]).expect("JSON");
        unknown["agent"] = serde_json::json!("copilot");
        let error = serde_json::from_value::<Recipe>(unknown).expect_err("an unknown agent");
        assert!(error.to_string().contains("copilot"), "{error}");
    }
}
