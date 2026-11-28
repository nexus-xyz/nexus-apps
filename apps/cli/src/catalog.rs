//! Every built-in `nexus` command in one place: `nexus help` prints it for people and
//! `nexus help --json` for machines, in the stable shape `nexus-cli-commands/1` that `commands.json` keeps. It is
//! generated from the same tables the per-command help prints. The `commands` example emits the default catalog
//! with SEMVER for docs; a test checks that the committed `commands.json` is fresh.

use nexus_client::agent_entries::Agent;
use nexus_workbench::event::SessionAgent;
use serde_json::{Map, Value, json};

pub const COMMANDS_SCHEMA: &str = "nexus-cli-commands/1";

pub type Rows = &'static [(&'static str, &'static str)];

pub const AUTH_COMMANDS: Rows = &[
    (
        "login --url <core>",
        "Sign in as you, in your organization: approve a one-time code in Nexus UI (--no-browser to only print the link)",
    ),
    (
        "login --with-token",
        "Sign in with an API key read from stdin, for CI and scripts",
    ),
    (
        "signup --name <name> [--email <email>] [--dry-run]",
        "Create an organization and its owner without a browser, and save the new API key (when key-only signup is enabled)",
    ),
    (
        "status",
        "Your org, user and role, the Gateway URL, your token (masked) and whether Gateway accepts it",
    ),
    ("logout", "Revoke the saved token on Gateway and forget it"),
    (
        "delete-account [--dry-run] [--confirm DELETE]",
        "Delete your account for good: your sign-in, your personal organization and your place in every organization",
    ),
];

/// Under `nexus auth tokens`.
pub const AUTH_TOKEN_COMMANDS: Rows = &[
    (
        "list [--all]",
        "Your API keys and CLI sign-ins in this organization, newest first (--all: everyone's, owners and admins)",
    ),
    (
        "create --label <label>",
        "Create a personal API key for CI and scripts: it acts as you, with your role, and it is shown once",
    ),
    (
        "revoke <id>",
        "Revoke one of your tokens at once (owners and admins: anyone's)",
    ),
];

/// `nexus agents`' subcommands. `use` names each launcher's command, and `setup` each agent this CLI carries a recipe
/// for.
pub fn agent_commands() -> Vec<(String, String)> {
    let recipes: Vec<&str> = crate::recipes::recipes()
        .unwrap_or_default()
        .iter()
        .map(|recipe| recipe.agent.as_str())
        .collect();
    vec![
        (
            "list".into(),
            "Each local launcher (also plain nexus agents): install, sign-in, route, payer, and setup steps".into(),
        ),
        (
            format!("use <{}> <gateway|subscription|default>", Agent::commands("|")),
            "The route an agent takes when you name none, in every Nexus app you sign in to".into(),
        ),
        (
            format!("setup <{}>", recipes.join("|")),
            "How to run an agent through Gateway, from its recipe: the limits first, then each step. Writes nothing"
                .into(),
        ),
    ]
}

/// `nexus agents share`: a Claude Code session shared as an artifact (`agent_share.rs`).
pub const AGENT_SHARE_COMMANDS: Rows = &[
    (
        "share <session|latest> [--org | --team T | --people a@x,b@y] [--include-thinking]",
        "Share a Claude Code session (nexus claude) as an artifact, from its transcript on this machine: latest is this folder's newest. Again: a new version, and the earlier ones revoked",
    ),
    (
        "share <session|latest> --to a@x,b@y | --public [--expires 30d]",
        "Share it outside your organization, as its settings allow",
    ),
    (
        "share <session|latest> --private",
        "Stop sharing a session: revoke every version, so only you can read it",
    ),
];

pub const KEY_COMMANDS: Rows = &[
    (
        "add <provider> <key> [--team <slug|id>] [--user <me|id>]",
        "Bring your own provider key (owners and admins); encrypted at rest. With --team or --user, a team's or a person's key, which their calls use first",
    ),
    (
        "list",
        "Your provider keys, masked, with the teams' and people's keys you may read",
    ),
    (
        "history <provider> [--team <slug|id>] [--user <me|id>]",
        "A provider key's versions: who added each, when, and its last four characters",
    ),
    (
        "replace <provider> <key> [--team <slug|id>] [--user <me|id>] [--dry-run]",
        "Replace a provider key, checked against its provider first (owners and admins)",
    ),
    (
        "disable <provider> [--team <slug|id>] [--user <me|id>] [--dry-run]",
        "Stop calls from using a provider key; it stays sealed (owners and admins)",
    ),
    (
        "enable <provider> [--team <slug|id>] [--user <me|id>] [--dry-run]",
        "Use a disabled or rejected provider key again, checked against its provider first (owners and admins)",
    ),
    (
        "remove <provider> [--team <slug|id>] [--user <me|id>] [--dry-run]",
        "Delete a provider key; its versions stay (owners and admins)",
    ),
    (
        "revoke <scoped key id> [--dry-run]",
        "Revoke a module's scoped key before it expires (owners and admins): its key id (keyId in the JSON of nexus spend, or the audit log), or its smk_… id",
    ),
];

pub const UPDATE_COMMANDS: Rows = &[
    (
        "[--json]",
        "Install the CLI version your Gateway publishes: checksum verified, swapped in atomically, the previous one kept",
    ),
    (
        "--check [--json]",
        "Only say whether a newer version is available (exit 0 either way)",
    ),
    (
        "--rollback [--json]",
        "Go back to the version before the last update (run it again to undo)",
    ),
];

pub const TEAM_COMMANDS: Rows = &[
    ("list", "Teams in your organization, with yours marked"),
    (
        "tree [--depth N]",
        "Teams nested under their parents, with levels, leads and headcount",
    ),
    (
        "show <slug>",
        "A team: its path, child teams, members and pending members",
    ),
    (
        "create <slug> [--parent P] [--level L] [--name N]",
        "Create a team, optionally under a parent (owners and admins)",
    ),
    (
        "move <slug> --parent P | --root",
        "Put a team under another one, or at the top (owners and admins)",
    ),
    (
        "rename <slug> <new-slug> [--name N]",
        "Change a team's slug, name, --level or --description (owners and admins)",
    ),
    (
        "delete <slug> --yes [--cascade]",
        "Delete a team (--cascade: and every team under it); spend stays",
    ),
    (
        "add <slug> <email|userId> [--lead] [--home]",
        "Add someone; an email not in the organization yet is pending until they join",
    ),
    (
        "remove <slug> <email|userId>",
        "Take someone out of a team (owners and admins)",
    ),
    (
        "pending [--team T] [--cancel <email|id>]",
        "People added by email who have not joined yet (owners and admins)",
    ),
    (
        "import <file> [--dry-run] [--plan P] [--prune [--yes]]",
        "Load teams, leads and members from a YAML or JSON file; --dry-run shows the changes and a plan to apply exactly them (owners and admins)",
    ),
    (
        "import <file> --invite [--dry-run] [--yes]",
        "Also invite the file's people who are not in the organization yet (the sign-in provider emails them; never without --invite)",
    ),
    (
        "use <slug>",
        "Make it your default team: where your spend goes when you name none",
    ),
];

pub const MEMBER_COMMANDS: Rows = &[
    ("list", "Everyone in your organization, by role"),
    (
        "pending [--team T]",
        "People in teams by email who have not joined yet (owners and admins)",
    ),
    (
        "invite <email…> [--role admin] [--dry-run] [--yes]",
        "Invite people by email; the sign-in provider sends each one (owners and admins)",
    ),
    (
        "invite --pending [--dry-run] [--yes]",
        "Invite everyone in a team by email who has not joined yet",
    ),
    (
        "remove <email|userId> [--dry-run] [--yes]",
        "Remove someone from the organization; recoverable for 30 days (owners and admins)",
    ),
    (
        "removed",
        "Removed members, and until when each can be recovered (owners and admins)",
    ),
    (
        "recover <userId> [--dry-run]",
        "Bring back a removed member with their teams, inside 30 days (owners and admins)",
    ),
];

pub const POLICY_VERBS: &[&str] = &[
    "show",
    "set",
    "test",
    "controls",
    "pause",
    "limit",
    "override",
    "clear",
    "connectors",
    "connector",
    "grants",
    "review-events",
    "agent-writes",
];

/// `nexus policy`'s subcommands. `show` names the agents a policy applies to: those with a Gateway route.
pub fn policy_commands() -> Vec<(String, String)> {
    let show = (
        format!(
            "show [org|team/<slug>|agent/<{}>]",
            crate::models::policy_agent_ids("|")
        ),
        "A level's rule, what it inherits, and what it allows".to_string(),
    );
    std::iter::once(show).chain(owned(POLICY_OTHER_COMMANDS)).collect()
}

/// `nexus policy`'s subcommands after `show`.
const POLICY_OTHER_COMMANDS: Rows = &[
    (
        "set <target> --allow a,b --deny c",
        "Replace a list (--allow all, --deny none clear it); owners and admins",
    ),
    (
        "set <target> --default m",
        "The default for nexus/auto (an agent's default for agent/…)",
    ),
    (
        "set <target> --from rule.yaml | --clear",
        "A whole rule from a file, or remove the level's rule",
    ),
    (
        "test [--model m] [--as who] [--team T] [--agent a]",
        "Would it be allowed, and by which rule (exit 1 if refused)",
    ),
    (
        "controls [--team <slug>]",
        "The operator controls in force that apply to you (all of them for owners and admins), or a team's with --team",
    ),
    (
        "pause <org|team/<slug>|person/<id>|key/<id>> --reason r --expires 12h [--team <slug>]",
        "Stop model traffic for a target until it expires; owners and admins, or a team lead with --team",
    ),
    (
        "limit <target> --models a,b --reason r --expires 12h [--team <slug>]",
        "Narrow a target to these models until it expires; owners and admins, or a team lead with --team",
    ),
    (
        "override <target> --models a,b --reason r --expires 12h",
        "Allow these models past a deny until it expires; owners and admins",
    ),
    (
        "clear <control-id> [--team <slug>]",
        "Take a control out of force; owners and admins, or a team lead with --team",
    ),
    ("connectors", "Which connectors your organization's members may use"),
    (
        "connector <name> <disabled|available|enabled_for_all> [--teams a,b]",
        "Choose who may use a connector; owners and admins",
    ),
    (
        "grants [list]",
        "The connector writes each module may make (write grants), and what may be granted",
    ),
    (
        "grants set <product/capability> --connector c --actions a,b",
        "Set a module's granted writes of one connector, in full (an empty --actions revokes them); owners and admins",
    ),
    (
        "grants set robustness/review --for-mode <shadow|comment|comment_and_check>",
        "Grant Nexus Review exactly what the mode needs, and revoke the rest; owners and admins",
    ),
    (
        "review-events [show]",
        "Which review events a module may post on pull requests: comment, request_changes or full (the default)",
    ),
    (
        "review-events set <comment|request_changes|full>",
        "Choose whether modules may request changes and approve; owners and admins",
    ),
    (
        "agent-writes [show]",
        "Whether your agents make your organization's allowed connector writes without a confirmation in Nexus",
    ),
    (
        "agent-writes <on|off>",
        "Turn your agents' writes on or off; only you, from your own sign-in, never an agent link",
    ),
];

pub const CHAT_COMMANDS: Rows = &[
    (
        "[list] [--all] [--search words] [--limit N]",
        "Your chat sessions, most recent first (--all: archived ones too)",
    ),
    (
        "show <id>",
        "A session's messages, with the model, cost and sources of each answer",
    ),
    (
        "images <id> [<image>…] [--out dir] [--force]",
        "Save a session's images to files: every question's, or the ones named by id",
    ),
    ("rename <id> <title…>", "Give a session a title"),
    (
        "archive <id> [--undo]",
        "Hide a session from the list (--undo brings it back)",
    ),
    ("delete <id>", "Delete a session and its messages"),
];

/// Sharing a session (Nexus Artifacts) and exporting it, beside `nexus chat sessions`.
pub const CHAT_SHARE_COMMANDS: Rows = &[
    (
        "share <session> [--org | --team T | --people a@x,b@y] [--replace] [--allow-continue]",
        "Share a session as an artifact: a snapshot for your organization, chosen teams or chosen people (again: a new version; --replace revokes the earlier ones)",
    ),
    (
        "share <session> --to a@x,b@y | --public [--expires 30d] [--include-code]",
        "Share it outside your organization, as its settings allow: with named people by email (they sign in with that email to open it) or anyone with the link",
    ),
    (
        "share <session> --private",
        "Stop sharing a session: revoke every version, so only you can read it",
    ),
    (
        "export <session> [--md] [--out file]",
        "A session as Markdown, to stdout or a file",
    ),
];

/// `nexus artifacts`: chats shared inside the organization or outside it.
pub const ARTIFACT_COMMANDS: Rows = &[
    (
        "list [--mine | --shared-with-me] [--revoked | --all]",
        "Artifacts shared with you and by you, newest first (--revoked: yours you revoked; --all: both); shared with you includes other organizations' shares to your email",
    ),
    (
        "list --external [--audience A] [--author A] [--team T]",
        "Everything shared outside your organization, with opens, readers, requests and reports (owners and admins); --search narrows by title",
    ),
    (
        "show <id>",
        "An artifact: who shared it with whom, its models and cost, and the conversation",
    ),
    (
        "export <id> [--md] [--out file]",
        "An artifact as Markdown, to stdout or a file",
    ),
    (
        "images <id|token|link> [<image>…] [--out dir] [--force]",
        "Save an artifact's images to files: every question's, or the ones named by id; a public link's token or the link works too",
    ),
    (
        "continue <id>",
        "Continue an artifact in a new private chat session of your own (when its author allows)",
    ),
    (
        "access <id> [--grant E] [--deny E] [--remove E]",
        "Who has an artifact shared outside, who asked for it and how often each opened it; grant, deny or remove people by email (its author, owners and admins)",
    ),
    (
        "request <id> [--message text]",
        "Ask the author of an artifact shared outside their organization for access",
    ),
    (
        "revoke <id> [<id>…]",
        "Stop sharing an artifact at once (yours; owners and admins: any they can read); several ids revoke in bulk",
    ),
];

