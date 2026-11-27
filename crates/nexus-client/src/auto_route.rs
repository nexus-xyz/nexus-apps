//! Auto for a coding agent's session before it starts, as the desktop app asks Gateway (`POST /v1/agents/route`): the
//! plan's room per model as this computer last read it, optional shape counts of the first message (never its text),
//! and Gateway's answer.

use serde_json::{Value, json};

use crate::Client;
use crate::local_agents::names_model;

/// How much of a plan's limits is left for one model. It lives with the provider layer's values; `as_str` is the word
/// this request sends.
pub use nexus_providers::Room;

/// One limit window of a plan, as its agent reported it.
#[derive(Debug, Clone, Copy)]
pub struct Window<'a> {
    /// `Session`, `Weekly`, `Weekly · Fable`, `Weekly · Gemini`.
    pub label: &'a str,
    /// The models a group limit covers (Antigravity); empty for a limit of the whole plan or of one named model.
    pub models: &'a [String],
    /// 0 to 100.
    pub used_percent: f64,
    /// Epoch milliseconds.
    pub resets_at: Option<i64>,
}

/// The windows of `windows` that bind `model` (its display name, `Fable 5.2`): the plan's own Session and Weekly, a
/// `Weekly · <name>` limit that names it, and a group limit whose models name it.
fn binds(window: &Window<'_>, model: &str) -> bool {
    if !window.models.is_empty() {
        return names_model(window.models, model);
    }
    match window.label.split_once('·') {
        Some((_, scoped)) => names_model(&[scoped.trim().to_string()], model),
        None => true,
    }
}

/// The room `model` has on a plan with `windows`, at `now`. A window past its reset has room again. A plan with no
/// window that binds the model, or whose windows read no number, is `Unknown`.
pub fn room(windows: &[Window<'_>], model: &str, now: i64) -> Room {
    let binding: Vec<&Window<'_>> = windows
        .iter()
        .filter(|window| binds(window, model))
        .filter(|window| window.resets_at.is_none_or(|at| at > now))
        .filter(|window| window.used_percent.is_finite())
        .collect();
    if binding.is_empty() {
        return Room::Unknown;
    }
    if binding.iter().any(|window| window.used_percent >= 100.0) {
        Room::UsedUp
    } else {
        Room::Left
    }
}

/// What Gateway answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// Run `model` (the agent's own id) on the person's plan, at `effort` when the router names one.
    UsePlan { model: String, effort: Option<String> },
    /// No plan model has room: `model` would run on the organization's API (`access`: `byok` or `nexus`).
    AskToPay {
        model: String,
        name: String,
        access: Option<String>,
        effort: Option<String>,
    },
    /// Nothing can run on Auto; `reason` is Gateway's code.
    Stop { reason: String },
}

/// The first prompt's shape as counts, never its text, as Gateway's router reads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub input_tokens: i64,
    pub code_blocks: u16,
    pub images: bool,
}

impl Counts {
    /// Approximate counts for a first message: a byte-based token estimate, fenced code blocks, and whether images are
    /// attached.
    pub fn of(text: &str, images: bool) -> Self {
        let trimmed = text.trim();
        let input_tokens = if trimmed.is_empty() {
            0
        } else {
            ((trimmed.len() as i64) + 3) / 4
        };
        let fences = text.lines().filter(|line| line.trim_start().starts_with("```")).count();
        let code_blocks = (fences / 2).min(u16::MAX as usize) as u16;
        Self {
            input_tokens,
            code_blocks,
            images,
        }
    }
}

