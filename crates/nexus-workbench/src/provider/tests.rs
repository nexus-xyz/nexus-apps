use super::*;
use crate::agent_launch::{Catalog, model_key, plan_of};
use nexus_client::auto_route;
use nexus_local_agents::Login;
use nexus_providers::{Funding, Room};
use serde_json::{Value, json};

/// No method of [`Provider`] has a default body: each provider's file writes every one.
#[test]
fn provider_trait_has_no_default_bodies() {
    let source = include_str!("mod.rs");
    let start = source
        .find("pub trait Provider: Send + Sync {")
        .unwrap_or_else(|| panic!("no Provider trait"));
    let body = &source[start..];
    let end = body.find("\n}\n").unwrap_or_else(|| panic!("the trait does not close"));
    let body = &body[body.find('{').map_or(0, |at| at + 1)..end];
    for line in body.lines().map(str::trim).filter(|line| !line.starts_with("//")) {
        assert!(!line.contains('{'), "a Provider method has a default body: {line}");
    }
    assert!(body.matches("fn ").count() >= 10, "the trait's methods were not read");
}

/// The two kinds a session records today keep the ids its `via` and the agents' route send.
#[test]
fn kind_ids_are_the_session_s_via_values() {
    use nexus_local_agents::launch::Route;
    assert_eq!(ProviderKind::NexusCloud.as_str(), Via::Nexus.as_str());
    assert_eq!(ProviderKind::NexusCloud.as_str(), Route::Gateway.via());
    assert_eq!(ProviderKind::CliSubscription.as_str(), Via::Subscription.as_str());
    assert_eq!(ProviderKind::CliSubscription.as_str(), Route::Subscription.via());
}

/// Every kind has an entry and a summary Settings can show before any instance exists.
#[test]
fn every_kind_has_what_settings_shows() {
    for kind in ProviderKind::ALL {
        assert!(!ProviderUi::entry(&kind).name.is_empty(), "{kind}");
        let summary = kind.summary();
        assert!(summary.ends_with('.') && !summary.contains("  "), "{kind}: {summary}");
    }
}

fn machine() -> MachineFacts {
    MachineFacts {
        agents: vec![
            MachineAgent {
                agent: SessionAgent::Claude,
                installed: true,
                login: Login::Plan,
                accounts: vec!["a_1700000000000".into()],
            },
            MachineAgent {
                agent: SessionAgent::Codex,
                installed: false,
                login: Login::SignedOut,
                accounts: Vec::new(),
            },
            MachineAgent {
                agent: SessionAgent::Nexus,
                installed: true,
                login: Login::Unknown,
                accounts: Vec::new(),
            },
        ],
    }
}

/// Each plan login is one provider (the agent's own, then each extra account), Nexus's own agent has none, and Nexus
/// Cloud joins only while signed in. Keys and servers have no provider yet.
#[test]
fn the_set_holds_each_plan_login_and_cloud_while_signed_in() {
    let ids = |set: &ProviderSet| set.iter().map(|p| p.id().as_str().to_string()).collect::<Vec<_>>();
    let client = Client::new(nexus_client::ClientOptions::new("https://gateway.invalid"));
    let signed_in = ProviderSet::from_settings(&ProviderSettings::default(), &machine(), Some(&client), false);
    assert_eq!(
        ids(&signed_in),
        [
            "subscription:claude:own",
            "subscription:claude:a_1700000000000",
            "subscription:codex:own",
            "nexus"
        ]
    );
    let kinds: Vec<ProviderKind> = signed_in.iter().map(|p| p.kind()).collect();
    assert_eq!(
        kinds,
        [
            ProviderKind::CliSubscription,
            ProviderKind::CliSubscription,
            ProviderKind::CliSubscription,
            ProviderKind::NexusCloud
        ]
    );
    let extra = signed_in
        .get(&InstanceId::subscription_account(
            nexus_agents::Agent::Claude,
            "a_1700000000000",
        ))
        .expect("the extra account");
    assert_eq!(extra.name(), "Claude Code · a_1700000000000");
    assert_eq!(extra.agents(), [SessionAgent::Claude]);

    let signed_out = ProviderSet::from_settings(&ProviderSettings::default(), &machine(), None, false);
    assert_eq!(
        ids(&signed_out),
        [
            "subscription:claude:own",
            "subscription:claude:a_1700000000000",
            "subscription:codex:own"
        ]
    );
}

