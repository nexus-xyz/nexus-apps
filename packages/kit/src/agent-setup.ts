/**
 * Coding agents' setup, shared by every TypeScript client (the `nexus` CLI, Nexus UI, Nexus for desktop): what each
 * agent needs to run on a route, as steps a person follows, each with the command that does it.
 *
 * Gateway's `GET /v1/agents/access` is the source of truth for the agents, their routes and who pays on each.
 * {@link AGENT_CATALOG} is its fallback when there is no answer (signed out, or a Gateway without the endpoint): the
 * same entries as the agents in the Rust catalog (`nexus_catalog::Agent`). A new agent or plan is added there first.
 * `agent-capabilities.test.ts` holds each entry to `kit/fixtures/agent-capabilities.json`, which Rust writes.
 */
import type { AgentAccessEntry, AgentAccessResponse, AgentName, AgentRoute, AgentRouteView, AgentSessionSummary } from "./api";

/** How an agent is signed in on this machine, as its own status command says. */
export type LocalLogin = "plan" | "api_key" | "signed_out" | "unknown";

/** An agent on this machine: found or not, its version, and how it is signed in (when a client can look). */
export interface LocalAgentStatus {
  agent: AgentName;
  installed: boolean;
  /** Where it was found. */
  path?: string;
  version?: string;
  /** Unset when not installed, or when the client did not check. */
  login?: LocalLogin;
  /** The plan it is signed in to, as the agent names it (`max`, `team`), when it says. */
  plan?: string;
}

/**
 * One of the agents' own conversations on this machine (Claude Code's transcripts, Codex's rollouts), as Nexus for
 * desktop reads it (`nexus_client::local_sessions`): never sent to Gateway.
 */
export interface LocalSession {
  agent: AgentName;
  /** The agent's conversation id, what `--resume` / `resume` takes. */
  id: string;
  title?: string;
  cwd?: string;
  /** The folder's last part. */
  project?: string;
  gitBranch?: string;
  model?: string;
  /** Where Claude Code ran it (`cli`, `claude-desktop`, `sdk-ts`), or what started Codex. */
  entrypoint?: string;
  startedAt?: string;
  updatedAt: string;
  prUrl?: string;
  /** The shell command that resumes it through Nexus, in its folder. */
  resume: string;
}

/** One thing to do before an agent runs on a route. `done` is unset when the client cannot tell (a browser). */
export interface SetupStep {
  id: "install_cli" | "install" | "nexus_login" | "plan_login" | "plan_relogin" | "provider_key" | "route_unsupported" | "allow_subscriptions";
  /** Who does it: the person, on their machine, or an owner or admin of the organization. */
  who: "you" | "admin";
  text: string;
  command?: string;
  done?: boolean;
}

/**
 * An agent as the clients know it without Gateway: its entry in Gateway's answer, and the text about it that the
 * answer does not carry. Each agent states its own, so no client code tells the agents apart by name.
 */
export interface AgentCatalogEntry extends AgentAccessEntry {
  /** The program it runs (`claude`), which is also its command: `nexus <executable>`. It may differ from `agent` (`agy`). */
  executable: string;
  /** Whether the Nexus CLI exposes this launcher. */
  cliLauncher: boolean;
  configuredProvider?: string;
  /** How to sign it out, once it is signed in with an API key instead of its plan. */
  logout: string;
  /** What resumes one of its conversations: after `nexus <executable>` and its flags, before the conversation's id. */
  resume: string;
}

/** An agent's own facts; {@link agentCatalogEntry} lays its routes out as Gateway does. */
interface AgentDefinition extends Omit<AgentCatalogEntry, "route" | "from" | "preference" | "routes" | "cliLauncher"> {
  cliLauncher?: boolean;
  /**
   * How it runs through Gateway: the provider Gateway calls. Absent for an agent that runs only on the person's own
   * plan; its `gateway` route is then unavailable, as in the Rust client's offline catalog (`nexus-client`'s
   * `agent_setup::catalog_entry`).
   */
  gateway?: { provider: string };
  /** The plan its subscription route runs on (`Claude`), and who sells it (`Anthropic`). */
  plan: string;
  vendor: string;
  login: string;
  loginStatus: string;
}