/// `nexus org` (`nexus orgs` is the same command): your organizations, switching, and the organization's settings.
/// `nexus pages`: standalone HTML, Markdown or React pages in versions that never change.
pub const PAGE_COMMANDS: Rows = &[
    (
        "publish <file> [--title T] [--format html|md|jsx|board]",
        "Publish an HTML, Markdown, React (.jsx, .tsx) or board (.board.json) file as a new page (version 1); the title defaults to its <title> or first heading",
    ),
    ("list", "Your pages, most recently written first"),
    (
        "get <id> [<version>] [--source] [--out file]",
        "A page's latest version, or the one named; --source prints or saves its source as written",
    ),
    (
        "update <id> <file> [--base N] [--title T]",
        "Write the next version from a file; --base refuses it if someone wrote a newer version since",
    ),
    (
        "versions <id>",
        "A page's versions, newest first: title, size and how each was written",
    ),
    (
        "restore <id> <version> [--base N]",
        "Make a new version with an old version's title and content",
    ),
    (
        "delete <id> --yes",
        "Delete a page, every version and its content, for good",
    ),
    (
        "share <id> [--org | --team T | --people a@x,b@y | --to a@x,b@y | --public [--expires 30d]] [--pin N]",
        "Share a page with your organization, teams, people, named outsiders or anyone with the link; --pin N pins that version, else readers see the latest",
    ),
    (
        "open <id|art_id> [<version>] [--no-browser]",
        "Open a page (or a version of it) or a shared page (its art_ id) in Nexus UI's viewer",
    ),
    (
        "frame <id|art_id> [<version>]",
        "Print a signed URL that renders the page (the latest version, or the one named) with no sign-in, for five minutes",
    ),
    (
        "export <id> [<version>] --format png|svg|pdf|pptx|html|zip [--board B] [--scale 1|2] [--out file]",
        "Write the page as a file: one board as PNG or SVG, every board as PDF, PPTX or zip, or the page as standalone HTML; --board picks one board, or a deck's slide by its title or number",
    ),
    (
        "check <id> [<version>]",
        "Check a design against its design system: colours, font sizes and spacing off its tokens, and text below WCAG AA contrast; it changes nothing",
    ),
];

/// `nexus design-system`: Nexus Design's design systems.
pub const DESIGN_SYSTEM_COMMANDS: Rows = &[
    (
        "list",
        "The organization's design systems: the built-in @nexus/brand first, and which one new designs use",
    ),
    (
        "show [<name>]",
        "A design system's link tag, tokens (CSS custom properties) and notes; the default without a name",
    ),
    (
        "sync <path> [--name N] [--default]",
        "Read a repository's tokens (CSS variables, a Tailwind config, tokens.json), its DESIGN.md and its component names into a design system (owners and admins)",
    ),
    (
        "use <name>",
        "Make a design system the default for new designs (owners and admins)",
    ),
];

/// `nexus skills`: a person's own SKILL.md files, in versions that never change.
pub const SKILL_COMMANDS: Rows = &[
    (
        "list",
        "Your skills, most recently written first: name, latest version and description",
    ),
    (
        "create <name>",
        "Write a new skill in your editor ($VISUAL or $EDITOR), from a SKILL.md skeleton",
    ),
    (
        "import <path> [--update]",
        "Add a SKILL.md, or a folder that holds one, as a new skill (version 1); only SKILL.md is sent, and the other files are named; --update writes a new version if you already have a skill of that name, unless the file is already its latest version",
    ),
    (
        "export <name|id> [<version>] [--out dir]",
        "Write a skill's SKILL.md, unchanged, to <dir>/<name>/SKILL.md. The version is a number or latest. A name that a library skill has exports that skill, at an approved version",
    ),
    (
        "delete <name|id> --yes",
        "Delete a skill, every version and its content, for good",
    ),
    (
        "promote <name|id> [<version>] --to org|team/<slug> [--note TEXT]",
        "Ask to promote a version of your skill (default: the latest) to the organization or a team you are in. The request is held until an owner, admin or team lead who is not you approves it",
    ),
    (
        "promotions [--state STATE] [--to TARGET] [--mine] [--limit N] [--cursor C]",
        "The promotion requests you can see, newest first: your own, and those you may approve. STATE is held, approved, refused or withdrawn",
    ),
    (
        "promotions <id>",
        "One promotion request: who asked and from which version, a diff from the last approved version, and the full text",
    ),
    (
        "approve <id> [--reason TEXT]",
        "Approve a held promotion request, so its version joins the library. Your sign-in decides; an API key cannot",
    ),
    (
        "refuse <id> --reason TEXT",
        "Refuse a held promotion request; the reason is required, and the author reads it",
    ),
    (
        "withdraw <id> [--reason TEXT] [--all-versions]",
        "Withdraw your held request, or (as an owner, admin or team lead) an approved version. --all-versions withdraws every approved version of that skill",
    ),
    (
        "library [--to TARGET]",
        "The skills your organization and teams approved for you to load, with scope and approved version. A name that two skills share is marked duplicate_name",
    ),
];

pub const ORG_COMMANDS: Rows = &[
    (
        "list",
        "Your organizations: Personal first, then your companies; the one you are signed in to is marked",
    ),
    (
        "waiting",
        "Your pending Gateway approvals and waiting cloud sessions, by organization",
    ),
    (
        "switch <org>",
        "Sign in to another of your organizations (its slug, or personal): a new token, saved in your config",
    ),
    (
        "settings [list]",
        "Your organization's settings: each value and its default",
    ),
    ("settings get <key>", "One setting's value"),
    (
        "settings set <key> <value>",
        "Change a setting (owners and admins): a JSON value or a word; null restores the default",
    ),
    (
        "domains [list]",
        "Your organization's verified email domains and how people on them join (owners and admins)",
    ),
    (
        "domains add <domain> [--enrollment M] [--dry-run]",
        "Add a domain you have a verified email on; the sign-in provider invites everyone with a verified email there",
    ),
    (
        "domains remove <domain> [--dry-run]",
        "Remove a domain (its name or id); the sign-in provider stops inviting people on it, and who joined stays",
    ),
    (
        "subscription [show]",
        "Your organization's subscription: plan, status, payer, billing contact and paid term",
    ),
    (
        "subscription set [--payer U] [--billing-contact U] [--effective-at T] [--dry-run]",
        "Name the payer and the billing contact (owners): a member's id or email, none clears it; Billing sets the plan, term and status",
    ),
];

pub const BUDGET_COMMANDS: Rows = &[
    (
        "list",
        "This month's spend limits: the organization's default per person, and each person's limit, spend, what is left and who is at their limit",
    ),
    (
        "show <email|userId|me>",
        "One person's month: their limit and where it comes from, spend and what is left",
    ),
    (
        "default <amount|none>",
        "Set the organization's default monthly limit per person, in USD; none clears it (owners)",
    ),
    (
        "set <email|userId> <amount|default>",
        "Set one person's monthly limit, above or below the default; default removes it (owners, and admins an owner allows; an owner's limit: owners)",
    ),
    (
        "managers [list]",
        "Who may set person limits: every owner, and each admin an owner allowed",
    ),
    (
        "managers add <email|userId>",
        "Allow an admin to set person limits (owners)",
    ),
    (
        "managers remove <email|userId>",
        "Stop an admin from setting person limits (owners)",
    ),
];

/// `nexus billing`'s subcommands, in order: [`BILLING_COMMANDS`], with the plan rows after `card add` in a build with
/// plans (`--features plans`, `plans.rs`).
pub fn billing_commands() -> Vec<(&'static str, &'static str)> {
    let rows = BILLING_COMMANDS.to_vec();
    #[cfg(feature = "plans")]
    let rows = {
        let mut rows = rows;
        rows.splice(2..2, crate::plans::PLAN_COMMANDS.iter().copied());
        rows
    };
    rows
}

/// `nexus billing`'s subcommands without plans: what `commands.json` lists.
pub const BILLING_COMMANDS: Rows = &[
    (
        "show",
        "The Stripe mode, the card on file, the free start left, Nexus credits, paying after the fact, Nexus's rate, and an unpaid month with the time model calls stop",
    ),
    (
        "card add [--no-browser] [--no-wait]",
        "Add or replace the card through Stripe Checkout, then wait until Stripe saves it (up to 10 minutes)",
    ),
    (
        "statement [--period YYYY-MM]",
        "One month's bill: each charge, the total and what is charged (default: this month so far)",
    ),
    (
        "statements",
        "The closed months, newest first: the total, the invoice's state and its page on Stripe",
    ),
    (
        "receipts",
        "Every payment, newest first: its date, what it paid for, its amount, its status and its id",
    ),
    (
        "receipts open <paymentId> [--no-browser]",
        "Open Stripe's receipt of one payment",
    ),
    (
        "portal [--no-browser]",
        "Open Stripe's billing portal: cards, invoices and tax ids",
    ),
    (
        "confirm [--no-browser]",
        "Confirm a charge the bank wants the cardholder to confirm (3-D Secure) on the bank's page: the same charge, not a new one",
    ),
    (
        "credits",
        "Nexus credits: what is left, bought, spent and held. Nexus-run AI spends them after the free start",
    ),
    (
        "credits buy <usd> [--payment-token-stdin] [--dry-run] [--no-browser] [--no-wait]",
        "Buy Nexus credits (from $25): a delegated Stripe payment token from stdin, the payment method on file, or Stripe Checkout",
    ),
    (
        "auto-topup",
        "Auto top-up of Nexus credits: on or off, its line, its amount and today's top-ups",
    ),
    (
        "auto-topup set --below <usd> --add <usd> [--dry-run]",
        "Turn it on: when credits fall below --below, Nexus adds --add ($25 to $500) with the payment method on file",
    ),
    ("auto-topup off [--dry-run]", "Turn auto top-up off"),
    (
        "after-the-fact",
        "Paying after the fact: on or off, the monthly usage limit, what the card was used for this month, what is owed and a failed charge",
    ),
    (
        "after-the-fact on [--limit <usd>] [--dry-run]",
        "Turn it on: the card on file pays for Nexus-run AI after the free start and the credits, charged each $25 of unpaid usage, up to the limit a month (default $25)",
    ),
    ("after-the-fact off [--dry-run]", "Turn paying after the fact off"),
    (
        "after-the-fact limit <usd> [--dry-run]",
        "Set the monthly usage limit; paying after the fact stays on or off as it is",
    ),
];

pub const SERVICE_ACCOUNT_COMMANDS: Rows = &[
    (
        "list",
        "The organization's service accounts and their keys (owners and admins)",
    ),
    (
        "create --name <name> --team <slug> [--owner <user-id>] --expires <YYYY-MM-DD|30d> [--dry-run]",
        "Create a service account: a caller for CI and scripts that is no member and holds no seat (owners and admins)",
    ),
    (
        "revoke <id> [--dry-run]",
        "Revoke a service account and stop every live key it holds (owners and admins)",
    ),
];

/// `nexus slack`.
pub const SLACK_COMMANDS: Rows = &[
    (
        "connect [--confirm <install-id>] [--dry-run]",
        "Connect a Slack workspace: print Slack's install link; approve the app in the browser where you are signed in to Nexus UI, which completes the install; then --confirm binds that workspace to this organization (owners and admins)",
    ),
    (
        "status",
        "The organization's Slack workspaces, pending and active (owners and admins)",
    ),
    (
        "disconnect <id> [--dry-run]",
        "Unbind a workspace: Gateway uninstalls the app from it and deletes its bot token, unless another binding of the organization still uses it (owners and admins)",
    ),
];

/// Under `nexus slack channel`.
pub const SLACK_CHANNEL_COMMANDS: Rows = &[
    (
        "list <workspace id>",
        "The workspace's channels bound to a team (owners and admins: every one; a lead: those of the teams they lead)",
    ),
    (
        "bind <workspace id> <channel id> --team <slug>",
        "Bind a channel to a team: its calls go to that team, under its policy and caps (owners and admins: any team; a lead: a team they lead)",
    ),
    ("unbind <workspace id> <channel id>", "Unbind a channel from its team"),
];

/// Under `nexus service-accounts keys`.
pub const SERVICE_ACCOUNT_KEY_COMMANDS: Rows = &[
    (
        "create <account id> --label <label> [--expires <YYYY-MM-DD|30d>]",
        "Issue a key for a service account: its token (nxsa_…) is shown once",
    ),
    (
        "rotate <account id> <key id> [--label <label>] [--expires <YYYY-MM-DD|30d>]",
        "Issue a new key in place of one and stop the old one at once: it keeps the label and lifetime unless you set them; the new token is shown once",
    ),
    ("revoke <account id> <key id>", "Revoke one key at once"),
];

/// Under `nexus jobs`.
pub const JOB_COMMANDS: Rows = &[
    (
        "submit <product>/<capability> <verb> [--input <json>] [--team <slug>]",
        "Submit a job: a module's verb with its input, run in the background for you; prints the job's id. The module is <product>/<capability>, such as robustness/review",
    ),
    (
        "get <id>",
        "A job's state: accepted, running, done, failed or cancelled",
    ),
    (
        "cancel <id>",
        "Cancel a job no worker has taken yet; a running or finished job stays as it is",
    ),
    ("result <id>", "A job's result, once it is done or failed"),
];