/// A plan login's health is read on this computer: the program, then the login. Cloud exists only while signed in.
#[test]
fn health_reads_the_program_and_the_login() {
    let facts = machine();
    let cx = HealthContext {
        machine: &facts,
        keys: None,
    };
    let claude = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, None);
    assert_eq!(claude.local_health(&cx), Health::Ready);
    let codex = CliSubscriptionProvider::new(nexus_agents::Agent::Codex, None);
    assert_eq!(
        codex.local_health(&cx),
        Health::Setup(ProviderError::AgentMissing(nexus_agents::Agent::Codex))
    );
    let mut keyed = machine();
    keyed.agents[0].login = Login::ApiKey;
    assert_eq!(
        claude.local_health(&HealthContext {
            machine: &keyed,
            keys: None
        }),
        Health::Setup(ProviderError::WrongLogin(nexus_agents::Agent::Claude))
    );
    keyed.agents[0].login = Login::SignedOut;
    assert_eq!(
        claude.local_health(&HealthContext {
            machine: &keyed,
            keys: None
        }),
        Health::Setup(ProviderError::SignedOut(nexus_agents::Agent::Claude))
    );
}

/// Each provider refuses an agent it does not run. Cloud runs Nexus's own agent and the agents Gateway has a route for.
#[test]
fn each_provider_refuses_an_agent_it_does_not_run() {
    let cx = RefusalContext { home: None };
    let claude = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, None);
    assert_eq!(
        claude.refusal(SessionAgent::Codex, &cx),
        Err(ProviderError::NotOffered {
            agent: nexus_agents::Agent::Codex,
            kind: ProviderKind::CliSubscription
        })
    );
    assert!(matches!(
        claude.refusal(SessionAgent::Nexus, &cx),
        Err(ProviderError::Refused(_))
    ));
    let cloud = NexusCloudProvider::new(Client::new(nexus_client::ClientOptions::new("https://gateway.invalid")));
    assert!(cloud.agents().contains(&SessionAgent::Nexus));
    assert!(cloud.agents().contains(&SessionAgent::Claude));
    assert!(!cloud.agents().contains(&SessionAgent::Antigravity));
    assert_eq!(
        cloud.refusal(SessionAgent::Antigravity, &cx),
        Err(ProviderError::NotOffered {
            agent: nexus_agents::Agent::Antigravity,
            kind: ProviderKind::NexusCloud
        })
    );
}

fn model(id: &str, resolved: Option<&str>, name: &str) -> Model {
    Model {
        id: id.into(),
        name: name.into(),
        efforts: Vec::new(),
        resolved_id: resolved.map(String::from),
        fast: false,
    }
}

/// Gateway's catalog with each `(catalog id, provider model)` priced, as `load_catalog` reads it.
fn priced(rows: &[(&str, &str)]) -> Catalog {
    let mut catalog = Catalog::default();
    for (id, provider_model) in rows {
        let catalog_id = CatalogModelId::from_gateway(id).expect("an id");
        for key in [model_key(id), model_key(provider_model)] {
            catalog.prices.insert(key.clone(), (1.0, 2.0));
            catalog.ids.insert(key, catalog_id.clone());
        }
    }
    catalog
}

struct Reading {
    label: &'static str,
    models: Vec<String>,
    used: f64,
    resets_at: Option<i64>,
}

fn reading(label: &'static str, used: f64, resets_at: Option<i64>) -> Reading {
    Reading {
        label,
        models: Vec::new(),
        used,
        resets_at,
    }
}

const NOW: i64 = 1_000;