export function agentCatalogEntry({ gateway, plan, vendor, login, loginStatus, cliLauncher = true, ...agent }: AgentDefinition): AgentCatalogEntry {
  const gatewayRoute: AgentRouteView = gateway
    ? { route: "gateway", access: null, available: true, provider: gateway.provider }
    : { route: "gateway", access: null, available: false, reason: agent.configuredProvider ? `${agent.name} uses its configured provider.` : `${agent.name} runs only on your ${plan} plan through Nexus for now` };
  return {
    ...agent,
    cliLauncher,
    route: "subscription",
    from: "default",
    routes: [gatewayRoute, { route: "subscription", access: "subscription", available: true, plan, vendor, login, loginStatus }],
  };
}

/** Each agent and its routes when Gateway has not answered: who pays on Gateway is unknown (`access: null`). */
export const AGENT_CATALOG: AgentCatalogEntry[] = [
  agentCatalogEntry({
    agent: "claude",
    executable: "claude",
    name: "Claude Code",
    install: "curl -fsSL https://claude.ai/install.sh | bash",
    gateway: { provider: "anthropic" },
    plan: "Claude",
    vendor: "Anthropic",
    login: "claude auth login",
    loginStatus: "claude auth status --text",
    logout: "claude auth logout",
    resume: "--resume",
  }),
  agentCatalogEntry({
    agent: "codex",
    executable: "codex",
    name: "Codex",
    install: "npm install -g @openai/codex",
    gateway: { provider: "openai" },
    plan: "ChatGPT",
    vendor: "OpenAI",
    login: "codex login",
    loginStatus: "codex login status",
    logout: "codex logout",
    resume: "resume",
  }),
  // No `gateway`: it runs only on the person's Google AI plan. `agy` has no login or status command; it signs in on its
  // first start. `modelProvider` in its settings file selects an API key, which signing out does not undo.
  agentCatalogEntry({
    agent: "antigravity",
    executable: "agy",
    name: "Antigravity",
    install: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
    plan: "Google AI",
    vendor: "Google",
    login: "agy",
    loginStatus: "agy",
    logout: 'remove "modelProvider" from ~/.gemini/antigravity-cli/settings.json',
    resume: "--conversation",
  }),
  // No `gateway`: it runs only on the person's Cursor plan. `cursor-agent status` reads its sign-in without a model turn.
  agentCatalogEntry({
    agent: "cursor",
    executable: "cursor-agent",
    cliLauncher: true,
    name: "Cursor",
    install: "curl https://cursor.com/install -fsS | bash",
    plan: "Cursor",
    vendor: "Cursor",
    login: "cursor-agent login",
    loginStatus: "cursor-agent status",
    logout: "cursor-agent logout",
    resume: "--resume",
  }),
  agentCatalogEntry({
    agent: "opencode",
    executable: "opencode",
    cliLauncher: false,
    configuredProvider: "OpenCode provider",
    name: "OpenCode",
    install: "curl -fsSL https://opencode.ai/install | bash",
    plan: "configured provider",
    vendor: "OpenCode",
    login: "opencode auth login",
    loginStatus: "opencode auth list",
    logout: "opencode auth logout",
    resume: "--session",
  }),
  agentCatalogEntry({
    agent: "grok",
    executable: "grok",
    cliLauncher: false,
    name: "Grok",
    install: "curl -fsSL https://x.ai/cli/install.sh | bash",
    plan: "Grok",
    vendor: "xAI",
    login: "grok login",
    loginStatus: "grok",
    logout: "grok logout",
    resume: "--resume",
  }),
];

/** An agent this kit knows. A newer Gateway can name one it does not. */
function knownAgent(agent: string): AgentCatalogEntry | undefined {
  return AGENT_CATALOG.find((entry) => entry.agent === agent);
}

/** Each agent's launcher (`nexus claude`), in the catalog's order. */
export const AGENT_LAUNCHERS: readonly string[] = AGENT_CATALOG
  .filter((entry) => entry.cliLauncher)
  .map((entry) => `nexus ${entry.executable}`);

/** An agent's name (`Claude Code`); none for an agent this kit does not know. */
export function knownAgentName(agent: string): string | undefined {
  return knownAgent(agent)?.name;
}