/// Under `nexus routines` (W4.1).
pub const ROUTINE_COMMANDS: Rows = &[
    (
        "list [--all]",
        "Your routines, newest first, with each one's next run; --all lists the organization's (owners and admins)",
    ),
    (
        "show <id>",
        "A routine: its trigger, action, proactivity, caps, state and next run",
    ),
    (
        "create --at <HH:MM> [--days mon-fri] | --every <interval> [--timezone <zone>] [--title <title>] [--max-runs-per-day <n>] [--team <slug>]",
        "Create a morning brief on a schedule, for you only; prints its id and next run. The brief reads only, and reports to your Slack direct messages",
    ),
    ("pause <id>", "Pause a routine: its schedule stops before its next slot"),
    (
        "resume <id>",
        "Resume your routine from its next slot after now; nothing missed is replayed",
    ),
    ("delete <id>", "Delete a routine and its schedule"),
];

pub const APP_COMMANDS: Rows = &[
    (
        "deploy [dir] [--name N] [--framework F]",
        "Deploy a folder (default: this one) to Vercel; deploying the same name again ships its next version. Prints its URL",
    ),
    (
        "list",
        "Your apps (owners and admins: the organization's), with what each cost Nexus",
    ),
    ("cap <id> <usd>", "Set this app's monthly hosting cap in USD"),
    ("get <id>", "An app: its URL, its deployment's state and its cost"),
    ("delete <id> --yes", "Delete an app and its Vercel project"),
    (
        "claim <id>",
        "A link that moves the app to your own Vercel account, where it stops costing Nexus; it works for 24 hours",
    ),
];

pub const AUDIT_OPTIONS: Rows = &[
    (
        "--since 7d",
        "From this long ago (30m, 24h, 7d, 2w) or an ISO time; default 7d",
    ),
    ("--until <time>", "Up to this time"),
    (
        "--action <a>",
        "An action (team.moved), a list (a,b) or a prefix (team.*)",
    ),
    ("--actor <email>", "Changes made by this person (email or user id)"),
    ("--team <slug>", "Changes to this team and the teams under it"),
    ("--target <id>", "Changes to this target (id or slug)"),
    (
        "--limit <n>",
        "Events per page (1 to 500, default 50); --cursor <c> for the next page",
    ),
    (
        "--csv",
        "Every matching event as CSV on stdout (owners, admins and team leads)",
    ),
];