/// Auto's plan before this layer (`auto_route::plan_models`, known by price) and after it (`plan_offers`, then
/// `plan_of`, known by catalog id) are equal on every case: no readings, a used-up scoped week, a window past its
/// reset, a group limit, an unpriced id, `default` before an alias of the same model, two ids under one price key, an
/// empty resolved id, and Gateway rows that are priced but carry no id or an empty one.
#[test]
fn the_plan_auto_sends_is_the_same_before_and_after_the_layer() {
    let price = json!({ "input": 1.0, "output": 2.0 });
    let catalog = crate::agent_launch::catalog_of(&json!({ "data": [
        { "id": "anthropic/claude-opus-5.5", "provider_model": "claude-opus-5-5", "pricing": price },
        { "id": "anthropic/claude-fable-5.2", "provider_model": "claude-fable-5-2", "pricing": price },
        { "id": "google/gemini-3.8-flash", "provider_model": "gemini-3.8-flash", "pricing": price },
        { "provider_model": "claude-haiku-6", "pricing": price },
        { "id": "", "provider_model": "claude-nova-1", "pricing": price },
        { "id": "anthropic/claude-sonnet-9", "provider_model": "claude-sonnet-9" }
    ] }));
    let installed = vec![
        model("claude-haiku-6", None, "Haiku 6"),
        model("claude-nova-1", None, "Nova 1"),
        model("default", Some("claude-opus-5-5"), "Default"),
        model("opus", Some("claude-opus-5-5"), "Opus 5.5"),
        model("claude-opus-5-5", None, "Opus 5.5"),
        model("claude-opus-5.5", None, "Opus 5.5"),
        model("claude-fable-5-2", None, "Fable 5.2"),
        model("claude-sonnet-9", None, "Sonnet 9"),
        model("blank", Some(""), "Blank"),
        model("gemini-3.8-flash", None, "Gemini 3.8 Flash"),
    ];
    let mut gemini = reading("Weekly · Gemini", 100.0, None);
    gemini.models = vec!["Gemini 3.8 Flash".into()];
    let cases: Vec<(&str, Option<Vec<Reading>>)> = vec![
        ("no readings", None),
        (
            "a used-up scoped week",
            Some(vec![
                reading("Session", 40.0, Some(NOW + 1)),
                reading("Weekly · Fable", 100.0, None),
            ]),
        ),
        (
            "a window past its reset",
            Some(vec![reading("Session", 100.0, Some(NOW - 1))]),
        ),
        ("a group limit", Some(vec![gemini])),
        ("no window binds", Some(Vec::new())),
    ];
    let lookup = |id: &str| catalog.id(id);
    let provider = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, None);
    for (case, readings) in cases {
        let windows: Option<Vec<Window<'_>>> = readings.as_ref().map(|readings| {
            readings
                .iter()
                .map(|r| Window {
                    label: r.label,
                    models: &r.models,
                    used_percent: r.used,
                    resets_at: r.resets_at,
                })
                .collect()
        });
        let before = auto_route::plan_models(
            installed
                .iter()
                .map(|m| (m.id.as_str(), m.resolved_id.as_deref(), m.name.as_str())),
            |id| catalog.prices.contains_key(&model_key(id)),
            windows.as_deref(),
            NOW,
        );
        let cx = OfferContext {
            installed: &installed,
            windows: windows.as_deref(),
            now: NOW,
            catalog: &lookup,
        };
        let after = plan_of(&provider.plan_offers(&cx));
        assert_eq!(after, before, "{case}");
        assert_eq!(
            auto_route::body("claude", Some(&after)),
            auto_route::body("claude", Some(&before)),
            "{case}"
        );
        // The rows with no id are in the plan, so the case above compared them.
        for id in ["claude-haiku-6", "claude-nova-1"] {
            assert!(after.iter().any(|(model, _)| model == id), "{case}: {id}");
        }
    }
}