/** An agent's name in text (`Claude Code`); its id for an agent this kit does not know. */
export function agentLabel(agent: string): string {
  return knownAgentName(agent) ?? agent;
}

/** The payer label for an agent that reads its own provider configuration. */
export function agentConfiguredProvider(agent: string): string | undefined {
  return knownAgent(agent)?.configuredProvider;
}

/** The plan an agent runs on outside Gateway (`Claude`, `Google AI`); none for an agent this kit does not know. */
export function agentPlan(agent: string): string | undefined {
  const known = knownAgent(agent);
  return known && routeOf(known, "subscription")?.plan;
}

/** A catalog entry as Gateway would send it, without the clients' own text. */
function accessEntry({ agent, name, install, route, from, routes }: AgentCatalogEntry): AgentAccessEntry {
  return structuredClone({ agent, name, install, route, from, routes });
}

/** The fallback answer lists released launchers, as Gateway does. */
export const offlineAgentAccess = (): AgentAccessResponse => ({ agentSubscriptions: true, agents: AGENT_CATALOG.filter((entry) => entry.cliLauncher).map(accessEntry) });

export function agentEntry(access: AgentAccessResponse | undefined, agent: AgentName): AgentAccessEntry {
  const known = knownAgent(agent);
  return access?.agents.find((entry) => entry.agent === agent) ?? (known && accessEntry(known))!;
}

export function routeOf(entry: AgentAccessEntry, route: AgentRoute): AgentRouteView | undefined {
  return entry.routes.find((view) => view.route === route);
}

/**
 * Each Gateway provider's name in messages, by its id: the vendor of the agent Gateway calls it for. An agent without a
 * Gateway route names no provider.
 */
const PROVIDER_LABELS: Record<string, string> = Object.fromEntries(
  AGENT_CATALOG.flatMap((entry) => {
    const provider = routeOf(entry, "gateway")?.provider;
    const vendor = routeOf(entry, "subscription")?.vendor;
    return provider && vendor ? [[provider, vendor]] : [];
  }),
);

/** Who paid a session on an agent's plan, in a word or two (`Claude plan`); none for an agent this kit does not know. */
function planLabel(agent: AgentName): string | undefined {
  const plan = agentPlan(agent);
  return plan ? `${plan} plan` : undefined;
}

/** Who pays on a route, in a few words: "your organization's Anthropic key (BYOK)", "your Claude plan". */
export function payerLabel(view: AgentRouteView | undefined): string {
  if (!view) return "unknown";
  if (view.route === "subscription") return `your ${view.plan ?? "own"} plan`;
  const provider = PROVIDER_LABELS[view.provider ?? ""] ?? view.provider ?? "the provider";
  if (view.access === "byok") return `your organization's ${provider} key (BYOK)`;
  if (view.access === "nexus") return "Nexus credit";
  return view.available ? `your organization's ${provider} key or Nexus credit` : `no ${provider} key yet`;
}

/** A route in a few words: "Gateway", "your Claude plan". */
export function routeLabel(entry: AgentAccessEntry, route: AgentRoute): string {
  return route === "gateway" ? "Gateway" : agentConfiguredProvider(entry.agent) ?? `your ${routeOf(entry, "subscription")?.plan ?? "own"} plan`;
}

/**
 * What a person still has to do before `entry`'s agent runs on `route`, in order. `local` is what the client found
 * on this machine (unset: it cannot look, so the steps it would need say so with `done` unset); `signedIn`: whether
 * the `nexus` CLI is signed in (unset: unknown); `cliInstall`: the command that installs the CLI, for a client that is
 * not the CLI (Nexus UI, desktop), which then lists installing it first.
 */