pub struct HelpGroup {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

/// One `nexus help` row per launcher (`claude [args]`): through Gateway or on its plan, or on its plan only.
fn launcher_rows() -> Vec<(String, String)> {
    crate::agent_routes::launchers()
        .map(|agent| {
            let identity = agent.entry().identity;
            let text = if agent.entry().billing.gateway.is_some() {
                format!(
                    "{} through Nexus Gateway, or on your {} plan (--subscription)",
                    identity.name, identity.plan
                )
            } else {
                format!("{} on your {} plan", identity.name, identity.plan)
            };
            (format!("{} [args]", agent.command()), text)
        })
        .collect()
}

/// The staff-only rows of `nexus help` (`--features staff`, on by default). None in the open-source build.
fn staff_rows() -> Vec<(String, String)> {
    #[cfg(feature = "staff")]
    let rows = vec![(
        format!("triage [{}]", crate::triage::agents("|")),
        "Nexus staff: an agent diagnoses a Nexus defect on this computer and sends the report to the Nexus team".into(),
    )];
    #[cfg(not(feature = "staff"))]
    let rows = Vec::new();
    rows
}

/// A table's rows, owning their strings.
fn owned(rows: Rows) -> Vec<(String, String)> {
    rows.iter()
        .map(|(usage, text)| (usage.to_string(), text.to_string()))
        .collect()
}

/// The rows of `nexus help`, by group; `Products` is filled from the Gateway's modules between the first two groups.
pub fn help_groups() -> Vec<HelpGroup> {
    vec![
    HelpGroup {
        title: "Getting started",
        rows: owned(&[
            (
                "auth login",
                "Sign in through Nexus UI (--url for another Gateway, --with-token for CI)",
            ),
            (
                "auth signup --name <name>",
                "Create an organization and save its owner's API key without a browser",
            ),
            (
                "auth status",
                "Your org, user and role, and whether Gateway accepts your token",
            ),
            ("auth logout", "Revoke and forget the saved token"),
            (
                "auth tokens [create|…]",
                "Your API keys for CI and scripts: list, create (shown once), revoke",
            ),
            ("keys add <provider> <key>", "Bring your own provider key (BYOK)"),
            ("keys list", "Your provider keys, masked"),
        ]),
    },
    HelpGroup {
        title: "Teams",
        rows: owned(&[
            (
                "team list|tree|show|create|…",
                "Teams in your organization, nested; owners and admins manage them",
            ),
            (
                "team import <file>",
                "Load your teams, leads and members from a YAML or JSON file",
            ),
            (
                "team use <slug>",
                "Your default team: where your spend goes when you name none",
            ),
            (
                "analytics [--by team]",
                "Usage by team, person, model, provider, key, product or day (--depth, --under, --from, --to)",
            ),
            (
                "budget set|show org|team/<slug> …",
                "The organization's or a team's spend cap per day, week or month: owners and admins set it; everyone sees what is left",
            ),
        ]),
    },
    HelpGroup {
        title: "Models",
        rows: owned(&[
            (
                "models [--all]",
                "The models you may use (--all: the catalog, coming soon too)",
            ),
            (
                "chat \"prompt\" [--new]",
                "Chat sessions: continue yours or start one (--model, --team, --image)",
            ),
            (
                "chat sessions [list|show|…]",
                "Your chat sessions: list, show, images, rename, archive, delete",
            ),
            (
                "chat share <session> --team T",
                "Share a session as an artifact: inside your organization (--org, --people) or outside it (--to, --public)",
            ),
            (
                "artifacts [list|show|…]",
                "Artifacts shared with you and by you: show, export, images, continue, access, revoke",
            ),
            (
                "pages [publish|list|get|…]",
                "Your pages: HTML or Markdown files in versions that never change (update, restore, delete)",
            ),
            (
                "design-system [list|show|sync|use]",
                "Nexus Design's design systems: the tokens and notes designs are made, served and checked with",
            ),
            (
                "skills [list|create|import|…]",
                "Your skills: SKILL.md files in versions that never change (export, delete), and promoting them to your organization or a team (library)",
            ),
            (
                "policy show|set|test|controls|pause|limit|override|clear|connectors|connector|grants|review-events|agent-writes",
                "Model access: allow and deny lists per organization, team and agent; which connectors members may use",
            ),
        ]),
    },
    HelpGroup {
        title: "Account",
        rows: owned(&[
            (
                "org [list|switch <org>]",
                "Your organizations: Personal first, then your companies; switch to another",
            ),
            (
                "org settings [get|set]",
                "Your organization's settings: chat retention, subscription mode, sharing artifacts outside it",
            ),
            (
                "org domains [add|remove]",
                "Verified email domains: everyone with a verified email there joins (owners and admins)",
            ),
            (
                "org subscription [set]",
                "The subscription: plan, status, payer, billing contact and paid term (owners name the payer and contact)",
            ),
            (
                "members [list|pending]",
                "Everyone in your organization; who is in teams but has not joined",
            ),
            (
                "audit [--since 7d] [--csv]",
                "Who changed what, from where (owners and admins: everything)",
            ),
            (
                "members invite …",
                "Invite by email or --pending (the sign-in provider emails them; --dry-run, --yes)",
            ),
            (
                "spend [--by team]",
                "Budget and spend by product and capability, or by team",
            ),
            (
                "budgets [list|show|default|set|managers]",
                "Monthly spend limits per person: the default and overrides (owners set the default; owners, and admins an owner allows, set overrides)",
            ),
            (
                "billing [card add|statement|statements|receipts|portal|credits|auto-topup|after-the-fact]",
                "The card on file, an unpaid month, the monthly statements, the receipts, Stripe's portal, Nexus credits, their auto top-up and paying after the fact (owners)",
            ),
            (
                "service-accounts [list|create|revoke|keys]",
                "Callers for CI and scripts, and their keys (owners and admins; --dry-run)",
            ),
            (
                "slack [connect|status|disconnect|channel]",
                "Bind a Slack workspace to your organization through Slack's install (owners and admins; --dry-run), and a channel of it to a team, so its policy and caps apply (leads for their teams)",
            ),
            ("modules", "Registered products and capabilities"),
            (
                "jobs [submit|get|cancel|result]",
                "A module's verb run in the background: its state, its result, or cancel it",
            ),
            (
                "routines [list|show|create|pause|resume|delete]",
                "Your routines, such as a morning brief on a schedule: they run as you, read only, and report to Slack",
            ),
            (
                "apps [deploy|list|get|cap|delete|claim]",
                "Deploy a folder to Vercel and get its URL; claim it to your own Vercel account (when the Gateway turns Nexus Apps on)",
            ),
            (
                "connectors",
                "Connectors Gateway offers, with their read and write operations",
            ),
            (
                "connections",
                "Your accounts at a connector's vendor, and the organization's",
            ),
            (
                "connect <connector> [--org] [field=value …]",
                "Connect an account: a key form (a value of - reads stdin, and @<file> a file) or an OAuth link",
            ),
            (
                "disconnect <connector> <account> [--org]",
                "Disconnect one of your accounts, or with --org the organization's",
            ),
            (
                "machines [list|show|sleep]",
                "Your cloud machines: which run, why one sleeps; put one to sleep",
            ),
            (
                "notifications [list|read|done|kinds]",
                "Your inbox: what needs your attention in this organization; mark an item read or done",
            ),
            (
                "search \"<query>\" [--type T]",
                "Your chats, people, teams and models: what your role may read (⌘K in Nexus UI)",
            ),
        ]),
    },
    HelpGroup {
        title: "Agents",
        rows: [
        owned(&[
            (
                "agent ask \"<question>\"",
                "Ask Nexus, the same one as @nexus in Slack: its memory, its model, its sources",
            ),
            ("agent chat", "Talk with Nexus in one conversation, in this terminal"),
        ]),
        launcher_rows(),
        staff_rows(),
        owned(&[
            (
                "agents",
                "What is installed and signed in, each agent's route, who pays, and what is left to set up",
            ),
            (
                "agents use <agent> <route>",
                "The route an agent takes when you name none: gateway or subscription",
            ),
            (
                "agents setup <agent>",
                "How to run an agent through Gateway, from its recipe: the limits first, then each step",
            ),
            ("memory <command>", "Manage local shared memory for coding agents"),
            (
                "recordings [list|show|path|frames]",
                "Screen recordings from Nexus for Mac (⌥⇧S): nexus claude --recording latest starts on one",
            ),
            (
                "sessions [list|show|start|send|stop]",
                "Code sessions in Nexus for desktop, from scripts and other agents (turn on Outside access in the app)",
            ),
            (
                "mcp",
                "Nexus as tools for any MCP agent, under your sign-in; owners and admins manage the organization: claude mcp add nexus -- nexus mcp",
            ),
        ]),
        ]
        .concat(),
    },
    HelpGroup {
        title: "CLI",
        rows: owned(&[
            (
                "update [--check|--rollback]",
                "Update this CLI to the version your Gateway publishes",
            ),
            (
                "completion <bash|zsh|fish|install>",
                "Tab-completion for commands, subcommands and flags: install turns it on, a shell prints its script to source",
            ),
            (
                "help [--json]",
                "This list; --json: every command, flag and exit code, for scripts",
            ),
        ]),
    },
]
}

const GLOBAL_FLAGS: &[(&str, &str)] = &[
    (
        "--json",
        "Print only JSON on stdout; errors go to stderr as {\"error\": {\"message\", \"hint\"}}",
    ),
    ("--no-color", "Plain text on a terminal too (also NO_COLOR)"),
    ("--help", "Help for a command (also -h)"),
    ("--version", "The CLI's version (also -v; with --json: {\"version\"})"),
];

const EXIT_CODES: &[(i64, &str)] = &[
    (0, "Success"),
    (
        1,
        "Failure: Gateway refused or failed the request, a run failed, you are not signed in, or Gateway could not be reached",
    ),
    (
        2,
        "Usage error: an unknown command, a missing argument or an invalid flag",
    ),
];

const ENVIRONMENT: &[(&str, &str)] = &[
    (
        "NEXUS_URL",
        "The Gateway URL when none is saved (the installer's wrapper sets it; default https://api.nexus.inc)",
    ),
    (
        "NEXUS_GATEWAY",
        "One command against another Gateway, with its own sign-in (nexus auth login once per Gateway); the saved one stays the default",
    ),
    (
        "NEXUS_CONFIG",
        "The CLI config file (default ~/.nexus/factory/config.json)",
    ),
    (
        "NEXUS_UI_URL",
        "Where the CLI's Nexus UI links point, for the Gateway NEXUS_URL names (any Gateway when NEXUS_URL is unset)",
    ),
    (
        "NEXUS_TEAM",
        "The team runs and agent sessions are attributed to (--team wins)",
    ),
    (
        "NEXUS_CLAUDE_AUTH",
        "gateway or subscription: the route nexus claude takes in this shell (a flag wins; unset: your saved route, nexus agents)",
    ),
    (
        "NEXUS_CODEX_AUTH",
        "gateway or subscription: the route nexus codex takes in this shell (a flag wins; unset: your saved route)",
    ),
    (
        "NEXUS_ANTIGRAVITY_AUTH",
        "subscription, its only route: the route nexus agy takes in this shell (Antigravity runs on your Google AI plan)",
    ),
    (
        "NEXUS_CURSOR_AUTH",
        "subscription: run nexus cursor-agent on your Cursor plan",
    ),
    (
        "NEXUS_RECORDINGS_DIR",
        "Where nexus recordings reads screen recordings (default: Nexus for Mac's folder)",
    ),
    (
        "NEXUS_DESKTOP_BIN",
        "The binary nexus recordings frames runs (default: Nexus.app's nexus-desktop, else Nexus GPUI's nexus-desktop-gpui)",
    ),
    (
        "NEXUS_DESKTOP_SOCKET",
        "The socket nexus sessions reaches Nexus for desktop through (default: the app's, in $XDG_RUNTIME_DIR or its data folder)",
    ),
    (
        "NEXUS_NO_UPDATE_CHECK",
        "1: never check for a newer CLI (also off with --json, in CI and when stdout is not a terminal)",
    ),
    ("NO_COLOR", "Plain output on a terminal too"),
    ("NEXUS_DEBUG", "1: print the stack trace with an error"),
];

/// What a flag means, by command (`"team create --level"`), then by name.
const FLAG_TEXT: &[(&str, &str)] = &[
    ("memory --repo", "Choose the repository whose notes to use"),
    (
        "memory disable --export",
        "Save a Markdown copy to this path before turning off shared memory",
    ),
    (
        "memory disable --skip-export",
        "Turn off shared memory without exporting",
    ),
    ("memory export --output", "Write a Markdown export to this path"),
    (
        "--url",
        "The Gateway to sign in to, saved as the default (default: NEXUS_GATEWAY, else the saved one, else NEXUS_URL, else https://api.nexus.inc)",
    ),
    (
        "--with-token",
        "Read an API key from stdin instead of signing in through the browser",
    ),
    ("--no-browser", "Only print the approval link; do not open a browser"),
    ("--depth", "How many levels of teams to show or roll up"),
    ("--parent", "The parent team's slug"),
    ("--root", "Put the team at the top, under no parent"),
    ("--level", "Your own label for the team's level (e.g. department)"),
    ("--name", "A display name"),
    ("auth signup --name", "The new organization's name (1 to 80 characters)"),
    (
        "auth signup --email",
        "The owner's email address (unverified until verified through sign-in)",
    ),
    (
        "apps --name",
        "The app's name: lowercase words joined by dashes (default: the folder's name)",
    ),
    (
        "apps --framework",
        "A Vercel framework preset, such as nextjs or vite (default: Vercel detects it)",
    ),
    (
        "devices pair --name",
        "This computer's name in your devices (default: nexus CLI on <hostname>)",
    ),
    ("devices pair --new", "Forget this computer's key and pair a new one"),
    (
        "devices pair --no-wait",
        "Show the code and return without waiting for the approval",
    ),
    (
        "devices pair --approver-code",
        "Confirm the approver without a prompt: the code its own screen shows",
    ),
    ("devices revoke --yes", "Revoke without asking"),
    (
        "devices org-key --fingerprint",
        "Confirm your org's recovery key without a prompt: the fingerprint computed from your vault",
    ),
    (
        "devices org-key --vault-public-key",
        "Compute a fingerprint offline from your vault's X25519 public-key export, raw 32 bytes or base64url",
    ),
    ("--description", "A description"),
    ("--yes", "Confirm a change that deletes or sends more than one thing"),
    ("--cascade", "Also delete every team under it"),
    ("--lead", "Add the person as a lead"),
    ("--home", "Make it the person's default team"),
    ("--team", "A team's slug"),
    ("--cancel", "Cancel a pending member (email or id)"),
    ("--dry-run", "Show what would change, change nothing"),
    (
        "auth delete-account --confirm",
        "DELETE, to confirm without a terminal (on one, you type it when asked)",
    ),
    ("keys --team", "The team that holds the key: its slug or id"),
    ("keys --user", "The person who holds the key: me, or their user id"),
    (
        "--enrollment",
        "How people with a verified email on the domain join: automatic_invitation (the default), automatic_suggestion or manual_invitation",
    ),
    ("--plan", "Apply exactly the plan a dry run printed"),
    (
        "org subscription --payer",
        "The member who pays: their email or user id (none clears it)",
    ),
    (
        "org subscription --billing-contact",
        "The member who receives billing mail: their email or user id (none clears it)",
    ),
    (
        "org subscription --effective-at",
        "When the change took effect, an ISO 8601 time (default now; never in the future)",
    ),
    ("--prune", "Also remove teams and memberships missing from the file"),
    (
        "--invite",
        "Also invite the file's people who are not in the organization yet",
    ),
    ("--role", "The role to invite as (member or admin)"),
    ("--pending", "Everyone added to a team by email who has not joined yet"),
    (
        "--allow",
        "Allowed model patterns, comma-separated (all clears the list)",
    ),
    (
        "--deny",
        "Denied model patterns, comma-separated (none clears the list)",
    ),
    ("--default", "The default model"),
    (
        "--reason",
        "Why the control is needed; the audit log and the people it applies to can read it",
    ),
    (
        "--models",
        "The models a limit narrows to, or an override allows, comma-separated",
    ),
    (
        "policy --expires",
        "How long the control stays in force: 30d, 12h or a number of days",
    ),
    (
        "policy --teams",
        "Only these teams' members and the teams under them, comma-separated slugs (default: everyone)",
    ),
    (
        "policy --connector",
        "The connector the grants are for (github; the default with --for-mode)",
    ),
    (
        "policy --actions",
        "The writes the module holds, in full, comma-separated (empty revokes them all)",
    ),
    (
        "policy --for-mode",
        "Grant Nexus Review exactly what this mode needs: shadow none, comment pull_review, comment_and_check pull_review,check_run",
    ),
    ("--from", "A whole rule from a YAML or JSON file"),
    ("--clear", "Remove the level's own rule"),
    ("--model", "A model (provider/model, or auto)"),
    ("--as", "Test as this person (email or user id)"),
    ("--by", "Group by team, person, model, provider, key, product or day"),
    ("--days", "The last N days (1 to 366)"),
    ("--under", "Only teams under this one"),
    ("--to", "Until this day (YYYY-MM-DD)"),
    ("--all", "The whole catalog, coming-soon models included"),
    (
        "search --type",
        "chats, artifacts, people, teams or models; comma-separated for several (default: all)",
    ),
    ("search --limit", "Results per type, 1 to 20 (default 5)"),
    ("--mode", "hard_stop (warn and approval are not available yet)"),
    ("--warn-at", "For mode warn, which is not available yet"),
    ("--no-wait", "Start the run and return its id without waiting"),
    (
        "--payment-token-stdin",
        "Read the agent wallet's Stripe shared payment token (spt_) from stdin; no browser or saved card needed",
    ),
    ("--check", "Only check; change nothing"),
    ("--rollback", "Restore the version before the last update"),
    ("--json", "Print only JSON on stdout"),
    (
        "billing card --no-browser",
        "Only print the Stripe Checkout link; do not open a browser",
    ),
    (
        "billing card --no-wait",
        "Print the link and return without waiting for the card",
    ),
    (
        "billing statement --period",
        "The month, as YYYY-MM in UTC (default: this month so far)",
    ),
    (
        "billing receipts --no-browser",
        "Only print the receipt link; do not open a browser",
    ),
    (
        "billing portal --no-browser",
        "Only print the portal link; do not open a browser",
    ),
    (
        "billing confirm --no-browser",
        "Only print the bank's page; do not open a browser",
    ),
    (
        "billing credits --dry-run",
        "Say what `credits buy` would charge, and charge nothing",
    ),
    (
        "billing credits --no-browser",
        "Only print the Stripe Checkout link; do not open a browser",
    ),
    (
        "billing credits --no-wait",
        "Return without waiting for Stripe to take the payment",
    ),
    (
        "billing auto-topup --below",
        "Top up when the credits left fall below this many dollars",
    ),
    ("billing auto-topup --add", "Dollars one top-up buys, from 10 to 500"),
    (
        "billing auto-topup --dry-run",
        "Say what would change, and change nothing",
    ),
    (
        "billing after-the-fact --limit",
        "The monthly usage limit in dollars (default 25)",
    ),
    (
        "billing after-the-fact --dry-run",
        "Say what would change, and change nothing",
    ),
    ("analytics --from", "From this day (YYYY-MM-DD)"),
    ("analytics --team", "Only this team's usage"),
    (
        "analytics --person",
        "With --csv or --jsonl: only this person's usage (a user id)",
    ),
    (
        "analytics --model",
        "With --csv or --jsonl: only this model; - for calls with none",
    ),
    (
        "analytics --provider",
        "With --csv or --jsonl: only this provider; - for calls with none",
    ),
    (
        "analytics --product",
        "With --csv or --jsonl: only this product, or product/capability",
    ),
    (
        "analytics --csv",
        "Write the usage file as CSV, one row per day, team, person, model, provider and product; its manifest is in the audit log",
    ),
    (
        "analytics --jsonl",
        "Write the usage file as JSON Lines; its last line is the manifest",
    ),
    (
        "analytics --out",
        "With --csv or --jsonl: write the file here instead of stdout",
    ),
    (
        "analytics --cursor",
        "With --csv or --jsonl: continue a file the row limit stopped, from its manifest's nextCursor",
    ),
    (
        "spend --from",
        "With --by, calls, --csv or --jsonl: from this day (YYYY-MM-DD, UTC; at most 366 days in all)",
    ),
    (
        "spend --to",
        "With --by, calls, --csv or --jsonl: until this day, included (YYYY-MM-DD, UTC)",
    ),
    (
        "spend --team",
        "With --by, calls, --csv or --jsonl: only this team and the teams under it; with calls or a file, unassigned for calls with no team",
    ),
    (
        "spend --person",
        "With calls, --csv or --jsonl: only this person's calls (a user id)",
    ),
    (
        "spend --model",
        "With calls, --csv or --jsonl: only this model; - for calls with none",
    ),
    (
        "spend --provider",
        "With calls, --csv or --jsonl: only this provider; - for calls with none",
    ),
    (
        "spend --product",
        "With calls, --csv or --jsonl: only this product, or product/capability (Analytics' key)",
    ),
    (
        "spend --status",
        "With calls, --csv or --jsonl: only these statuses, comma-separated: settled, failed, refused, released",
    ),
    (
        "spend --session",
        "With calls, --csv or --jsonl: only this agent or chat session's calls (its id)",
    ),
    (
        "spend --limit",
        "With calls: at most this many calls on a page (1 to 500, default 50)",
    ),
    (
        "spend --cursor",
        "With calls, --csv or --jsonl: the next page of calls, or where a stopped file continues (its manifest's nextCursor)",
    ),
    (
        "spend --csv",
        "Write the matching calls as CSV, at most 100,000 rows a file; its manifest (rows, nextCursor for the rest) is in the audit log",
    ),
    (
        "spend --jsonl",
        "Write the matching calls as JSON Lines, at most 100,000 rows a file; its last line is the manifest",
    ),
    (
        "spend --out",
        "With --csv or --jsonl: write the file here instead of stdout",
    ),
    (
        "agent --conversation",
        "Continue this conversation: the id the last answer printed (default: a new one)",
    ),
    (
        "agent --model",
        "A catalog model id for this answer (default: your saved model)",
    ),
    (
        "agents --from",
        "With --csv or --jsonl: sessions started from this day (YYYY-MM-DD, UTC)",
    ),
    (
        "agents --to",
        "With --csv or --jsonl: sessions started until this day, included",
    ),
    (
        "agents --team",
        "With --csv or --jsonl: only this team's sessions and the teams under it",
    ),
    (
        "agents --person",
        "With --csv or --jsonl: only this person's sessions (a user id)",
    ),
    ("agents --via", "With --csv or --jsonl: nexus, subscription or ori"),
    (
        "agents --status",
        "With --csv or --jsonl: only these statuses, comma-separated: running, succeeded, failed, interrupted",
    ),
    (
        "agents --cursor",
        "With --csv or --jsonl: continue a file the row limit stopped, from its manifest's nextCursor",
    ),
    (
        "agents --csv",
        "Write the organization's agent sessions as CSV (its newest 500 at most); the manifest is in the audit log",
    ),
    (
        "agents --jsonl",
        "Write the organization's agent sessions as JSON Lines; its last line is the manifest",
    ),
    (
        "agents --out",
        "With --csv or --jsonl: write the file here instead of stdout",
    ),
    (
        "spend --by",
        "Show the analytics table grouped by team, person, model, provider, key, product or day",
    ),
    ("models --team", "The models allowed in this team"),
    (
        "chat --team",
        "The team the session's spend is attributed to (default: yours)",
    ),
    ("--session", "Continue this chat session (its id)"),
    ("--new", "Start a new chat session instead of continuing the last one"),
    (
        "chat --model",
        "A model you may use (provider/model), or auto; also changes the session's model",
    ),
    (
        "chat --image",
        "Attach images to the prompt: PNG, JPEG, GIF or WebP files, comma-separated (up to 8, at most 5 MB each)",
    ),
    (
        "chat --no-web",
        "Turn web search off for this session (it is on by default when your organization allows it)",
    ),
    ("--search", "Only sessions with these words in the title or messages"),
    ("--limit", "At most this many (1 to 100)"),
    ("--undo", "Undo it: restore an archived session"),
    ("--org", "Share with everyone in your organization"),
    (
        "connect --org",
        "The organization's account, not your own (owners and admins)",
    ),
    (
        "connect --write",
        "Allow writes: the OAuth link asks the vendor for the write scopes, not only the reads",
    ),
    (
        "disconnect --org",
        "The organization's account, not your own (owners and admins)",
    ),
    (
        "--private",
        "Stop sharing: revoke every version, so only you can read it",
    ),
    (
        "--replace",
        "Revoke the session's earlier versions, so only this audience can open it",
    ),
    (
        "chat share --team",
        "Share with these teams (slugs, comma-separated) and the teams under them",
    ),
    (
        "--people",
        "Share with these people in your organization (emails, comma-separated)",
    ),
    (
        "--allow-continue",
        "Let readers continue it in a new private chat of their own",
    ),
    (
        "agents share --team",
        "Share with these teams (slugs, comma-separated) and the teams under them",
    ),
    (
        "agents share --to",
        "Share outside your organization with these people (emails, comma-separated); they sign in with that email to open it",
    ),
    (
        "--include-thinking",
        "Keep the agent's thinking in the shared session (left out by default)",
    ),
    (
        "chat share --to",
        "Share outside your organization with these people (emails, comma-separated); they sign in with that email to open it",
    ),
    (
        "--public",
        "Share with anyone who has the link (unguessable, not indexed by search engines)",
    ),
    (
        "--expires",
        "When a public link stops working: 30d, 12h or a number of days",
    ),
    (
        "--pin",
        "Share this version of the page; without it, readers see the latest version",
    ),
    (
        "pages share --team",
        "Share with these teams (slugs, comma-separated) and the teams under them",
    ),
    (
        "pages share --to",
        "Share outside your organization with these people (emails, comma-separated); they sign in with that email to open it",
    ),
    (
        "--include-code",
        "Keep an old code session's sources in a share outside your organization (only if your organization allows it)",
    ),
    (
        "--external",
        "Everything shared outside your organization (owners and admins)",
    ),
    ("--audience", "external (named people) or public (anyone with the link)"),
    ("--author", "Only what this person shared (email or user id)"),
    (
        "artifacts --team",
        "Only artifacts shared from this team's sessions and the teams under it",
    ),
    ("artifacts --search", "Only artifacts with these words in the title"),
    ("--grant", "Give these people access (emails, comma-separated)"),
    (
        "artifacts --deny",
        "Refuse these people's requests (emails, comma-separated)",
    ),
    (
        "--remove",
        "Take access away from these people (emails, comma-separated)",
    ),
    ("--message", "A note for the author"),
    ("--md", "As Markdown (the only format for now)"),
    ("--out", "Write it to this file instead of stdout"),
    (
        "chat sessions images --out",
        "The directory to save the images in (default: the working directory; made if it is missing)",
    ),
    (
        "artifacts images --out",
        "The directory to save the images in (default: the working directory; made if it is missing)",
    ),
    ("--force", "Overwrite files that are already there"),
    ("--mine", "Only the artifacts you shared"),
    ("--shared-with-me", "Only the artifacts others shared with you"),
    ("--revoked", "Only your revoked artifacts"),
    ("artifacts --all", "Active and revoked artifacts"),
    (
        "pages --title",
        "The page's title (default: its <title> or first # heading, else the file name)",
    ),
    (
        "pages --format",
        "html, md, jsx, sheet or board (default: from the file name; .md and .markdown are Markdown, .jsx and .tsx are React, .sheet.json is a sheet, .board.json is a board)",
    ),
    (
        "pages --base",
        "The version this was built on: refused if someone wrote a newer version since",
    ),
    ("pages --source", "The source as written, not the version's details"),
    ("pages --out", "Write the source to this file instead of stdout"),
    (
        "pages export --format",
        "png or svg (one board), pdf (one page per board), pptx (one slide per board), html (the page, opens offline) or zip",
    ),
    (
        "pages export --board",
        "One board or slide, by its data-id, its name or its number from 1; a PNG or SVG of a design or a deck with several needs one",
    ),
    (
        "pages export --scale",
        "1 or 2: the pixel ratio the boards render at (default: 2 for pdf and pptx, else 1)",
    ),
    (
        "pages export --out",
        "Write the file here (default: <id>[-<board>].<format> in the working directory)",
    ),
    (
        "design-system --name",
        "The design system's name (default: the folder's package.json name, else the folder's own)",
    ),
    ("design-system --default", "Make it the default for new designs too"),
    (
        "skills --update",
        "Write a new version if you already have a skill of that name, unless the file is already its latest version",
    ),
    (
        "skills --out",
        "The folder to write <name>/SKILL.md in (default: the working directory; made if it is missing)",
    ),
    (
        "skills promote --to",
        "Where to promote it: org, or team/<slug> for a team you are in",
    ),
    (
        "skills promotions --to",
        "Only requests to this target: org or team/<slug>",
    ),
    (
        "skills library --to",
        "Only skills promoted to this target: org or team/<slug>",
    ),
    (
        "skills --note",
        "Why you ask, for the approvers (at most 500 characters)",
    ),
    (
        "skills approve --reason",
        "Why you approve it, for the author (optional, at most 500 characters)",
    ),
    (
        "skills refuse --reason",
        "Why you refuse it, for the author (required, at most 500 characters)",
    ),
    (
        "skills withdraw --reason",
        "Why you withdraw it, for the author and the audit log (optional, at most 500 characters)",
    ),
    (
        "skills --state",
        "Only requests in this state: held, approved, refused or withdrawn",
    ),
    ("skills --mine", "Only the promotion requests you made"),
    (
        "skills --all-versions",
        "Withdraw every approved version of that library skill, not only this one",
    ),
    ("skills --limit", "Requests per page (1 to 200, default 50)"),
    ("skills --cursor", "The next page: the cursor the last page printed"),
    (
        "claude --team",
        "The team this session's spend goes to (also NEXUS_TEAM)",
    ),
    (
        "claude --recording",
        "Start on a screen recording from Nexus for Mac (⌥⇧S): its id, or latest (the default); Claude Code is told how to read it",
    ),
    (
        "codex --recording",
        "Start on a screen recording from Nexus for Mac (⌥⇧S): its id, or latest (the default); Codex gets its contact sheet",
    ),
    (
        "recordings frames --from",
        "From this moment of the recording (0:12 as summary.md writes it, or seconds)",
    ),
    ("recordings frames --to", "Until this moment"),
    (
        "recordings frames --count",
        "How many frames, evenly spread (default 8, at most 60)",
    ),
    (
        "sessions --project",
        "The project: its folder (. is the current one), its id, or GitHub owner/repo",
    ),
    ("sessions --archived", "Archived sessions too"),
    ("sessions --limit", "At most this many sessions (1 to 200, default 50)"),
    ("sessions --turns", "How many of the latest turns (1 to 20, default 3)"),
    ("sessions --model", "The model; default: the project's"),
    (
        "sessions --mode",
        "The permission mode: plan, default, acceptEdits, auto or bypassPermissions; never above the project's default",
    ),
    (
        "sessions --worktree",
        "Work in a new worktree; default: the project's choice",
    ),
    (
        "sessions --folder",
        "Work in the project's folder; default: the project's choice",
    ),
    ("sessions --read-only", "Ask only to read sessions"),
    (
        "sessions --name",
        "The name the app shows for this client (default: nexus CLI)",
    ),
    (
        "sessions start --name",
        "The session's unique name, which Claude Code's ListAgents and SendMessage reach it by",
    ),
    ("sessions name --clear", "Remove the session's name"),
    (
        "codex --team",
        "The team this session's spend goes to (also NEXUS_TEAM)",
    ),
    (
        "claude --subscription",
        "Use your Claude subscription (claude auth login), not Gateway: model calls are not metered or checked (also NEXUS_CLAUDE_AUTH=subscription)",
    ),
    (
        "codex --subscription",
        "Use your ChatGPT plan (codex login), not Gateway: model calls are not metered or checked (also NEXUS_CODEX_AUTH=subscription)",
    ),
    (
        "claude --api",
        "Through Gateway for this run, whatever route you saved (also NEXUS_CLAUDE_AUTH=gateway)",
    ),
    (
        "codex --api",
        "Through Gateway for this run, whatever route you saved (also NEXUS_CODEX_AUTH=gateway)",
    ),
    (
        "claude --via",
        "The route for this run: gateway or subscription; default: your saved route (nexus agents)",
    ),
    (
        "codex --via",
        "The route for this run: gateway or subscription; default: your saved route (nexus agents)",
    ),
    (
        "claude --model",
        "Claude Code's own flag, read by Nexus: auto runs on the model Gateway chooses",
    ),
    (
        "codex --model",
        "Codex's own flag, read by Nexus: auto runs on the model Gateway chooses",
    ),
    ("claude --json", "A JSON summary on stdout when the agent exits"),
    ("codex --json", "A JSON summary on stdout when the agent exits"),
    ("agy --json", "A JSON summary on stdout when the agent exits"),
    (
        "agy --subscription",
        "Run on your Google AI plan, the default (also NEXUS_ANTIGRAVITY_AUTH=subscription)",
    ),
    ("agy --team", "The team this session is attributed to (also NEXUS_TEAM)"),
    (
        "agy --model",
        "agy's own flag: the model it runs, by its id (auto is not available)",
    ),
    ("cursor-agent --json", "A JSON summary on stdout when Cursor exits"),
    ("cursor-agent --subscription", "Run on your Cursor plan"),
    (
        "cursor-agent --team",
        "The team this session is attributed to (also NEXUS_TEAM)",
    ),
    ("cursor-agent --model", "Cursor's own model flag"),
    (
        "slack channel bind --team",
        "The team the channel's calls go to, by slug or id (a lead: a team they lead or one under it)",
    ),
    (
        "slack connect --confirm",
        "The install id (chw_…) that nexus slack connect printed: binds its workspace once you approved the app in Slack",
    ),
    (
        "auth tokens --all",
        "Everyone's tokens in the organization (owners and admins)",
    ),
    (
        "auth tokens create --label",
        "A name for the key (1 to 80 characters), to tell keys apart",
    ),
    (
        "service-accounts create --name",
        "The service account's name (1 to 64 characters)",
    ),
    (
        "service-accounts create --team",
        "The team that owns it, by slug or id: its keys stop when the team is deleted",
    ),
    (
        "service-accounts create --owner",
        "The member who answers for it, by user id (default: you): its keys stop when they leave the organization",
    ),
    (
        "service-accounts create --expires",
        "When it stops working: a day (YYYY-MM-DD, UTC, the whole day) or a duration (30d, 12h or a number of days); at most 365 days from now",
    ),
    (
        "service-accounts keys create --expires",
        "When the key stops working: a day or a duration, as for the account, and not past the account's own (default: 90 days from now)",
    ),
    (
        "service-accounts keys create --label",
        "A name for the key (1 to 80 characters), to tell keys apart",
    ),
    (
        "service-accounts keys rotate --label",
        "A name for the new key (1 to 80 characters); the old key's name by default",
    ),
    (
        "service-accounts keys rotate --expires",
        "When the new key stops working: a day or a duration, as for the account, and not past the account's own (default: as long as the old key lived)",
    ),
    ("jobs submit --input", "The verb's input, a JSON object (default: {})"),
    (
        "jobs submit --team",
        "The team the job's model calls go to, by slug or id (default: yours)",
    ),
    ("routines list --all", "Every member's routines (owners and admins)"),
    ("routines create --at", "A local time, HH:MM, 24-hour"),
    (
        "routines create --days",
        "The days --at runs on, such as mon-fri or mon,wed,fri (default: every day)",
    ),
    (
        "routines create --every",
        "An interval of 10 minutes or more, such as 30m, 2h or 1d; not with --at",
    ),
    (
        "routines create --timezone",
        "An IANA time zone, such as America/Costa_Rica (default: UTC)",
    ),
    (
        "routines create --title",
        "A name for the routine (default: Morning brief)",
    ),
    (
        "routines create --max-runs-per-day",
        "The most runs a day; a slot past it is skipped",
    ),
    (
        "routines create --team",
        "The team its model calls go to, by slug or id (default: yours)",
    ),
    ("members pending --team", "Only people pending in this team"),
    ("team pending --team", "Only people pending in this team"),
    ("policy test --team", "Test as a member of this team"),
    (
        "policy --team",
        "Act through this team: a lead may pause or limit it, or a person or key in it or in a team under it",
    ),
    ("notifications --unread", "Only items not yet read"),
    (
        "notifications --group",
        "Only this group's items: my_sessions, usage_and_billing, requests_and_approvals, automations_and_integrations or account_and_security",
    ),
    (
        "notifications --cursor",
        "The next page: the cursor the list printed last",
    ),
];

/// Placeholders for value-taking flags a table names only in its prose.
const PROSE_VALUES: &[(&str, &str)] = &[("--level", "<L>"), ("--description", "<text>"), ("--search", "<words>")];

fn lookup(table: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(name, _)| *name == key).map(|(_, text)| *text)
}