/// A plan offer keeps what Auto's request drops: a model the catalog lacks, and a room no reading gave.
#[test]
fn plan_offers_keep_unpriced_models_and_unknown_room() {
    let catalog = priced(&[("anthropic/claude-opus-5.5", "claude-opus-5-5")]);
    let installed = vec![
        model("claude-opus-5-5", None, "Opus 5.5"),
        model("claude-sonnet-9", None, "Sonnet 9"),
    ];
    let lookup = |id: &str| catalog.id(id);
    let cx = OfferContext {
        installed: &installed,
        windows: None,
        now: NOW,
        catalog: &lookup,
    };
    let offers = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, None).plan_offers(&cx);
    assert_eq!(
        offers,
        [
            ModelOffer {
                agent: nexus_agents::Agent::Claude,
                agent_model: "claude-opus-5-5".into(),
                catalog: CatalogModelId::from_gateway("anthropic/claude-opus-5.5"),
                name: "Opus 5.5".into(),
                funding: Funding::Plan {
                    account: nexus_providers::AccountHandle(0)
                },
                room: Room::Unknown,
            },
            ModelOffer {
                agent: nexus_agents::Agent::Claude,
                agent_model: "claude-sonnet-9".into(),
                catalog: None,
                name: "Sonnet 9".into(),
                funding: Funding::Plan {
                    account: nexus_providers::AccountHandle(0)
                },
                room: Room::Unknown,
            },
        ]
    );
}

/// Cloud offers only what Gateway prices, paid through Gateway, and nothing for Nexus's own agent.
#[tokio::test]
async fn cloud_offers_priced_models_through_gateway() {
    let catalog = priced(&[("anthropic/claude-opus-5.5", "claude-opus-5-5")]);
    let installed = vec![
        model("opus", Some("claude-opus-5-5"), "Opus 5.5"),
        model("claude-sonnet-9", None, "Sonnet 9"),
    ];
    let lookup = |id: &str| catalog.id(id);
    let cx = OfferContext {
        installed: &installed,
        windows: None,
        now: NOW,
        catalog: &lookup,
    };
    let cloud = NexusCloudProvider::new(Client::new(nexus_client::ClientOptions::new("https://gateway.invalid")));
    let offers = cloud.offers(SessionAgent::Claude, &cx).await.expect("offers");
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].agent_model, "claude-opus-5-5");
    assert_eq!(offers[0].funding, Funding::Gateway);
    assert_eq!(offers[0].room, Room::Unknown);
    assert!(cloud.offers(SessionAgent::Nexus, &cx).await.expect("offers").is_empty());
}

#[cfg(unix)]
mod launch {
    use super::*;
    use crate::agent_launch::tests::{auto_request, fake_gateway};
    use std::sync::{Arc, Mutex};

    /// A fake Gateway that allows plan sessions and opens `sess_1` for team `platform`, keeping each session body.
    async fn gateway() -> (Client, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
        let opened = Arc::new(Mutex::new(Vec::new()));
        let seen = opened.clone();
        let (url, server) = fake_gateway(move |path, body| match path {
            "/v1/orgs/settings" => (200, json!({ "agentSubscriptions": true })),
            "/v1/agents/sessions" => {
                seen.lock()
                    .expect("seen lock")
                    .push(serde_json::from_str(body).unwrap_or(Value::Null));
                (200, json!({ "id": "sess_1", "team": "platform" }))
            }
            _ => (404, json!({})),
        })
        .await;
        let mut options = nexus_client::ClientOptions::new(&format!("{url}/"));
        options.api_key = Some("nx_test".into());
        (Client::new(options), opened, server)
    }