/// The request body: the agent, optional team, optional first-prompt counts, and, for a launch on the person's plan,
/// the one account it starts on with its room per model. `models` are the agent's own ids (`claude-opus-5-5`). A launch
/// through Gateway sends no plan.
pub fn body_with(agent: &str, plan: Option<&[(String, Room)]>, team: Option<&str>, counts: Option<Counts>) -> Value {
    let mut body = json!({ "agent": agent });
    if let Some(team) = team.filter(|team| !team.is_empty()) {
        body["team"] = json!(team);
    }
    if let Some(models) = plan.filter(|models| !models.is_empty()) {
        let models: Vec<Value> = models
            .iter()
            .map(|(id, room)| json!({ "id": id, "room": room.as_str() }))
            .collect();
        body["plans"] = json!([{ "account": 0, "models": models }]);
    }
    if let Some(counts) = counts {
        body["counts"] = json!({
            "inputTokens": counts.input_tokens.max(0),
            "codeBlocks": counts.code_blocks,
            "images": counts.images,
        });
    }
    body
}

/// The request body: the agent, and, for a launch on the person's plan, the one account it starts on with its room per
/// model. `models` are the agent's own ids (`claude-opus-5-5`). A launch through Gateway sends no plan.
pub fn body(agent: &str, plan: Option<&[(String, Room)]>) -> Value {
    body_with(agent, plan, None, None)
}

/// The request body when Auto picks the agent too (`agent: "auto"`): each agent Auto may start, with the room its plan
/// login has per model when the launch is on the person's plan. An agent with no plan reading still competes on Gateway.
/// One agent of an `agent: "auto"` request: its id, and its plan's room per model when the launch is on the plan.
pub type AcrossReading<'a> = (&'a str, Option<&'a [(String, Room)]>);