/// What a flag that names an agent means, by command then by name as in [`FLAG_TEXT`], with the agents it takes.
fn agent_flag_text(key: &str) -> Option<String> {
    match key {
        "--agent" => Some(format!(
            "Test for a coding agent ({})",
            crate::models::policy_agent_ids(" or ")
        )),
        "agents --agent" => Some(format!(
            "With --csv or --jsonl: one agent's sessions ({})",
            Agent::ids(" or ")
        )),
        // Nexus for desktop starts a Code session with any of its session agents (`start_session`).
        "sessions --agent" => Some(format!(
            "{}; default: the project's",
            SessionAgent::ALL.map(SessionAgent::as_str).join(" or ")
        )),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    pub name: String,
    pub value: Option<String>,
    pub description: String,
    /// A placeholder added to a flag named in prose (`{ ...flag, value }`) comes after its description.
    pub value_last: bool,
}

impl Flag {
    fn json(&self) -> Value {
        let mut map = Map::new();
        map.insert("name".into(), json!(self.name));
        if let Some(value) = self.value.as_ref().filter(|_| !self.value_last) {
            map.insert("value".into(), json!(value));
        }
        map.insert("description".into(), json!(self.description));
        if let Some(value) = self.value.as_ref().filter(|_| self.value_last) {
            map.insert("value".into(), json!(value));
        }
        Value::Object(map)
    }
}

/// The flags a usage string names (`[--parent P]`, `--root`, `--allow a,b`), with their placeholders: the regex
/// `--([a-z][a-z-]*)(?:[ =]((?![-[|])[^\s\]]+))?` of `commands.ts`, scanned by hand.
pub fn flags_of(usage: &str, command: &str) -> Vec<Flag> {
    let chars: Vec<char> = usage.chars().collect();
    let mut found: Vec<Flag> = Vec::new();
    let mut i = 0;
    while i + 2 < chars.len() + 1 {
        let starts = chars.get(i) == Some(&'-')
            && chars.get(i + 1) == Some(&'-')
            && chars.get(i + 2).is_some_and(|c| c.is_ascii_lowercase());
        if !starts {
            i += 1;
            continue;
        }
        let mut end = i + 3;
        while chars.get(end).is_some_and(|c| c.is_ascii_lowercase() || *c == '-') {
            end += 1;
        }
        let name: String = chars[i + 2..end].iter().collect();
        let mut value = None;
        let mut next = end;
        if matches!(chars.get(end), Some(' ') | Some('=')) {
            let first = chars.get(end + 1);
            if first.is_some_and(|c| !matches!(c, '-' | '[' | '|') && !c.is_whitespace() && *c != ']') {
                let mut stop = end + 1;
                while chars.get(stop).is_some_and(|c| !c.is_whitespace() && *c != ']') {
                    stop += 1;
                }
                value = Some(chars[end + 1..stop].iter().collect::<String>());
                next = stop;
            }
        }
        i = next;
        let flag = format!("--{name}");
        if found.iter().any(|existing| existing.name == flag) {
            continue;
        }
        let value = value.map(|raw| if raw.starts_with('<') { raw } else { format!("<{raw}>") });
        let description = flag_text(command, &flag);
        found.push(Flag {
            name: flag,
            value,
            description,
            value_last: false,
        });
    }
    found
}

fn flag_text(command: &str, flag: &str) -> String {
    let words: Vec<&str> = command.split(' ').collect();
    for i in (1..=words.len()).rev() {
        let key = format!("{} {flag}", words[..i].join(" "));
        if let Some(text) = lookup(FLAG_TEXT, &key) {
            return text.to_string();
        }
        if let Some(text) = agent_flag_text(&key) {
            return text;
        }
        #[cfg(feature = "plans")]
        if let Some(text) = lookup(crate::plans::PLAN_FLAG_TEXT, &key) {
            return text.to_string();
        }
    }
    lookup(FLAG_TEXT, flag)
        .map(String::from)
        .or_else(|| agent_flag_text(flag))
        .unwrap_or_default()
}

fn dedupe(flags: Vec<Flag>) -> Vec<Flag> {
    let mut out: Vec<Flag> = Vec::new();
    for flag in flags {
        if !out.iter().any(|other| other.name == flag.name) {
            out.push(flag);
        }
    }
    out
}

/// A catalog entry, its keys in the order the TypeScript object literals and spreads give them.
#[derive(Debug, Clone)]
struct Spec {
    name: String,
    fields: Map<String, Value>,
    flags: Vec<Flag>,
}

impl Spec {
    fn usage(&self) -> String {
        self.fields
            .get("usage")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    fn description(&self) -> String {
        self.fields
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    fn set(&mut self, key: &str, value: Value) {
        self.fields.insert(key.into(), value);
    }

    fn json(&self) -> Value {
        let mut map = Map::new();
        for (key, value) in &self.fields {
            if key == "flags" {
                map.insert(
                    "flags".into(),
                    Value::Array(self.flags.iter().map(Flag::json).collect()),
                );
            } else {
                map.insert(key.clone(), value.clone());
            }
        }
        Value::Object(map)
    }
}

/// One command from a table row: `("create <slug> [--parent P]", "Create a team")` under `team`.
fn from_row(parent: &str, (usage, description): (&str, &str)) -> Spec {
    let verb = usage.split(' ').next().unwrap_or("");
    let name = if verb.starts_with('[') || verb.starts_with("--") {
        parent.to_string()
    } else {
        format!("{parent} {verb}")
    };
    let prose: Vec<Flag> = flags_of(description, &name)
        .into_iter()
        .map(|mut flag| {
            flag.value = lookup(PROSE_VALUES, &flag.name).map(String::from);
            flag.value_last = true;
            flag
        })
        .collect();
    let mut flags = flags_of(usage, &name);
    flags.extend(prose);
    let mut fields = Map::new();
    fields.insert("name".into(), json!(name));
    fields.insert("usage".into(), json!(format!("nexus {parent} {usage}").trim()));
    fields.insert("description".into(), json!(description));
    fields.insert("flags".into(), Value::Null);
    fields.insert("examples".into(), json!([]));
    Spec {
        name,
        fields,
        flags: dedupe(flags),
    }
}

/// The deepest command the words name (`keys add anthropic` names `keys add`), or none.
pub fn find_command<'a>(commands: &'a [Value], words: &[String]) -> Option<&'a Value> {
    let parts = |spec: &Value| spec["name"].as_str().unwrap_or_default().split(' ').count();
    let found = commands
        .iter()
        .filter(|spec| {
            let name = spec["name"].as_str().unwrap_or_default();
            name.split(' ')
                .enumerate()
                .all(|(index, part)| words.get(index).map(String::as_str) == Some(part))
        })
        .max_by_key(|spec| parts(spec))?;
    let subcommands = found["subcommands"].as_array().map(Vec::as_slice).unwrap_or_default();
    find_command(subcommands, words).or(Some(found))
}

/// Rows that share a verb become one subcommand: the first row, the others as variants.
fn merge_rows(parent: &str, rows: &[(impl AsRef<str>, impl AsRef<str>)]) -> Vec<Spec> {
    let mut merged: Vec<Spec> = Vec::new();
    for (usage, description) in rows {
        let spec = from_row(parent, (usage.as_ref(), description.as_ref()));
        if let Some(existing) = merged.iter_mut().find(|other| other.name == spec.name) {
            let mut variants = existing
                .fields
                .get("variants")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            variants.push(json!({ "usage": spec.usage(), "description": spec.description() }));
            existing.set("variants", Value::Array(variants));
            let mut flags = existing.flags.clone();
            flags.extend(spec.flags);
            existing.flags = dedupe(flags);
        } else {
            merged.push(spec);
        }
    }
    merged
}

fn exit_codes(codes: &[(i64, &str)]) -> Value {
    Value::Array(
        codes
            .iter()
            .map(|(code, meaning)| json!({ "code": code, "meaning": meaning }))
            .collect(),
    )
}

/// `command({...})`: the literal's keys in order; `flags` from the usage, appended last, when the literal has none.
struct Literal<'a> {
    name: &'a str,
    usage: String,
    description: String,
    /// Some: the literal gives `flags` (in its position after `description`).
    flags: Option<Vec<Flag>>,
    examples: Vec<String>,
    /// Keys after `examples`, in order (`subcommands`, `exitCodes`).
    after: Vec<(&'static str, Value)>,
    /// `members`: `flags: []` written after `examples`.
    flags_after_examples: bool,
}

fn command(literal: Literal) -> Value {
    let mut map = Map::new();
    map.insert("name".into(), json!(literal.name));
    map.insert("usage".into(), json!(literal.usage));
    map.insert("description".into(), json!(literal.description));
    let explicit = literal.flags.is_some();
    let flags = literal.flags.unwrap_or_else(|| flags_of(&literal.usage, literal.name));
    let flags = Value::Array(flags.iter().map(Flag::json).collect());
    if explicit && !literal.flags_after_examples {
        map.insert("flags".into(), flags.clone());
    }
    map.insert("examples".into(), json!(literal.examples));
    if explicit && literal.flags_after_examples {
        map.insert("flags".into(), flags.clone());
    }
    for (key, value) in literal.after {
        map.insert(key.into(), value);
    }
    if !explicit {
        map.insert("flags".into(), flags);
    }
    Value::Object(map)
}

fn subcommands(specs: Vec<Spec>) -> Value {
    Value::Array(specs.iter().map(Spec::json).collect())
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

/// Every built-in command, with its subcommands, flags, examples and any exit codes of its own.
pub fn command_catalog(version: &str) -> Value {
    let lit = |name: &'static str, usage: &str, description: &str, examples: &[&str]| Literal {
        name,
        usage: usage.to_string(),
        description: description.to_string(),
        flags: None,
        examples: strings(examples),
        after: Vec::new(),
        flags_after_examples: false,
    };

    let auth_subs = merge_rows("auth", AUTH_COMMANDS)
        .into_iter()
        .chain(merge_rows("auth tokens", AUTH_TOKEN_COMMANDS))
        .map(|mut spec| {
            if spec.name == "auth status" {
                spec.set(
                    "exitCodes",
                    exit_codes(&[
                        (0, "Signed in, and Gateway accepts the token"),
                        (1, "Not signed in, or Gateway rejects the saved token"),
                    ]),
                );
            }
            spec
        })
        .collect();
    let policy_subs = merge_rows("policy", &policy_commands())
        .into_iter()
        .map(|mut spec| {
            if spec.name == "policy test" {
                spec.set("exitCodes", exit_codes(&[(0, "Allowed"), (1, "Refused")]));
            }
            spec
        })
        .collect();

    let mut commands = vec![
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(auth_subs))],
            ..lit(
                "auth",
                "nexus auth <command> [options]",
                "Sign in to a Nexus Gateway, check who you are, sign out, and manage your API keys",
                &[
                    "nexus auth login --url https://api.nexus.inc",
                    "echo \"$NEXUS_API_KEY\" | nexus auth login --with-token",
                    "nexus auth status --json",
                    "nexus auth tokens create --label \"CI deploys\"",
                ],
            )
        }),
        command(Literal {
            after: vec![(
                "subcommands",
                subcommands(KEY_COMMANDS.iter().map(|row| from_row("keys", *row)).collect()),
            )],
            ..lit(
                "keys",
                "nexus keys <add|list|history|replace|disable|enable|remove|revoke>",
                "Your organization's own provider keys (BYOK): Gateway calls each provider with its key",
                &["nexus keys add anthropic sk-ant-…", "nexus keys list --json"],
            )
        }),
        command(Literal {
            after: vec![("subcommands", subcommands(merge_rows("team", TEAM_COMMANDS)))],
            ..lit(
                "team",
                "nexus team <command> [options]",
                "Teams in your organization, nested; owners and admins manage them",
                &[
                    "nexus team create web --name \"Web\"",
                    "nexus team add web bo@acme.example",
                    "nexus team import teams.yaml --dry-run",
                ],
            )
        }),
        command(lit(
            "analytics",
            "nexus analytics [--by team|person|model|provider|key|product|day] [--days N] [--from D] [--to D] [--team T] [--depth N] [--under T] [--person P] [--model M] [--provider P] [--product P] [--csv | --jsonl] [--cursor C] [--out F]",
            "Usage by team, person, model, provider, key, product or day, with a daily series",
            &[
                "nexus analytics --by team --days 7",
                "nexus analytics --by model --json",
                "nexus analytics --csv --days 30 > usage.csv",
                "nexus analytics --jsonl --team web --out usage.jsonl",
            ],
        )),
        command(lit(
            "budget",
            "nexus budget set org|team/<slug> <amount>/<day|week|month> [--mode hard_stop] [--warn-at 80] [--dry-run]",
            "The organization's or a team's spend cap per day, week or month: owners and admins set it; a member sees what is left for them. --mode and --warn-at are for teams. Person limits: nexus budgets",
            &[
                "nexus budget set org 2000/month",
                "nexus budget set org 50/week --dry-run",
                "nexus budget show org",
                "nexus budget set team/web 500/month",
                "nexus budget set team/web 500/month --dry-run",
                "nexus budget show team/web",
            ],
        )),
        command(lit(
            "models",
            "nexus models [--all] [--team T]",
            "The models you may use (--all: the whole catalog, coming soon too)",
            &["nexus models", "nexus models --all --json"],
        )),
        command(Literal {
            after: vec![(
                "subcommands",
                subcommands(
                    merge_rows("chat sessions", CHAT_COMMANDS)
                        .into_iter()
                        .chain(merge_rows("chat", CHAT_SHARE_COMMANDS))
                        .collect(),
                ),
            )],
            ..lit(
                "chat",
                "nexus chat \"prompt\" [--image a.png,b.png] [--session <id> | --new] [--model m|auto] [--team T] [--no-web] [--json]",
                "A chat session, the same as Nexus UI's home: the answer streams; a prompt continues the session you used last",
                &[
                    "nexus chat \"hello\"",
                    "nexus chat \"Where is billing handled?\" --new",
                    "nexus chat \"Why is this cut off?\" --image shot.png",
                    "nexus chat \"hello\" --model anthropic/claude-haiku-5-5 --json",
                    "nexus chat sessions images chs_… --out shots",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("artifacts", ARTIFACT_COMMANDS)))],
            ..lit(
                "artifacts",
                "nexus artifacts <command> [options]",
                "Chats shared as snapshots (Nexus Artifacts): inside your organization (everyone, chosen teams or chosen people) or outside it (named people by email, or anyone with the link), as its settings allow",
                &[
                    "nexus chat share chs_… --team web --allow-continue",
                    "nexus chat share chs_… --to ada@partner.example",
                    "nexus chat share chs_… --public --expires 30d",
                    "nexus artifacts --shared-with-me",
                    "nexus artifacts list --external --audience public",
                    "nexus artifacts access art_… --grant bo@partner.example",
                    "nexus artifacts export art_… --md --out answer.md",
                    "nexus artifacts images art_… --out shots",
                    "nexus artifacts continue art_…",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("pages", PAGE_COMMANDS)))],
            ..lit(
                "pages",
                "nexus pages <command> [options]",
                "Standalone HTML or Markdown pages, in versions that never change; share one with your organization, teams, people or a public link",
                &[
                    "nexus pages publish report.html",
                    "nexus pages update pag_… report.html --base 2",
                    "nexus pages get pag_… --source --out report.html",
                    "nexus pages restore pag_… 1",
                    "nexus pages share pag_… --team web --pin 2",
                    "nexus pages delete pag_… --yes",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![(
                "subcommands",
                subcommands(merge_rows("design-system", DESIGN_SYSTEM_COMMANDS)),
            )],
            ..lit(
                "design-system",
                "nexus design-system <command> [options]",
                "Nexus Design's design systems: the tokens and notes designs are made with, served with and checked against. The built-in @nexus/brand is the default until you pick another",
                &[
                    "nexus design-system list",
                    "nexus design-system show",
                    "nexus design-system sync ./web --default",
                    "nexus design-system use acme",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("skills", SKILL_COMMANDS)))],
            ..lit(
                "skills",
                "nexus skills <command> [options]",
                "Your own skills: SKILL.md files, kept in versions that never change; import one, export it unchanged, delete it. Promote one to your organization or a team. It is held until an approver who is not you approves it",
                &[
                    "nexus skills",
                    "nexus skills import ./pdf-processing",
                    "nexus skills import ./pdf-processing --update",
                    "nexus skills create release-notes",
                    "nexus skills export pdf-processing --out skills",
                    "nexus skills delete pdf-processing --yes",
                    "nexus skills promote pdf-processing --to org --note \"For the whole team\"",
                    "nexus skills promote pdf-processing 2 --to team/web",
                    "nexus skills promotions --state held",
                    "nexus skills promotions spr_…",
                    "nexus skills approve spr_…",
                    "nexus skills refuse spr_… --reason \"Say when to use it in the description\"",
                    "nexus skills withdraw spr_… --all-versions",
                    "nexus skills library",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("org", ORG_COMMANDS)))],
            ..lit(
                "org",
                "nexus org <command>",
                "Your organizations (Personal first, then your companies; switch without signing in again) and the organization's settings: how long chat messages are kept and how artifacts may be shared outside it; owners and admins change them. `nexus orgs` is the same command",
                &[
                    "nexus org",
                    "nexus org switch personal",
                    "nexus org switch acme --json",
                    "nexus org settings",
                    "nexus org settings set artifacts.external named",
                    "nexus org settings set artifacts.external.domains '{\"allow\":[\"partner.example\"],\"deny\":[]}'",
                    "nexus org settings set chatRetentionDays null",
                    "nexus org settings set agentSubscriptions false",
                    "nexus org settings set webSearch.engine borrowed",
                    "nexus org subscription",
                    "nexus org subscription set --payer ada@acme.example --dry-run",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(policy_subs))],
            ..lit(
                "policy",
                "nexus policy <show|set|test|controls|pause|limit|override|clear|connectors|connector|grants|review-events|agent-writes>",
                "Model access: allow and deny lists per organization, team and agent, and time-boxed operator controls; a deny always wins. Which connectors the organization's members may use",
                &[
                    "nexus policy show team/web",
                    "nexus policy set org --deny \"anthropic/claude-fable-*\"",
                    "nexus policy test --agent claude --model claude-haiku-5-5",
                    "nexus policy pause person/usr_123 --reason \"runaway spend\" --expires 12h",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            flags_after_examples: true,
            after: vec![("subcommands", subcommands(merge_rows("members", MEMBER_COMMANDS)))],
            ..lit(
                "members",
                "nexus members <command> [options]",
                "Everyone in your organization, who has not joined yet, and invitations",
                &[
                    "nexus members",
                    "nexus members invite ada@acme.example --dry-run",
                    "nexus members invite --pending --yes",
                ],
            )
        }),
        command(Literal {
            flags: Some(
                AUDIT_OPTIONS
                    .iter()
                    .map(|(usage, description)| {
                        let mut flag = flags_of(usage, "audit").into_iter().next().unwrap_or(Flag {
                            name: usage.to_string(),
                            value: None,
                            description: String::new(),
                            value_last: false,
                        });
                        flag.description = description.to_string();
                        flag
                    })
                    .collect(),
            ),
            ..lit(
                "audit",
                "nexus audit [options]",
                "Who changed what in your organization, from where, newest first",
                &[
                    "nexus audit --since 24h",
                    "nexus audit --action 'team.*' --json",
                    "nexus audit --csv > audit.csv",
                ],
            )
        }),
        command(lit(
            "search",
            "nexus search \"<query>\" [--type chats|artifacts|people|teams|models] [--limit N] [--json]",
            "Search what your role may read, grouped and ranked: your own chats (titles and messages), artifacts shared with or by you, people, teams, models and model policies",
            &[
                "nexus search \"deploy checklist\"",
                "nexus search ada --type people",
                "nexus search fly --type chats --limit 10 --json",
            ],
        )),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("budgets", BUDGET_COMMANDS)))],
            ..lit(
                "budgets",
                "nexus budgets <command> [options]",
                "Monthly spend limits per person (UTC calendar month): the organization's default and each person's override; a call that would go over is refused",
                &[
                    "nexus budgets",
                    "nexus budgets default 50",
                    "nexus budgets set ada@acme.example 100",
                    "nexus budgets set ada@acme.example default",
                    "nexus budgets show me --json",
                    "nexus budgets managers add bo@acme.example",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("billing", &billing_commands())))],
            ..lit(
                "billing",
                "nexus billing <command> [options]",
                "Your organization's billing (owners): the card on file through Stripe Checkout, the free start, Nexus credits and their auto top-up, paying after the fact up to a monthly usage limit, the receipts, an unpaid month and when model calls stop for it, the monthly statements and Stripe's billing portal",
                &[
                    "nexus billing",
                    "nexus billing card add",
                    "nexus billing credits buy 10",
                    "nexus billing auto-topup set --below 10 --add 50",
                    "nexus billing after-the-fact on --limit 50",
                    "nexus billing receipts",
                    "nexus billing statement --period 2026-09",
                    "nexus billing statements --json",
                    "nexus billing portal",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![(
                "subcommands",
                subcommands(
                    merge_rows("service-accounts", SERVICE_ACCOUNT_COMMANDS)
                        .into_iter()
                        .chain(merge_rows("service-accounts keys", SERVICE_ACCOUNT_KEY_COMMANDS))
                        .collect(),
                ),
            )],
            ..lit(
                "service-accounts",
                "nexus service-accounts <command> [options]",
                "Service accounts (owners and admins): callers for CI and scripts that are no member and hold no seat. Each holds keys (nxsa_…) that you issue, rotate and revoke; a token is shown once",
                &[
                    "nexus service-accounts",
                    "nexus service-accounts create --name ci --team web --expires 90d --dry-run",
                    "nexus service-accounts create --name ci --team web --expires 2027-01-31",
                    "nexus service-accounts keys create sva_… --label github-actions",
                    "nexus service-accounts keys rotate sva_… sak_…",
                    "nexus service-accounts revoke sva_… --dry-run",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![(
                "subcommands",
                subcommands(
                    merge_rows("slack", SLACK_COMMANDS)
                        .into_iter()
                        .chain(merge_rows("slack channel", SLACK_CHANNEL_COMMANDS))
                        .collect(),
                ),
            )],
            ..lit(
                "slack",
                "nexus slack <command> [options]",
                "The Nexus Slack app (owners and admins): bind a Slack workspace to your organization through Slack's install. Only the person who started an install confirms it, and a workspace binds to one organization at a time. A channel binds to a team: in a bound channel, @nexus attributes each call to that team, under its policy and caps; a team lead binds a channel to a team they lead",
                &[
                    "nexus slack connect",
                    "nexus slack connect --confirm chw_… --dry-run",
                    "nexus slack connect --confirm chw_…",
                    "nexus slack status --json",
                    "nexus slack disconnect chw_… --dry-run",
                    "nexus slack channel list chw_…",
                    "nexus slack channel bind chw_… C0123ABCD --team web",
                    "nexus slack channel unbind chw_… C0123ABCD --json",
                ],
            )
        }),
        command(lit(
            "spend",
            "nexus spend [calls] [--by team|person|model|provider|key|product|day] [--from D] [--to D] [--team T] [--person P] [--model M] [--provider P] [--product P] [--status S] [--session S] [--limit N] [--cursor C] [--csv | --jsonl] [--out F]",
            "Budget and spend by product and capability, or by team; spend calls lists each model call, and --csv or --jsonl writes them to a file",
            &[
                "nexus spend",
                "nexus spend --by team --json",
                "nexus spend calls --team web --status failed",
                "nexus spend --csv --from 2026-10-01 > calls.csv",
                "nexus spend --jsonl --out calls.jsonl",
            ],
        )),
        command(lit(
            "modules",
            "nexus modules",
            "Registered products and capabilities (their commands: nexus <product> --help)",
            &["nexus modules --json"],
        )),
        command(lit(
            "connectors",
            "nexus connectors",
            "Connectors Gateway offers, with their read and write operations",
            &["nexus connectors --json"],
        )),
        command(lit(
            "connections",
            "nexus connections",
            "Your accounts at a connector's vendor, and the organization's: who it says you are there, and whether it needs a reconnect",
            &["nexus connections", "nexus connections --json"],
        )),
        command(lit(
            "connect",
            "nexus connect <connector> [--org] [--write] [field=value ...]",
            "Connect an account at a connector's vendor: the fields its form takes (a value of - reads stdin for one field, and @<file> reads a file only you can read, so a secret stays out of your shell history), or with none, the OAuth link to open. --org connects the organization's (owners and admins); --write asks for the write access a write refused with write_consent_required needs",
            &[
                "echo \"$JIRA_TOKEN\" | nexus connect jira site=acme.atlassian.net email=ada@acme.example token=-",
                "nexus connect notion",
                "nexus connect microsoft-todo --write",
                "nexus connect jira --org site=acme.atlassian.net email=ada@acme.example token=-",
                "echo \"$DD_API_KEY\" | nexus connect datadog --org site=datadoghq.com api_key=- application_key=@dd-app.key",
            ],
        )),
        command(lit(
            "disconnect",
            "nexus disconnect <connector> <account> [--org]",
            "Disconnect an account by its id, name or host. --org disconnects the organization's (owners and admins)",
            &[
                "nexus disconnect jira acme.atlassian.net",
                "nexus disconnect notion Acme --org",
            ],
        )),
        // Staff-only (`--features staff`, on by default): the open-source build lists no triage.
        #[cfg(feature = "staff")]
        command(Literal {
            flags: Some(vec![]),
            after: vec![(
                "subcommands",
                subcommands(merge_rows("triage", crate::triage::TRIAGE_COMMANDS)),
            )],
            ..lit(
                "triage",
                &format!(
                    "nexus triage [{}] | nexus triage send <file> --yes",
                    crate::triage::agents("|")
                ),
                "Nexus staff: start an agent that reads this computer's Nexus files, diagnoses a Nexus defect and drafts a report. Send the report you approved to the Nexus team, with your name and email",
                &[
                    "nexus triage",
                    "nexus triage codex",
                    "nexus triage send report.md --yes",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![(
                "subcommands",
                subcommands(merge_rows("machines", crate::machines::MACHINE_COMMANDS)),
            )],
            ..lit(
                "machines",
                "nexus machines [list|show|sleep] [<id>]",
                "Your cloud machines in this organization, one per cloud session: their state, size and why one sleeps. Put one to sleep; the app wakes it when you use the session",
                &[
                    "nexus machines",
                    "nexus machines show cm_…",
                    "nexus machines sleep cm_…",
                    "nexus machines --json",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![(
                "subcommands",
                subcommands(merge_rows("notifications", crate::notifications::NOTIFICATION_COMMANDS)),
            )],
            ..lit(
                "notifications",
                "nexus notifications [list|read|done|kinds] [<id>]",
                "Your inbox in this organization, as Nexus UI's notifications page shows it: the items not yet done, newest first. Mark an item read, or done to clear it",
                &[
                    "nexus notifications",
                    "nexus notifications list --unread --group usage_and_billing",
                    "nexus notifications done ntf_…",
                    "nexus notifications kinds --json",
                ],
            )
        }),
        command(Literal {
            flags: Some(flags_of("--team T --no-wait", "<product>")),
            ..lit(
                "<product>",
                "nexus <product> <verb> [capability] [--<input> value] [--team T] [--no-wait]",
                "A product's commands, discovered from the modules your Gateway registers (nexus modules): run, report and the rest",
                &[
                    "nexus robustness run --repo https://github.com/nexus-demo/hello-factory",
                    "nexus robustness status review --repo https://github.com/nexus-demo/hello-factory",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("jobs", JOB_COMMANDS)))],
            ..lit(
                "jobs",
                "nexus jobs <command> [options]",
                "Jobs: a module's verb with its input, run in the background for the person who submits it. A job is accepted, then running, then done, failed or cancelled. You, and your organization's owners and admins, read and cancel it",
                &[
                    "nexus jobs submit robustness/review run --input '{\"repo\":\"https://github.com/nexus-demo/hello-factory\"}'",
                    "nexus jobs get job_…",
                    "nexus jobs cancel job_…",
                    "nexus jobs result job_… --json",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("routines", ROUTINE_COMMANDS)))],
            ..lit(
                "routines",
                "nexus routines <command> [options]",
                "Routines: work Nexus does for you without being asked, such as a morning brief on a schedule. A routine runs as you only, reads and notifies only, and reports to your Slack direct messages. You manage your own; your organization's owners and admins may read, pause and delete them",
                &[
                    "nexus routines create --at 08:00 --days mon-fri --timezone America/Costa_Rica",
                    "nexus routines list",
                    "nexus routines pause rtn_…",
                    "nexus routines resume rtn_…",
                ],
            )
        }),
        command(Literal {
            flags: Some(vec![]),
            after: vec![("subcommands", subcommands(merge_rows("apps", APP_COMMANDS)))],
            ..lit(
                "apps",
                "nexus apps <command> [options]",
                "Deploy and manage Apps on Nexus. Hosting costs Vercel list price plus 8%. Each app starts with a $10 monthly cap.",
                &[
                    "nexus apps deploy ./my-site",
                    "nexus apps deploy --name docs --framework vite",
                    "nexus apps list",
                    "nexus apps claim app_…",
                    "nexus apps delete app_… --yes",
                ],
            )
        }),
    ];
    for entry in Agent::ALL
        .into_iter()
        .filter(|&agent| crate::agents::launch_available(agent))
        .map(Agent::entry)
    {
        // Named by its command (`agy`), not its id.
        let (agent, label, plan) = (entry.identity.executable, entry.identity.name, entry.identity.plan);
        let example = format!("nexus {agent} {}", entry.session.example);
        // An agent without a Gateway route has only its plan: no Gateway flags, no recording, no `auto` model.
        if entry.billing.gateway.is_none() {
            commands.push(command(Literal {
                name: agent,
                usage: format!("nexus {agent} [--json] [--subscription] [--team <slug>] [--] [{agent} args…]"),
                description: format!(
                    "{label} on your own {plan} plan: its calls go straight to {}, and Nexus records the session",
                    entry.identity.vendor
                ),
                flags: Some(flags_of("--json --subscription --team <slug> --model <id>", agent)),
                examples: vec![
                    example,
                    format!("nexus {agent} --team web"),
                    format!("nexus {agent} -- {}", entry.session.resume.replace("{id}", "<id>")),
                ],
                after: vec![(
                    "exitCodes",
                    exit_codes(&[
                        (0, "The agent's own exit code is passed through"),
                        (2, "--api, --via gateway, --model auto or --recording: not available, and the agent does not start"),
                    ]),
                )],
                flags_after_examples: false,
            }));
            continue;
        }
        commands.push(command(Literal {
            name: agent,
            // The TypeScript CLI's routes; this CLI lists them so its catalog stays commands.json, but runs every agent
            // through Gateway (it is not distributed). Its `nexus agents` is the same as the
            // TypeScript one.
            usage: format!(
                "nexus {agent} [--json] [--subscription | --api | --via <route>] [--team <slug>] [--recording [<id>|latest]] [--no-skills] [--] [{agent} args…]"
            ),
            description: format!(
                "{label} on the route you chose (nexus agents): through Nexus Gateway, metered, checked against your policies and attributed to you, or on your own {plan} plan"
            ),
            flags: Some(flags_of(
                "--json --subscription --api --via <route> --team <slug> --recording <id> --model auto",
                agent,
            )),
            examples: vec![
                example,
                format!("nexus {agent} --team web"),
                format!("nexus {agent} --subscription"),
                format!("nexus {agent} --recording latest"),
            ],
            after: vec![(
                "exitCodes",
                exit_codes(&[(0, "The agent's own exit code is passed through")]),
            )],
            flags_after_examples: false,
        }));
    }
    commands.push(command(Literal {
        name: "agents",
        usage: "nexus agents <command>".into(),
        description: "Your coding agents: whether each is installed and signed in on this machine, the route each takes (through Gateway, or on your own plan), who pays on it, and what is left to set up, with the command for each step. Works signed out (this machine only). Signed in, --csv or --jsonl writes the organization's agent sessions to a file".into(),
        flags: Some(flags_of(
            "--csv --jsonl --from D --to D --team T --person P --agent A --via V --status S --cursor C --out F",
            "agents",
        )),
        examples: [
            "nexus agents",
            "nexus agents use claude subscription",
            "nexus agents use codex default",
            "nexus agents setup claude",
            "nexus agents share latest --org",
            "nexus agents --json",
            "nexus agents --jsonl --from 2026-10-01 --out sessions.jsonl",
        ]
        .map(String::from)
        .to_vec(),
        after: vec![(
            "subcommands",
            subcommands(
                agent_commands()
                    .iter()
                    .map(|(usage, description)| from_row("agents", (usage, description)))
                    .chain(merge_rows("agents", AGENT_SHARE_COMMANDS))
                    .collect(),
            ),
        )],
        flags_after_examples: false,
    }));
    commands.push(command(Literal {
        flags: Some(vec![]),
        after: vec![(
            "subcommands",
            subcommands(merge_rows("agent", crate::agent_chat::AGENT_CHAT_COMMANDS)),
        )],
        ..lit(
            "agent",
            "nexus agent <ask|chat> [options]",
            "Nexus, the same one as @nexus in Slack, with its memory and saved model. It never runs a write: it prints the Nexus UI page where you approve one. Needs NEXUS_AGENTS_API_URL, the agent API's address. Any other nexus agent command is nexus agents",
            &[
                "nexus agent ask \"What did I decide about the launch?\"",
                "nexus agent ask --conversation <id> \"And the date?\"",
                "nexus agent chat",
                "nexus agent ask \"Summarize my week\" --json",
            ],
        )
    }));
    commands.push(command(Literal {
        after: vec![(
            "subcommands",
            subcommands(merge_rows("recordings", crate::recordings::RECORDINGS_COMMANDS)),
        )],
        ..lit(
            "recordings",
            "nexus recordings [list|show|path|frames] [<id>|latest] [--json]",
            "Screen recordings Nexus for Mac makes for agents (⌥⇧S), on this computer: what an agent reads, and more frames for a closer look",
            &[
                "nexus recordings",
                "nexus recordings show latest",
                "nexus recordings frames latest --from 0:10 --to 0:14",
                "nexus claude --recording latest",
            ],
        )
    }));
    commands.push(command(Literal {
        after: vec![(
            "subcommands",
            subcommands(merge_rows("memory", crate::shared_memory::COMMANDS)),
        )],
        ..lit(
            "memory",
            "nexus memory [status|enable|disable|list|add|edit|forget|import|export]",
            "Manage the local shared memory that coding agents use across sessions and worktrees",
            &[
                "nexus memory enable",
                "nexus memory import claude",
                "nexus memory list --json",
            ],
        )
    }));
    commands.push(command(Literal {
        after: vec![(
            "subcommands",
            subcommands(merge_rows("sessions", crate::sessions::SESSIONS_COMMANDS)),
        )],
        ..lit(
            "sessions",
            "nexus sessions [list|show|start|send|stop|name|connect|disconnect|mcp] [--json]",
            "Code sessions in Nexus for desktop, on this computer: list, read, start, name, message and stop them from a script or another agent. Turn on Outside access in the app first; each client asks you once",
            &[
                "nexus sessions list",
                "nexus sessions start --project . \"Find why the tests are slow\"",
                "nexus sessions send <id> \"Now fix it\"",
                "nexus sessions show <id>",
                "nexus sessions mcp --name \"Claude Code\"",
            ],
        )
    }));
    commands.push(command(Literal {
        after: vec![(
            "subcommands",
            subcommands(merge_rows("devices", crate::devices::DEVICES_COMMANDS)),
        )],
        ..lit(
            "devices",
            "nexus devices [list|pair|revoke|org-key] [--json]",
            "Your devices for end-to-end encrypted session sync: pair this computer so its nexus claude and nexus codex sessions show in Nexus UI, and revoke a device",
            &["nexus devices pair", "nexus devices", "nexus devices revoke dev_…"],
        )
    }));
    commands.push(command(lit(
        "mcp",
        "nexus mcp",
        "A local MCP server on stdin and stdout: your spend, budgets, usage, models, teams and agent sessions as tools, under your own sign-in and permissions. Owners and admins also read and change members, teams, budgets, policies and keys; the agent asks before each change. An agent starts it: claude mcp add nexus -- nexus mcp, or codex mcp add nexus -- nexus mcp",
        &["nexus mcp"],
    )));
    commands.push(command(Literal {
        after: vec![(
            "exitCodes",
            exit_codes(&[
                (0, "Updated, already up to date, rolled back, or (--check) checked"),
                (
                    1,
                    "Nothing changed: Gateway unreachable, a checksum mismatch, or this CLI was not installed by Nexus's installer",
                ),
            ]),
        )],
        ..lit(
            "update",
            "nexus update [--check] [--rollback] [--json]",
            "Update this CLI to the version your Gateway publishes (GET /cli/version)",
            &["nexus update", "nexus update --check --json", "nexus update --rollback"],
        )
    }));
    commands.push(command(lit(
        "completion",
        "nexus completion <bash|zsh|fish|install>",
        "Tab-completion for commands, subcommands and flags: install turns it on, a shell prints its script to source",
        &[
            "nexus completion install",
            "nexus completion bash",
            "nexus completion zsh",
            "nexus completion fish",
        ],
    )));
    commands.push(command(Literal {
        flags: Some(vec![]),
        ..lit(
            "version",
            "nexus --version [--json]",
            "The CLI's version: semver and build, e.g. 0.2.0+abc1234",
            &["nexus --version", "nexus --version --json"],
        )
    }));
    commands.push(command(Literal {
        flags: Some(vec![]),
        ..lit(
            "help",
            "nexus help [--json]",
            "Every command; --json: this list, for scripts and the docs",
            &["nexus help --json"],
        )
    }));

    json!({
        "schema": COMMANDS_SCHEMA,
        "version": version,
        "usage": "nexus <command> [options]",
        "globalFlags": GLOBAL_FLAGS.iter().map(|(name, description)| json!({ "name": name, "description": description })).collect::<Vec<_>>(),
        "exitCodes": exit_codes(EXIT_CODES),
        "environment": ENVIRONMENT.iter().map(|(name, description)| json!({ "name": name, "description": description })).collect::<Vec<_>>(),
        "commands": commands,
    })
}

/// `[{ command: "nexus <parent> <verb>", description }]`, the per-group `help --json` of the TypeScript CLI.
pub fn command_rows(parent: &str, rows: &[(impl AsRef<str>, impl AsRef<str>)]) -> Value {
    Value::Array(
        rows.iter()
            .map(|(name, text)| {
                json!({ "command": format!("nexus {parent} {}", name.as_ref().split(' ').next().unwrap_or("")), "description": text.as_ref() })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The help names each agent's route variable, so a new agent's is not left out.
    #[test]
    fn environment_names_every_agents_route_variable() {
        for agent in Agent::ALL
            .into_iter()
            .filter(|&agent| crate::agents::launch_available(agent))
        {
            let variable = nexus_client::agents::route_variable(agent);
            assert!(
                ENVIRONMENT.iter().any(|(name, _)| *name == variable),
                "{variable} is not in ENVIRONMENT"
            );
        }
    }

    /// `nexus help` has one row per launcher, from the catalog.
    #[test]
    fn help_lists_each_launcher() {
        let groups = help_groups();
        let rows: Vec<&(String, String)> = groups.iter().flat_map(|group| &group.rows).collect();
        for (usage, text) in [
            (
                "claude [args]",
                "Claude Code through Nexus Gateway, or on your Claude plan (--subscription)",
            ),
            (
                "codex [args]",
                "Codex through Nexus Gateway, or on your ChatGPT plan (--subscription)",
            ),
            ("agy [args]", "Antigravity on your Google AI plan"),
            ("cursor-agent [args]", "Cursor on your Cursor plan"),
        ] {
            assert!(rows.iter().any(|row| row.0 == usage && row.1 == text), "{usage}");
        }
    }

    #[test]
    fn cursor_launcher_is_subscription_only() {
        assert!(crate::agents::launch_available(Agent::Cursor));
        let catalog = command_catalog("0.0.0");
        let commands = catalog["commands"].as_array().expect("commands");
        let cursor = commands
            .iter()
            .find(|entry| entry["name"] == "cursor-agent")
            .expect("Cursor launcher");
        let flags = cursor["flags"].as_array().expect("flags");
        assert!(flags.iter().any(|entry| entry["name"] == "--subscription"));
        assert!(flags.iter().all(|entry| entry["name"] != "--api"));
    }

    #[test]
    fn reads_flags_from_usage_strings() {
        let flags = flags_of(
            "create <slug> [--parent P] [--level L] --root --allow a,b [--x|--y]",
            "team create",
        );
        let names: Vec<(&str, Option<&str>)> = flags.iter().map(|f| (f.name.as_str(), f.value.as_deref())).collect();
        assert_eq!(
            names,
            vec![
                ("--parent", Some("<P>")),
                ("--level", Some("<L>")),
                ("--root", None),
                ("--allow", Some("<a,b>")),
                ("--x", None),
                ("--y", None),
            ]
        );
        assert_eq!(
            flags[1].description,
            "Your own label for the team's level (e.g. department)"
        );
    }
}