    fn launch_cx<'a>(
        request: &'a LaunchRequest,
        route: LaunchRoute<'a>,
        env: &'a HashMap<String, String>,
    ) -> LaunchContext<'a> {
        LaunchContext {
            request,
            route,
            program: Path::new("/bin/claude"),
            env,
            home: None,
        }
    }

    fn gateway_cx<'a>(
        request: &'a LaunchRequest,
        client: &'a Client,
        env: &'a HashMap<String, String>,
    ) -> LaunchContext<'a> {
        launch_cx(
            request,
            LaunchRoute::Gateway(GatewayLaunch {
                client,
                session: SessionFacts { usage_data: true },
                auto: None,
            }),
            env,
        )
    }

    fn request(via: Via) -> LaunchRequest {
        let mut request = auto_request(via);
        request.model = Some("claude-opus-5-5".into());
        request.args = vec!["--model".into(), "claude-opus-5-5".into()];
        request.prompt_counts = None;
        request
    }

    /// Cloud wraps today's launch through Gateway: the Gateway session with `via: nexus`, then Gateway's credential on
    /// it, with the sign-in's key and the team Gateway answered.
    #[tokio::test]
    async fn cloud_opens_the_gateway_session_and_gives_its_credential() {
        nexus_client::install_tls();
        let (client, opened, server) = gateway().await;
        let request = request(Via::Nexus);
        let env = HashMap::new();
        let cx = gateway_cx(&request, &client, &env);
        let cloud = NexusCloudProvider::new(client.clone());
        let prepared = cloud.prepare(SessionAgent::Claude, &cx).await.expect("prepared");
        let Prepared::Gateway { session_id, team, .. } = &prepared else {
            panic!("a Gateway session: {prepared:?}");
        };
        assert_eq!((session_id.as_str(), team.as_deref()), ("sess_1", Some("platform")));
        let credential = cloud
            .credential(SessionAgent::Claude, &prepared, &cx)
            .await
            .expect("credential");
        server.abort();
        let Credential::Gateway {
            core_url,
            token,
            session_id,
            team,
        } = credential
        else {
            panic!("a Gateway credential");
        };
        assert!(!core_url.ends_with('/'), "{core_url}");
        assert_eq!(token.expose(), "nx_test");
        assert_eq!((session_id.as_str(), team.as_deref()), ("sess_1", Some("platform")));
        let body = opened.lock().expect("opened lock")[0].clone();
        assert_eq!(body["via"], "nexus");
        assert_eq!(body["agent"], "claude");
        assert!(body.get("person").is_none(), "no login name leaves the computer");
        assert!(body.get("cwd").is_none(), "no folder name leaves the computer");
    }

    /// A plan login wraps today's plan launch: the Gateway session with `via: subscription`, then the agent's own login
    /// reporting usage to it, on the extra account's folder.
    #[tokio::test]
    async fn a_plan_login_reports_usage_to_its_gateway_session() {
        nexus_client::install_tls();
        let (client, opened, server) = gateway().await;
        let mut request = request(Via::Subscription);
        request.account_dir = Some("/data/claude/a_1700000000000".into());
        let env = HashMap::new();
        let cx = gateway_cx(&request, &client, &env);
        let plan = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, Some("a_1700000000000"));
        let prepared = plan.prepare(SessionAgent::Claude, &cx).await.expect("prepared");
        let credential = plan
            .credential(SessionAgent::Claude, &prepared, &cx)
            .await
            .expect("credential");
        server.abort();
        let Credential::AgentLogin { account_dir, usage } = credential else {
            panic!("the agent's login");
        };
        assert_eq!(account_dir, request.account_dir);
        let usage = usage.expect("a usage sink");
        assert_eq!(usage.session_id, "sess_1");
        assert_eq!(usage.token.expose(), "nx_test");
        assert_eq!(opened.lock().expect("opened lock")[0]["via"], "subscription");
    }

    #[tokio::test]
    async fn a_saved_provider_route_does_not_admit_a_local_plan() {
        let request = request(Via::Subscription);
        let env = HashMap::new();
        let keys: Arc<dyn KeySource> = Arc::new(crate::saved_provider_tests::Keys::default());
        let cx = launch_cx(&request, LaunchRoute::SavedProvider { keys: &keys }, &env);
        let plan = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, None);
        assert_eq!(
            plan.prepare(SessionAgent::Claude, &cx).await,
            Err(signed_out(SessionAgent::Claude))
        );
    }

    #[tokio::test]
    async fn signed_out_plan_uses_its_account_without_gateway_usage() {
        let mut request = request(Via::Subscription);
        request.account_dir = Some("/data/claude/a_1700000000000".into());
        let env = HashMap::new();
        request.scope = crate::session_scope::SessionScope::local_text().into();
        let mode = LocalProvidersOptIn::Enabled(LocalProviders {
            keys: Arc::new(crate::saved_provider_tests::Keys::default()),
        });
        let Some(crate::agent_launch::LocalLaunch::Subscription(admission)) =
            crate::agent_launch::admit_local_launch(&request, &request.scope, &mode)
        else {
            panic!("the local plan is admitted");
        };
        let cx = launch_cx(&request, LaunchRoute::LocalSubscription(admission), &env);
        let plan = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, Some("a_1700000000000"));
        let prepared = plan.prepare(SessionAgent::Claude, &cx).await.expect("prepared");
        assert!(matches!(prepared, Prepared::Local { .. }));
        let credential = plan
            .credential(SessionAgent::Claude, &prepared, &cx)
            .await
            .expect("credential");
        assert!(
            matches!(credential, Credential::AgentLogin { account_dir, usage: None } if account_dir == request.account_dir)
        );
    }

    /// A fake Gateway that answers nothing and counts the calls that reach it.
    async fn counting_gateway() -> (Client, Arc<Mutex<usize>>, tokio::task::JoinHandle<()>) {
        let calls = Arc::new(Mutex::new(0));
        let seen = calls.clone();
        let (url, server) = fake_gateway(move |_, _| {
            *seen.lock().expect("calls lock") += 1;
            (404, json!({}))
        })
        .await;
        let mut options = nexus_client::ClientOptions::new(&url);
        options.api_key = Some("nx_test".into());
        (Client::new(options), calls, server)
    }

    /// Each provider refuses, before any call, a session that is not its own: another route, another plan account, or
    /// Nexus's own agent, which starts on its own launch.
    #[tokio::test]
    async fn a_provider_refuses_a_session_that_is_not_its_own_before_any_call() {
        nexus_client::install_tls();
        let (client, calls, server) = counting_gateway().await;
        let env = HashMap::new();
        let cloud = NexusCloudProvider::new(client.clone());

        let on_plan = request(Via::Subscription);
        assert_eq!(
            cloud
                .prepare(SessionAgent::Claude, &gateway_cx(&on_plan, &client, &env))
                .await,
            Err(ProviderError::Refused(
                "This session does not start Claude Code on nexus.".into()
            ))
        );

        let mut on_account = request(Via::Subscription);
        on_account.account_dir = Some("/data/claude/a_1700000000000".into());
        let own = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, None);
        assert_eq!(
            own.prepare(SessionAgent::Claude, &gateway_cx(&on_account, &client, &env))
                .await,
            Err(ProviderError::Refused(
                "This session starts on another Claude Code account than Claude Code.".into()
            ))
        );
        let other = CliSubscriptionProvider::new(nexus_agents::Agent::Claude, Some("a_1800000000000"));
        assert!(matches!(
            other
                .prepare(SessionAgent::Claude, &gateway_cx(&on_account, &client, &env))
                .await,
            Err(ProviderError::Refused(_))
        ));

        let mut nexus = request(Via::Nexus);
        nexus.agent = SessionAgent::Nexus;
        let prepared = Prepared::Gateway {
            session_id: "sess_1".into(),
            model: None,
            team: None,
            args: Vec::new(),
        };
        let cx = gateway_cx(&nexus, &client, &env);
        assert!(matches!(
            cloud.prepare(SessionAgent::Nexus, &cx).await,
            Err(ProviderError::Refused(_))
        ));
        assert!(matches!(
            cloud.credential(SessionAgent::Nexus, &prepared, &cx).await,
            Err(ProviderError::Refused(_))
        ));
        server.abort();
        assert_eq!(*calls.lock().expect("calls lock"), 0, "no call reached Gateway");
    }
}