export function setupSteps(entry: AgentAccessEntry, route: AgentRoute, options: { local?: LocalAgentStatus; signedIn?: boolean; agentSubscriptions?: boolean; cliInstall?: string } = {}): SetupStep[] {
  const { local, signedIn } = options;
  const steps: SetupStep[] = [];
  if (options.cliInstall) steps.push({ id: "install_cli", who: "you", text: "Install the nexus CLI", command: options.cliInstall });
  steps.push({ id: "install", who: "you", text: `Install ${entry.name}`, command: entry.install, done: local?.installed });
  steps.push({ id: "nexus_login", who: "you", text: "Sign in to Nexus", command: "nexus auth login", done: signedIn });
  const view = routeOf(entry, route);
  if (route === "gateway") {
    if (view && !view.available && view.provider) {
      const provider = view.provider;
      steps.push({ id: "provider_key", who: "admin", text: `Add your organization's ${PROVIDER_LABELS[provider] ?? provider} key (owners and admins)`, command: `nexus keys add ${provider} <key>`, done: false });
    } else if (view && !view.available) {
      // No provider: the agent has no Gateway route (`agentCatalogEntry`, Gateway's `gateway_route`), so no key makes it
      // run. The step says so, and keeps the route from reading as ready.
      steps.push({ id: "route_unsupported", who: "you", text: view.reason ?? `${entry.name} has no Gateway route`, done: false });
    }
    return steps;
  }
  if (options.agentSubscriptions === false || (view && !view.available)) {
    steps.push({ id: "allow_subscriptions", who: "admin", text: "Allow subscription mode (owners and admins)", command: "nexus org settings set agentSubscriptions true", done: false });
  }
  const plan = view?.plan ?? "own";
  if (local?.login === "api_key") {
    const logout = knownAgent(entry.agent)?.logout;
    steps.push({ id: "plan_relogin", who: "you", text: `${entry.name} is signed in with an API key, which bills the API, not your plan: sign out${logout ? ` (${logout})` : ""}, then sign in with your ${plan} account`, command: view?.login, done: false });
  } else {
    const done = local?.login === "plan" ? true : local?.login === "signed_out" ? false : undefined;
    steps.push({ id: "plan_login", who: "you", text: `Sign ${entry.name} in to your ${plan} plan (once, on this machine)`, command: view?.login, done });
  }
  return steps;
}

/** Whether every step is done; `undefined` when some cannot be checked from here. */
export function setupReady(steps: SetupStep[]): boolean | undefined {
  if (steps.some((step) => step.done === false)) return false;
  return steps.every((step) => step.done === true) ? true : undefined;
}

const SLUG = /^[A-Za-z0-9._-]+$/;

/**
 * The command that resumes an agent session's conversation, once it has ended and the CLI reported the agent's id: on
 * the same route and team, so its spend goes where the first run's did (named, since a saved route could differ), run
 * on the machine and in the folder it ran in. Team slugs are lower-case letters, digits and dashes.
 */
export function resumeCommand(session: Pick<AgentSessionSummary, "agent" | "agentSessionId" | "status" | "via" | "team">): string | undefined {
  if (!session.agentSessionId || session.status === "running") return undefined;
  // It is pasted into a shell: an id or team that is not a plain slug gets no command rather than a quoted one.
  if (!SLUG.test(session.agentSessionId) || (session.team && !SLUG.test(session.team))) return undefined;
  // An agent this kit does not know gets no command rather than another agent's syntax.
  const known = knownAgent(session.agent);
  if (!known?.cliLauncher) return undefined;
  const route = session.via === "subscription" ? " --subscription" : session.via === "nexus" ? " --gateway" : "";
  const flags = `${route}${session.team ? ` --team ${session.team}` : ""}`;
  return `nexus ${known.executable}${flags} ${known.resume} ${session.agentSessionId}`;
}

/** Who paid an agent session's calls, in a word, and the longer form. No word for the plan of an agent this kit does not know. */
export function sessionPayer(session: Pick<AgentSessionSummary, "agent" | "via" | "access">): { label: string | undefined; title: string } {
  if (session.via === "subscription") {
    const configured = agentConfiguredProvider(session.agent);
    if (configured) return { label: configured, title: "Model calls go directly to the agent's configured provider" };
    return { label: planLabel(session.agent), title: "The person's own plan: calls straight to the vendor, not metered by Gateway" };
  }
  if (session.access === "byok") return { label: "BYOK", title: "Through Gateway, paid with your organization's own provider key" };
  if (session.access === "nexus") return { label: "Nexus credit", title: "Through Gateway, paid with a key Nexus pays for" };
  if (session.via === "nexus") return { label: "Gateway", title: "Through Gateway; who paid was not recorded (a session from before, or no key)" };
  return { label: "–", title: "Not recorded" };
}