pub fn body_across(agents: &[AcrossReading<'_>], counts: Option<Counts>) -> Value {
    let names: Vec<&str> = agents.iter().map(|(agent, _)| *agent).collect();
    let plans: Vec<Value> = agents
        .iter()
        .enumerate()
        .filter_map(|(account, (agent, plan))| {
            let models = plan.filter(|models| !models.is_empty())?;
            let models: Vec<Value> = models
                .iter()
                .map(|(id, room)| json!({ "id": id, "room": room.as_str() }))
                .collect();
            Some(json!({ "agent": agent, "account": account, "models": models }))
        })
        .collect();
    let mut body = json!({ "agent": "auto", "agents": names });
    if !plans.is_empty() {
        body["plans"] = json!(plans);
    }
    if let Some(counts) = counts {
        body["counts"] = json!({
            "inputTokens": counts.input_tokens.max(0),
            "codeBlocks": counts.code_blocks,
            "images": counts.images,
        });
    }
    body
}

/// Ask Gateway to pick the agent too: the agent it names (`None` on a stop) and its answer. An error is the caller's to
/// fall back from: a Gateway older than `agent: "auto"` refuses the body with a 400.
pub async fn route_across(client: &Client, body: Value) -> Result<(Option<String>, Answer), String> {
    let answer = client
        .post("/v1/agents/route", body)
        .await
        .map_err(|error| error.to_string())?;
    let agent = answer
        .get("agent")
        .and_then(Value::as_str)
        .filter(|agent| *agent != "auto")
        .map(String::from);
    let parsed = parse(&answer).ok_or_else(|| "Gateway's Auto answer is not one this app reads".to_string())?;
    Ok((agent, parsed))
}

/// Gateway's answer as an [`Answer`]; `None` for a body this client does not know.
pub fn parse(answer: &Value) -> Option<Answer> {
    let text = |key: &str| answer.get(key).and_then(Value::as_str).map(String::from);
    let model = || text("providerModel").filter(|model| !model.is_empty());
    match answer.get("result").and_then(Value::as_str)? {
        "use_plan" => Some(Answer::UsePlan {
            model: model()?,
            effort: text("effort"),
        }),
        "ask_for_paid_fallback" => Some(Answer::AskToPay {
            model: model()?,
            name: text("model").unwrap_or_default(),
            access: text("access"),
            effort: text("effort"),
        }),
        "stop" => Some(Answer::Stop {
            reason: text("reason").unwrap_or_default(),
        }),
        _ => None,
    }
}

/// The models a plan launch offers: each installed model by the id it runs (its resolved id, else its own), the agent's
/// "default" entry and repeats left out, and only ids `known` accepts (Gateway refuses an id its catalog lacks). Each
/// carries its room from `windows`, the plan's last reading. A login this app has not read yet has room, as a launch on
/// the plan has today: its agent reports the limit when it meets one.
pub fn plan_models<'a>(
    installed: impl IntoIterator<Item = (&'a str, Option<&'a str>, &'a str)>,
    known: impl Fn(&str) -> bool,
    windows: Option<&[Window<'_>]>,
    now: i64,
) -> Vec<(String, Room)> {
    let mut models: Vec<(String, Room)> = Vec::new();
    for (id, resolved, name) in installed {
        let runs = resolved.unwrap_or(id);
        if id == "default" || runs.is_empty() || !known(runs) || models.iter().any(|(seen, _)| seen == runs) {
            continue;
        }
        let room = match windows {
            Some(windows) => match room(windows, name, now) {
                Room::Unknown => Room::Left,
                read => read,
            },
            None => Room::Left,
        };
        models.push((runs.to_string(), room));
    }
    models
}

/// Whether Gateway can route a launch at all. On the person's plan with no model Gateway knows, it could offer only a
/// paid model, and the plan launch that runs today would be refused: such a launch starts as before instead.
pub fn routable(on_plan: bool, plan: Option<&[(String, Room)]>) -> bool {
    !on_plan || plan.is_some_and(|models| !models.is_empty())
}

/// What a launch on Auto does with Gateway's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    /// Start the agent on this model (its own id), at the router's effort when it names one.
    Run { model: String, effort: Option<String> },
    /// Do not start: the person must choose. The text says why and what to do.
    Refuse(String),
    /// Start as before Auto routed launches: the organization's default model.
    AsBefore,
}

/// The launch for `answer`. On the person's plan (`on_plan`), a paid model is never started: the person picks the API
/// under Who pays first. Through Gateway the person already chose to pay, so the paid model starts. `plan` names the
/// plan (`Claude`, `ChatGPT`).
pub fn launch(answer: Answer, on_plan: bool, plan: &str) -> Launch {
    launch_with_hint(
        answer,
        on_plan,
        plan,
        "pick your organization under Who pays, or pick a model.",
    )
}

/// As [`launch`], with surface-specific instructions when the plan has no room left.
pub fn launch_with_hint(answer: Answer, on_plan: bool, plan: &str, switch_hint: &str) -> Launch {
    match answer {
        Answer::UsePlan { model, effort } => Launch::Run { model, effort },
        Answer::AskToPay { model, effort, .. } if !on_plan => Launch::Run { model, effort },
        Answer::AskToPay { name, .. } => Launch::Refuse(format!(
            "Your {plan} plan has no room left for this task. Auto would run {} on your organization's API: {switch_hint}",
            name.rsplit('/').next().unwrap_or(&name)
        )),
        Answer::Stop { .. } => Launch::AsBefore,
    }
}

/// Ask Gateway. An error is the caller's to fall back from: a Gateway without the route answers 404.
pub async fn route(client: &Client, body: Value) -> Result<Answer, String> {
    let answer = client
        .post("/v1/agents/route", body)
        .await
        .map_err(|error| error.to_string())?;
    parse(&answer).ok_or_else(|| "Gateway's Auto answer is not one this app reads".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window<'a>(label: &'a str, models: &'a [String], used: f64, resets_at: Option<i64>) -> Window<'a> {
        Window {
            label,
            models,
            used_percent: used,
            resets_at,
        }
    }

    const NOW: i64 = 1_000;

    #[test]
    fn the_plan_s_own_windows_bind_every_model_and_a_scoped_week_binds_its_model_only() {
        let none: [String; 0] = [];
        let windows = [
            window("Session", &none, 40.0, Some(NOW + 1)),
            window("Weekly", &none, 60.0, None),
            window("Weekly · Fable", &none, 100.0, None),
        ];
        assert_eq!(room(&windows, "Opus 5.5", NOW), Room::Left);
        assert_eq!(room(&windows, "Fable 5.2", NOW), Room::UsedUp);
        let session_out = [window("Session", &none, 100.0, Some(NOW + 1))];
        assert_eq!(room(&session_out, "Opus 5.5", NOW), Room::UsedUp);
    }

    #[test]
    fn a_window_past_its_reset_has_room_and_no_reading_is_unknown() {
        let none: [String; 0] = [];
        let reset = [window("Session", &none, 100.0, Some(NOW - 1))];
        assert_eq!(room(&reset, "Opus 5.5", NOW), Room::Unknown);
        assert_eq!(room(&[], "Opus 5.5", NOW), Room::Unknown);
        let unread = [window("Session", &none, f64::NAN, None)];
        assert_eq!(room(&unread, "Opus 5.5", NOW), Room::Unknown);
    }

    #[test]
    fn a_group_limit_binds_the_models_it_names() {
        let gemini = ["Gemini 3.8 Flash".to_string()];
        let windows = [window("Weekly · Gemini", &gemini, 100.0, None)];
        assert_eq!(room(&windows, "Gemini 3.8 Flash", NOW), Room::UsedUp);
        assert_eq!(room(&windows, "Claude Opus 5.5", NOW), Room::Unknown);
    }

    #[test]
    fn the_body_sends_one_account_only_for_a_plan_launch() {
        let models = [("claude-opus-5-5".to_string(), Room::Left)];
        assert_eq!(
            body("claude", Some(&models)),
            json!({ "agent": "claude", "plans": [{ "account": 0, "models": [{ "id": "claude-opus-5-5", "room": "left" }] }] })
        );
        assert_eq!(body("claude", None), json!({ "agent": "claude" }));
        assert_eq!(body("claude", Some(&[])), json!({ "agent": "claude" }));
    }

    #[test]
    fn a_plan_launch_offers_each_model_it_runs_once_and_only_known_ids() {
        let none: [String; 0] = [];
        let windows = [
            window("Weekly", &none, 50.0, None),
            window("Weekly · Fable", &none, 100.0, None),
        ];
        let installed = [
            ("default", Some("claude-opus-5-5"), "Default"),
            ("opus", Some("claude-opus-5-5"), "Opus 5.5"),
            ("claude-opus-5-5", None, "Opus 5.5"),
            ("claude-fable-5-2", None, "Fable 5.2"),
            ("claude-sonnet-9", None, "Sonnet 9"),
        ];
        let known = |id: &str| id != "claude-sonnet-9";
        assert_eq!(
            plan_models(installed, known, Some(&windows), NOW),
            [
                ("claude-opus-5-5".to_string(), Room::Left),
                ("claude-fable-5-2".to_string(), Room::UsedUp)
            ]
        );
        // Never read: room, as today.
        assert_eq!(
            plan_models([("claude-opus-5-5", None, "Opus 5.5")], |_| true, None, NOW),
            [("claude-opus-5-5".to_string(), Room::Left)]
        );
    }

    #[test]
    fn a_paid_model_starts_only_when_the_person_chose_the_api() {
        let ask = || Answer::AskToPay {
            model: "claude-sonnet-5-5".into(),
            name: "anthropic/claude-sonnet-5.5".into(),
            access: Some("byok".into()),
            effort: None,
        };
        assert_eq!(
            launch(ask(), false, "Claude"),
            Launch::Run {
                model: "claude-sonnet-5-5".into(),
                effort: None
            }
        );
        let Launch::Refuse(text) = launch(ask(), true, "Claude") else {
            panic!("refused on the plan");
        };
        assert!(text.starts_with("Your Claude plan has no room left"), "{text}");
        assert!(text.contains("claude-sonnet-5.5"), "{text}");
        assert!(!text.contains("  "), "no double spaces in refusal: {text}");
        let Launch::Refuse(cli_text) = launch_with_hint(
            ask(),
            true,
            "ChatGPT",
            "run with --route gateway, or pass --model <id>.",
        ) else {
            panic!("refused on the plan");
        };
        assert!(cli_text.starts_with("Your ChatGPT plan has no room left"), "{cli_text}");
        assert!(
            cli_text.ends_with("run with --route gateway, or pass --model <id>."),
            "{cli_text}"
        );
        assert_eq!(
            launch(
                Answer::UsePlan {
                    model: "claude-opus-5-5".into(),
                    effort: Some("medium".into())
                },
                true,
                "Claude"
            ),
            Launch::Run {
                model: "claude-opus-5-5".into(),
                effort: Some("medium".into())
            }
        );
        assert_eq!(
            launch(
                Answer::Stop {
                    reason: "no_offer".into()
                },
                true,
                "Claude"
            ),
            Launch::AsBefore
        );
    }

    #[test]
    fn a_plan_launch_with_no_known_model_is_not_routed() {
        let one = [("claude-opus-5-5".to_string(), Room::Left)];
        assert!(routable(true, Some(&one)));
        assert!(!routable(true, Some(&[])));
        assert!(!routable(true, None));
        assert!(routable(false, None));
    }

    #[test]
    fn each_answer_reads_as_gateway_sends_it() {
        let plan = json!({ "result": "use_plan", "model": "anthropic/claude-opus-5.5", "providerModel": "claude-opus-5-5", "effort": "high" });
        assert_eq!(
            parse(&plan),
            Some(Answer::UsePlan {
                model: "claude-opus-5-5".into(),
                effort: Some("high".into())
            })
        );
        let ask = json!({ "result": "ask_for_paid_fallback", "model": "anthropic/claude-sonnet-5.5", "providerModel": "claude-sonnet-5-5", "access": "byok" });
        assert_eq!(
            parse(&ask),
            Some(Answer::AskToPay {
                model: "claude-sonnet-5-5".into(),
                name: "anthropic/claude-sonnet-5.5".into(),
                access: Some("byok".into()),
                effort: None
            })
        );
        let stop = json!({ "result": "stop", "reason": "no_offer" });
        assert_eq!(
            parse(&stop),
            Some(Answer::Stop {
                reason: "no_offer".into()
            })
        );
        assert_eq!(parse(&json!({ "result": "use_plan" })), None);
        assert_eq!(parse(&json!({ "result": "later" })), None);
    }

    #[test]
    fn counts_measure_prompt_shape_without_text_and_body_with_includes_team_and_counts() {
        let empty = Counts::of("   \n ", false);
        assert_eq!(
            empty,
            Counts {
                input_tokens: 0,
                code_blocks: 0,
                images: false
            }
        );

        let prompt = "Refactor this module:\n```rust\nfn main() {}\n```\nAnd this test:\n```rust\nfn t() {}\n```";
        let counts = Counts::of(prompt, true);
        assert!(counts.input_tokens > 0);
        assert_eq!(counts.code_blocks, 2);
        assert!(counts.images);

        let built = body_with("claude", None, Some("platform"), Some(counts));
        assert_eq!(
            built,
            json!({
                "agent": "claude",
                "team": "platform",
                "counts": {
                    "inputTokens": counts.input_tokens,
                    "codeBlocks": 2,
                    "images": true
                }
            })
        );
    }

    #[test]
    fn an_across_body_lists_every_agent_and_tags_each_plan() {
        let claude = [("claude-opus-5-5".to_string(), Room::UsedUp)];
        let codex = [("gpt-6.1-sol".to_string(), Room::Left)];
        assert_eq!(
            body_across(&[("claude", Some(&claude)), ("codex", Some(&codex))], None),
            json!({
                "agent": "auto",
                "agents": ["claude", "codex"],
                "plans": [
                    { "agent": "claude", "account": 0, "models": [{ "id": "claude-opus-5-5", "room": "usedUp" }] },
                    { "agent": "codex", "account": 1, "models": [{ "id": "gpt-6.1-sol", "room": "left" }] },
                ],
            })
        );
        // Through Gateway: no plans, the agents still compete.
        assert_eq!(
            body_across(&[("claude", None), ("codex", Some(&[]))], None),
            json!({ "agent": "auto", "agents": ["claude", "codex"] })
        );
    }
}
