/**
 * The Code workbench's contract between the webview and the app's Rust core (the
 * `nexus-workbench` crate, whose `event` module is this file's twin). The core runs the agents (Claude Code and
 * Codex) and keeps every session's event log; the webview only renders it and sends intents.
 *
 * Delivery: `wb_thread_page` returns a session's latest events (older pages on request), `wb_thread_events` its stored
 * events after a sequence number, then `wb:event` pushes each
 * new one live. Every `wb:event` is stored and numbered per session (`seq`, from 1, no gaps), so a view that missed
 * some asks again after the last it has. Streamed text is not an event of the log: it goes on its own channel,
 * `wb:delta`, carries no `seq`, and is replaced by the final `text` event with the same id.
 *
 * Organization: the core serves and emits only the sessions of the organization the app is signed in to (the
 * Gateway and organization a session bills to are fixed when it is created). A window that sees the organization
 * change reads `wb_overview` again. Projects are local folders and are shared by every organization.
 *
 * Policy lives in the core, not here: who pays (`via`) is checked again in Rust before every agent start, so a webview
 * that skips that check gets an error, not the thing it asked for.
 */

export type Agent = "claude" | "codex" | "nexus";

/** The agents as people name them. */
export const AGENT_NAMES: Record<Agent, string> = { claude: "Claude Code", codex: "Codex", nexus: "Nexus" };

/** Claude Code's `--permission-mode`, under the names people see: Ask first, Auto-accept edits, Plan only, Full access. */
export type PermissionMode = "default" | "auto" | "acceptEdits" | "plan" | "bypassPermissions";

/** A project's new-session defaults as the store keeps them (nexus-workbench `project_defaults.rs`). */
export interface StoredProjectDefaults {
  agent?: Agent;
  model?: string | null;
  mode?: PermissionMode;
  workspace?: "folder" | "worktree";
}
/** What a project's own Claude settings would turn on (`.claude/settings*.json`, `.mcp.json`, and the skills, subagents and commands under `.claude`), or its OpenCode config (`opencode.json(c)`, `.opencode/`). */
export interface ClaudeSettingsFound {
  hooks: number;
  allowed: number;
  mcpServers: number;
  other: boolean;
  /** Skills, subagents and slash commands (`.claude/skills`, `.claude/agents`, `.claude/commands`). */
  extras: number;
  /** OpenCode plugins and custom tools. Absent from an older runner. */
  plugins?: number;
}
/** A project's answer about its own Claude settings and OpenCode config. `trusted` null: never asked. */
export interface ClaudeTrust {
  trusted: boolean | null;
  found: ClaudeSettingsFound;
  /** What the project's OpenCode config would turn on. Absent from an older runner. */
  opencode?: ClaudeSettingsFound;
}
/** The agent's reasoning effort on each turn. */
export type Effort = "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";

/**
 * How model calls are paid: through Nexus Gateway (the default), or the person's own Claude subscription. The core
 * starts a `subscription` agent only while the organization's `agentSubscriptions` setting is on: it reads the
 * setting before every start, and Gateway refuses to open the session otherwise.
 */
export type Via = "nexus" | "subscription";

export type ThreadStatus =
  /** No agent process; the next message starts one (resuming the conversation). */
  | "idle"
  /** The agent process is starting. */
  | "starting"
  /** A turn is in progress. */
  | "running"
  /** An action or a question needs the person. Independent work may still run. */
  | "approval"
  /** The process ended in the middle of a turn (Stop, quit, crash): the Continue button shows. */
  | "interrupted"
  /** The agent could not start or failed; the last `error` event says why. */
  | "error";

/**
 * A turn the person's own Claude or ChatGPT plan stopped at its usage limit. The core continues it at `resetsAt` when
 * `scheduled` ("Continue at reset"), on the computer that runs the session, after a restart too. A message to the
 * session, archiving it or deleting it cancels that.
 */
export interface LimitStop {
  /** When the limit resets, epoch ms. Null: the agent did not say, or another Claude account has room (Continue now moves there). */
  resetsAt: number | null;
  scheduled: boolean;
}

export interface Project {
  id: string;
  name: string;
  /** Absolute path of the folder. */
  path: string;
  /** The checked-out branch when last looked at, if the folder is a git repository. */
  branch: string | null;
  /** The repository's remote URLs, `origin` first (credentials taken out): a prompt link names its project by one. Absent: none, or an older core. */
  remotes?: string[];
  /** Where its pull requests live, from its `origin` remote (source_host.rs). Absent: GitHub, or an older core. */
  host?: SourceHost;
  addedAt: number;
  /** The project's saved subsession flag. No start reads it: new sessions follow the computer's switch. Absent: off. */
  orchestration?: boolean;
  /** First in the sidebar and in every project picker (project-state.ts). Absent: not pinned, or an older core. */
  pinned?: boolean;
  /**
   * Out of the project pickers and the sidebar, which still shows it while one of its sessions works or needs you; its
   * sessions, settings, worktrees and files stay. Absent: not archived, or an older core.
   */
  archived?: boolean;
}

/**
 * A project command (nexus-workbench `scripts.rs`): a named shell command the person runs on request in a new terminal
 * of a session's folder, from the top bar, ⌘K or its own key. `shortcut` is a key as script-keys.ts writes it
 * (`Mod+Alt+KeyD`); absent: none.
 */
export interface ProjectScript {
  /** The app's, stable across renames. */
  id: string;
  name: string;
  command: string;
  shortcut?: string | null;
}

/**
 * When a schedule runs (nexus-workbench `schedules/when.rs`): every `minutes` (10 to 10080), at `minute` after local
 * midnight on `days` (0 is Sunday), or only when its webhook or Run now starts it.
 */
export type ScheduleWhen = { kind: "interval"; minutes: number } | { kind: "weekly"; days: number[]; minute: number } | { kind: "manual" };

/** What Settings saves for a schedule (`wb_schedule_save`). Absent fields take the core's defaults. */
export interface ScheduleInput {
  projectId: string;
  /** Null or blank: the prompt's first line. */
  title?: string | null;
  prompt: string;
  agent: Agent;
  model?: string | null;
  permissionMode: PermissionMode;
  effort?: Effort | null;
  via: Via;
  /** Each run in a new worktree of its own. Default: true. */
  worktree?: boolean;
  when: ScheduleWhen;
  /** Default: true. */
  enabled?: boolean;
  /** 1 to 48. Default: 12. */
  maxRunsPerDay?: number | null;
  /** A run Nexus missed (closed, asleep) runs once when it is back; false skips it. Default: true. */
  catchUp?: boolean;
}

/** One run of a schedule, newest first in its `runs`: the session it started, or why it did not. */
export interface ScheduleRun {
  at: number;
  trigger: "time" | "webhook" | "now";
  status: "started" | "failed" | "skipped";
  threadId?: string;
  note?: string;
}

/**
 * A scheduled session (nexus-workbench `schedules/`): a prompt that starts a new session on its project at its times,
 * from its webhook or with Run now, the way a person's new session starts. It runs while the app runs, at most
 * `maxRunsPerDay` times a local day. Its webhook's token is never in it.
 */
export interface Schedule {
  id: string;
  projectId: string;
  title: string;
  prompt: string;
  agent: Agent;
  model: string | null;
  permissionMode: PermissionMode;
  effort: Effort | null;
  via: Via;
  worktree: boolean;
  when: ScheduleWhen;
  enabled: boolean;
  maxRunsPerDay: number;
  catchUp: boolean;
  createdAt: number;
  /** The session whose agent added it (`schedule_session`); null: the person did. */
  createdBy: string | null;
  anchor: number;
  /** Null: it runs only when started (off, or manual). */
  nextRunAt: number | null;
  day: number;
  runsOnDay: number;
  runs: ScheduleRun[];
  webhook: boolean;
  /** The webhook's address while its listener runs. */
  webhookUrl?: string;
}

/** A webhook's address and token, shown once when it is turned on or its token replaced. */
export interface WebhookSecret {
  url: string;
  token: string;
}

/** One folder of a `wb_dirs` listing. */
export interface Dir {
  name: string;
  branch?: string;
  projectId?: string;
}

/** The folders in one folder (`wb_dirs`); `path` is absolute. */
export interface Dirs {
  path: string;
  branch?: string;
  projectId?: string;
  dirs: Dir[];
  truncated?: boolean;
}

/** Where a clone stands, as git last said it (`wb_project_clone_progress`): `Receiving objects` at 45%. */
export interface CloneProgress {
  stage: string;
  percent: number | null;
}

/**
 * A cloud machine's own setup (`wb_cloud_setup`): the machine clones its repository (or refreshes a saved
 * copy) and runs its `.agents/setup` at boot, with no app connected. `idle` when it has none: the app clones as before.
 * `cloning` carries git's stage and percent, `ready` the project, and `failed` why.
 */
export interface CloudSetup {
  state: "idle" | "cloning" | "setup" | "ready" | "failed";
  /** `owner/name`. */
  repo: string | null;
  stage: string | null;
  percent: number | null;
  project: Project | null;
  reason: string | null;
  /**
   * Why the repository's `.agents/resume` last failed or timed out (10 s), for the status line. Null: it passed or
   * none ran. It never changes `state`. An older host leaves it out.
   */
  resume?: string | null;
}

/** A session's own git worktree: where its agent works, its branch, and what the branch started from. */
export interface Worktree {
  path: string;
  branch: string;
  /** `origin/main`: what the diff and the pull request compare against. */
  base: string;
  /** Taken off the disk: the session can be read but no longer run. */
  removed: boolean;
  /** The session's latest pull request: opened from the app, or found by the watch on the branch. */
  pullRequest: string | null;
  /** Every pull request the session opened, oldest first; absent when it has none, or from an older core. */
  pullRequests?: string[];
  /** Someone else's pull request, checked out to review: never pushed, no pull request opened from it. */
  review?: { number: number; url: string } | null;
}

/** What the pull request page's merge button does. */
export type MergeAction = "merge" | "auto" | "queue";
/** A review thread: comments on one place in the diff. */
export interface ReviewThread {
  id: string;
  path: string | null;
  line: number | null;
  resolved: boolean;
  /** The lines it is on changed since. */
  outdated: boolean;
  canReply: boolean;
  canResolve: boolean;
  canUnresolve: boolean;
  comments: { id: string; author: string; body: string; createdAt: string | null; url: string | null }[];
}
/** A session's pull request page: the watch's read, its review threads, the repository's merge rules, and the merge. */
export interface PullPage {
  status: PullRequestStatus;
  /** The head commit: a merge goes through only while it is still the head. */
  head: string;
  /** GitHub's `mergeStateStatus`: `CLEAN`, `BLOCKED`, `BEHIND`, `UNSTABLE`, `DIRTY`, `DRAFT` or `UNKNOWN`. */
  mergeState: string;
  /** The merge methods the repository allows: `merge`, `squash`, `rebase`. */
  methods: string[];
  defaultMethod: string | null;
  autoAllowed: boolean;
  autoMerge: { method: string; by: string } | null;
  /** The base branch merges through a merge queue. */
  queue: boolean;
  inQueue: boolean;
  queuePosition: number | null;
  /** The checks a merge needs, by name. */
  required: string[];
  threads: ReviewThread[];
  /** `action` null: the button is disabled, for `blocked`'s reasons (or it is closed, merged, queued or auto-merging). */
  merge: { action: MergeAction | null; method: string | null; blocked: string[] };
}
/** Why the pull request page failed, by kind; `message` is one or two sentences to show. */
export interface PullError {
  kind: "unavailable" | "signedOut" | "timeout" | "notFound" | "session" | "invalid" | "blocked" | "changed" | "input" | "refused" | "github";
  message: string;
}

/** A source-control host whose pull requests the app opens, reviews and watches: GitHub through `gh`, GitLab through `glab`. */
export type SourceHost = "github" | "gitlab";

/** An open pull request of a project's repository, as the GitHub CLI lists it (or a GitLab merge request, as `glab` does). */
/** A worktree as Settings › Worktrees lists it (`cleanup::Listed`): a session's, or one no session names. */
export interface ListedWorktree {
  path: string;
  branch: string | null;
  /** Its session; null: no session's. */
  threadId: string | null;
  title: string | null;
  archived: boolean;
  updatedAt: number | null;
  live: boolean;
  missing: boolean;
}

export interface PullRequestInfo {
  number: number;
  title: string;
  url: string;
  /** The author's GitHub login. */
  author: string;
  /** Its branch. */
  head: string;
  /** The branch it targets. */
  base: string;
  /** Where it lives. Absent: GitHub. */
  host?: SourceHost;
}

/** A file of a session's folder for the viewer (`wb_file_read`). `size` is the whole file's, in bytes. */
export interface FileText {
  path: string;
  text: string | null;
  binary: boolean;
  truncated: boolean;
  size: number;
}

/** One selected output on the session's machine. Media is base64; text is at most 1 MiB. */
export interface FilePreview {
  path: string;
  mediaType: string;
  size: number;
  outsideProject: boolean;
  text: string | null;
  data: string | null;
  truncated: boolean;
  status: "ready" | "tooLarge" | "unsupported";
  /** Where `data` starts, when `wb_file_preview` was asked for a piece (`offset`); absent for a whole read. */
  offset?: number;
}

/**
 * One piece of an image or video that a reply names (`wb_file_media`): only inside the session folder, at
 * most 10 MB for an image and 50 MB for a video. `data` is base64, at most 4 MiB of the file from `offset`. With
 * `tooLarge`, `size` is the file's size and `data` is null.
 */
export interface MediaChunk {
  path: string;
  mediaType: string;
  size: number;
  offset: number;
  data: string | null;
  status: "ready" | "tooLarge";
}

/**
 * One line of project search (`wb_file_search`): `line` and `column` (of the first match) count from 1. A long line
 * comes as the part around the match; `start` is the column that part starts at.
 */
export interface FileSearchMatch {
  path: string;
  line: number;
  column: number;
  start: number;
  text: string;
}

export interface FileSearchResult {
  matches: FileSearchMatch[];
  truncated: boolean;
}

/** Where Code puts sessions' worktrees: new work's and reviews', each with its default. */
export interface WorktreeRoots {
  worktrees: string;
  reviews: string;
  worktreesDefault: string;
  reviewsDefault: string;
}

/**
 * When Code removes a session's worktree by itself (Settings): its pull request merged, it is archived, or it had no
 * activity for `idleDays` days (null: off). Only a worktree that loses nothing goes: no uncommitted changes, no commits
 * on no remote, its agent stopped, never the project's own folder.
 */
export interface CleanupRules {
  merged: boolean;
  archived: boolean;
  idleDays: number | null;
}

export type CleanupRule = "merged" | "archived" | "idle";

/** A session a cleanup rule matched: its worktree went, or stayed for the reason in `kept`. */
export interface CleanupEntry {
  threadId: string;
  title: string;
  path: string;
  branch: string;
  rule: CleanupRule;
  kept?: string;
}

/** One machine's sweep (`wb_worktree_cleanup`). */
export interface CleanupReport {
  at: number;
  removed: CleanupEntry[];
  kept: CleanupEntry[];
}

/** The last sweep, everywhere it ran: `machine` names the server (null: this computer). */
export interface LastCleanup {
  at: number;
  removed: (CleanupEntry & { machine: string | null })[];
  kept: (CleanupEntry & { machine: string | null })[];
  /** A machine it could not sweep, and why. */
  problems: string[];
}

/** When a hook runs (settings.json's `hooks[].event`; nexus-workbench's hooks.rs). */
export type HookEvent = "turnFinished" | "needsYou" | "settled" | "snoozed" | "archived" | "checkFailed";

/** A hook as settings.json keeps it: a shell command run on a Code session event. */
export interface Hook {
  event: HookEvent;
  command: string;
  /** Seconds, 1 to 600; absent: 60. */
  timeoutSecs?: number;
}

/** One run of a hook: how it ended and the last 20 lines of its output. */
export interface HookRun {
  hook: string;
  event: HookEvent;
  threadId: string;
  startedAt: number;
  /** Null: it timed out or did not start. */
  exitCode: number | null;
  timedOut: boolean;
  output: string;
}

/** Settings › Hooks: each hook with its last runs (newest first), and whether the organization lets them run. */
export interface HooksPage {
  hooks: (Hook & { id: string; runs: HookRun[] })[];
  enabled: boolean;
  managed: boolean;
}

/** Settings › Projects & worktrees: the cleanup rules, new worktrees' branch prefix, and the last sweep. */
export interface CleanupSettings {
  rules: CleanupRules;
  branchPrefix: string;
  branchPrefixDefault: string;
  last: LastCleanup | null;
}

/**
 * An MCP server every Code session gets (Settings): a local one (`command`, `args`) or a remote one (`url`).
 * `secrets` names a local server's environment variables or a remote server's headers; their values stay in the
 * keychain, and `saved` says which have one.
 */
export interface McpServer {
  name: string;
  enabled: boolean;
  command?: string;
  args?: string[];
  url?: string;
  secrets?: string[];
}

export interface McpServerView extends McpServer {
  saved: string[];
}

/** New secret values, by server name then secret name. */
export type McpValues = Record<string, Record<string, string>>;

/** A session's folder as git sees it: its git panel (topbar's branch). */
export interface GitState {
  /** Where the agent works: its worktree, else the project's folder. */
  path: string;
  /** Null: a detached HEAD, or not a git repository. */
  branch: string | null;
  /** Local branches to switch to, most recent first; none for a session with its own worktree (it keeps its branch). */
  branches: string[];
  /** What a commit would take, tracked and untracked. Null when git can't say (no commit yet, not a repository). */
  uncommitted: { files: number; insertions: number; deletions: number } | null;
  /** Null in a repository. In a folder that isn't in one: the repositories directly inside it, changed ones first. */
  repos: Repo[] | null;
  /** The branch against the one it tracks, as last fetched. Null: no upstream (never pushed), or a detached HEAD. */
  upstream: Tracking | null;
  /** A session's own worktree against the branch it was made from (`origin/main`). Null elsewhere. */
  base: Tracking | null;
}

/** Where the checked-out branch stands against another: the commits each has that the other lacks. */
export interface Tracking {
  /** The other branch, short (`origin/main`). */
  name: string;
  ahead: number;
  behind: number;
}

/** What `wb_git_pull` did. */
export interface PullResult {
  git: GitState;
  /** The rebase stopped on conflicts in these files and was undone: nothing changed. Null: the pull worked. */
  conflict: string[] | null;
}

/** What "Write it" drafts: the commit form's message, or the pull request form's title and description. */
export type DraftKind = "commit" | "pullRequest";

/** What a draft is written from, read from the repository on the machine that holds it (`wb_draft_context`). */
export interface DraftContext {
  kind: DraftKind;
  /** Null: a detached HEAD. */
  branch: string | null;
  /** A pull request's: the branch it merges into (`origin/main`). */
  base: string | null;
  /** Recent commit subjects, newest first: the repository's style. A pull request's are its base's. */
  subjects: string[];
  /** A pull request's: the branch's own commits, oldest first. */
  commits: string[];
  /** A pull request's: the repository's template (`.github/pull_request_template.md`), cut at 8 KB. */
  template: string | null;
  /** The change as a unified diff, cut at 40 KB: what isn't committed, or (a pull request) everything since its base. */
  diff: string;
  /** The diff was cut. */
  cut: boolean;
  /** New files git does not track yet, at most 100. */
  untracked: string[];
}

/** A drafted text. A commit message is all `title`, `body` empty. */
export interface Draft {
  title: string;
  body: string;
}

/** A repository directly inside a session's folder, as its git panel lists it. */
export interface Repo {
  /** Its folder's name: the `repo` the git commands take. */
  name: string;
  branch: string | null;
  uncommitted: GitState["uncommitted"];
}

/** An app a session's folder opens in: an editor or a terminal that is installed, or `files` (Finder, Explorer, Files). */
export interface Editor {
  id: string;
  name: string;
  kind: "editor" | "terminal" | "console" | "files";
}

/** What a session changed, as a unified diff (`git diff`), cut at 2 MiB. */
export interface Changes {
  diff: string;
  truncated: boolean;
}

/** A file one turn changed (nexus-workbench `turn_files.rs`): `path` is relative to the session's folder. */
export interface TurnFile {
  path: string;
  added: number;
  removed: number;
  status: "added" | "modified" | "deleted" | "renamed";
}

/**
 * What the Changes view compares besides the session's checkpoints (`wb_diff`): the folder as it is now against where
 * its branch left its base branch, one commit against its first parent, or the folder against HEAD.
 */
export type DiffSource = { kind: "branch" } | { kind: "commit"; id: string } | { kind: "uncommitted" };

/** One of a branch's newest commits, for the Changes view's picker. `time`: seconds since 1970. */
export interface Commit {
  id: string;
  short: string;
  subject: string;
  author: string;
  time: number;
}

/** What the Changes view can compare against (`wb_diff_targets`): the base branch (`origin/main`), and the newest 100 commits. */
export interface DiffTargets {
  base: string | null;
  commits: Commit[];
}

/** A session, as the sidebar and the header show it. ("Thread" in the code, "session" on screen.) */
export interface ThreadSummary {
  id: string;
  projectId: string;
  title: string;
  agent: Agent;
  /** The model asked for, or null for the organization's default (`nexus/auto`). */
  model: string | null;
  permissionMode: PermissionMode;
  /** The effort asked for, or null for the model's default. */
  effort: Effort | null;
  via: Via;
  provider?: string;
  /**
   * The Claude account a `subscription` session last ran on; null: Claude Code's own login. The core picks it at each
   * start, and moves the session to the account with the most left when its own reaches 90% of a limit.
   */
  account: string | null;
  status: ThreadStatus;
  /** Cost so far in USD: Claude Code's figure for finished turns plus the running estimate for the current one. */
  costUsd: number;
  /** The Nexus agent sessions (one per agent process) this session has billed, newest last. */
  nexusSessions: string[];
  /** Tokens of the latest call's context, for the context meter, and the model's window, when known. */
  contextTokens: number | null;
  contextWindow: number | null;
  createdAt: number;
  updatedAt: number;
  /** Creation, completion, or a request for attention. Absent from older hosts. */
  listedAt?: number | null;
  archived: boolean;
  /** Held at the top of its project in the sidebar. Absent from an older core (or server): not pinned. */
  pinned?: boolean;
  /**
   * Its unique name: 1 to 40 of A-Z, a-z, 0-9, ".", "_" and "-", never starting with "t_". Claude Code gets it as
   * `--name`. Absent: no name.
   */
  name?: string;
  /** The person's plan stopped its last turn at a usage limit. Absent: it did not, or a message has gone since. */
  limited?: LimitStop | null;
  /** Snoozed until this time (ms): out of the active list until then (snooze.ts). Absent: not snoozed. */
  snoozedUntil?: number;
  /** When its pull request's merge settled it: in the sidebar's "Settled" group. Absent: in the active list. */
  settledAt?: number;
  /** The pull request whose merge settled it; kept after it comes back. */
  settledBy?: number;
  /** "Keep in list": a merge never settles it. */
  keepListed?: boolean;
  /** null: the session works in its project's folder. */
  worktree: Worktree | null;
  /** Changes the running agent has not answered yet, shown over the values above. Absent when there are none. */
  pending?: Pending;
  /** What the running agent accepts. Absent with no live process, and from an older core: then no control shows. */
  capabilities?: AgentCapabilities;
  /** The session whose agent (or whose person, for a crew) started this one. Absent: the person started it. */
  parentThreadId?: string | null;
  /** A fork's source: the session it was forked from, which "Send back to source" writes to (`wb_send_back`). Absent: not a fork, or from an older core. It may since have been deleted. */
  sourceThreadId?: string | null;
  /** A side chat (side-chat.ts): a fork of its session (`sourceThreadId`) in that session's folder, shown in the session's side pane and never in the sidebar. Absent: not one. */
  sideChat?: boolean;
  /** Who started it: "agent" (its parent's agent) or "crew" (the person, from the new-session screen). */
  spawnedBy?: string | null;
  /** The session that handed its work to this one and was archived (`handoff_session`). Absent otherwise. */
  handoffFrom?: HandoffFrom | null;
  /** The group it was started in, with its siblings: a crew's request id, or the id of the parent's turn whose agent started it. */
  groupId?: string | null;
  /** 0 (absent) for a session the person started; its parent's depth + 1 for a linked one. */
  depth?: number;
  /** A subsession in another project than its parent's (`start_session` with `project`): a leaf, whose agent gets no subsession tools and can't hand off. Absent: not one. */
  crossProject?: boolean;
  /** Its agent may start, message, wait for and stop linked sessions. Absent: off. */
  orchestration?: boolean;
  /** The linked sessions its agent's open wait covers. Absent: none, or an older core. Never stored. */
  waitsOn?: string[];
  /** Its linked sessions' results it has not read yet; they go with its next message. Absent: none. Never stored. */
  pendingResults?: number;
  /** A choice before a linked session or its worktree exists. Absent from older hosts. */
  startChoices?: SessionStartChoice[];
  /** Why it runs no automatic turn: the person stopped it, or a session above it; "fault": a value this core can't read. Only a person message clears it. Absent: none. */
  halt?: "person" | "parent" | "fault" | null;
  /** Its tree reached a limit (automatic turns, empty waits or spend) since the person last wrote to its root. Never stored. */
  crewPaused?: boolean;
  /** The pause is the spend cap's. Never stored. */
  crewTripped?: boolean;
  /** Cap and estimated spend when this crew paused. Older hosts omit both. */
  crewSpendTrip?: { capUsd: number; spentUsd: number };
  /** Its turn is over and a background sub-agent (or a turn its agent owes) still holds it. Never stored. */
  inBackground?: boolean;
  /** It is at the deepest level linked sessions go, or a subsession in another project (`crossProject`): its agent gets no tools, and its orchestration can't be turned on. Never stored. */
  atDepthLimit?: boolean;
}
export type SessionStartPreference = "ask" | "new" | "here";
export interface SessionStartChoice {
  id: string;
  title: string;
  prompt: string;
  agent: Agent;
  /** Who pays for the child: Gateway, or the person's own plan for `agent`. Absent from an older host. */
  via?: Via;
  model: string | null;
  permissionMode: PermissionMode;
  workspace: string;
  canRemember: boolean;
}

/** Who wrote a `user` row other than the person: a linked session (its parent starting or messaging it). */
export type Sender = { kind: "session"; threadId: string; title: string };

/** The session that handed its work to another (`handoff_session`), as it was when it did. */
export interface HandoffFrom {
  threadId: string;
  title: string;
  agent: Agent;
}

/** A background command's latest output, read live (`wb_task_output`). */
export interface TaskOutput {
  /** At most its last 8 KiB. */
  output: string;
  /** How much it has written in all. */
  totalBytes: number;
  /** Earlier output is left out. */
  truncated: boolean;
}

/**
 * What the running agent process accepts, by task kind ("agent", "shell"; a kind not listed cannot be controlled),
 * from its version and the options Nexus sent it. Live only: never stored.
 */
export interface SessionAttribution {
  gateway: string | null;
  organization: string | null;
  team: string | null;
  access: "byok" | "nexus" | "subscription" | null;
  configuredMcp: string[] | null;
}
export interface RuntimeAccount {
  kind: string;
  email: string | null;
  plan: string | null;
}
export interface McpHealth {
  name: string;
  status: string;
  detail: string | null;
}
/** Safe observations from one selected process. Requested settings remain on ThreadSummary. */
export interface SessionConnection {
  generation: string;
  via: Via | null;
  provider: string | null;
  nativeId: string | null;
  reportedAt: number;
  model: string | null;
  version: string | null;
  effort: string | null;
  permissions: string | null;
  attribution: SessionAttribution;
  selectedAccount: string | null;
  account: RuntimeAccount | null;
  mcp: McpHealth[] | null;
  mcpUnavailable: string | null;
  skills: string[] | null;
  plugins: string[] | null;
}

export interface AgentCapabilities {
  connection?: SessionConnection;
  /** Current-process command discovery; absent before discovery, empty means none. */
  commands?: string[];
  /** Manual compaction through this host and runtime adapter. */
  compact?: boolean;
  /** The agent's version, as it reported it. */
  version?: string;
  /** The kinds of task `wb_task_stop` stops. */
  stop?: string[];
  /** The kinds of foreground task that can move to the background (`wb_task_background`). */
  background?: string[];
  /** The kinds of task whose output can be read live (`wb_task_output`). */
  output?: string[];
  /** The kinds of task that take a message of their own. None: no sub-agent of either agent does. */
  message?: string[];
  /** The session's Interrupt ends only the lead's turn; sub-agents and background commands run on. */
  interruptSparesAgents?: boolean;
}

/**
 * A mode, model or effort change sent to the running agent and not answered yet. The core saves it when the agent
 * accepts it, drops it when the agent refuses it (with a notice in the log), and saves it when the agent ends first.
 * An absent field is not pending; a null model or effort is a change back to the default.
 */
export interface Pending {
  permissionMode?: PermissionMode;
  model?: string | null;
  effort?: Effort | null;
}

/** Model capabilities reported by the installed agent. */
export interface AgentModel {
  id: string;
  name: string;
  efforts: Effort[];
  /** Canonical model behind a provider alias, when the agent reports it. */
  resolvedId?: string | null;
}

export interface Overview {
  executionIdentities?: import("./remote-computers").ExecutionIdentity[];
  projects: Project[];
  threads: ThreadSummary[];
  /**
   * Whether the agents are installed on this computer, and where. `problem`: why the program found cannot run the agent
   * (`path` is null then): a `nexus` that is the JavaScript CLI or too old for Nexus's agent.
   */
  agents: { agent: Agent; path: string | null; version: string | null; models?: AgentModel[] | null; problem?: string | null }[];
  /**
   * False when the organization keeps nothing on disk (`AllowLocalCache` off): sessions last until the app quits. Only
   * the Nexus sessions with a running agent are written down (each one's id, its session's id and its start time), so
   * a crash still ends them at the next launch.
   */
  persistent: boolean;
  /** The organization allows `via: "subscription"`. False hides the choice; the core enforces it either way. */
  agentSubscriptions: boolean;
  /** What this computer offers beyond the protocol (`HostInfo.features`): "orchestration" for linked sessions, "orchestration-controls" for the crew's controls. Absent from an older core. */
  features?: string[];
  /**
   * What this computer's agent runs read besides the session: Antigravity's Sandbox switch, and whether agy's last run
   * offered its `auto` mode (null: no run has said). Outside access reports each session's settings from it. Absent
   * from an older core.
   */
  runFacts?: { sandbox: boolean; autoOffered: boolean | null };
}

/** One usage limit of a plan: the five-hour session, the week, one model's week (`Weekly · Fable`). */
export interface PlanLimit {
  label: string;
  /** 0 to 100. */
  usedPercent: number;
  /** Epoch milliseconds. */
  resetsAt: number | null;
}

/** A plan's limits, as its agent reports them (`wb_plans`). */
export interface PlanUsage {
  agent: Agent;
  /** The agent's extra account (`wb_account_add`); null: the agent's own login. */
  account: string | null;
  email: string | null;
  /** As the agent names it: `Claude Max`, `Claude Team`, `plus`. */
  plan: string | null;
  /** Signed in to a plan. False: signed out, or on an API key (no plan limits). */
  signedIn: boolean;
  /** The Claude account a new session on the plan would start on. */
  active: boolean;
  limits: PlanLimit[];
  /** Why the limits could not be read. */
  error: string | null;
}

/** One suggestion Claude Code makes with an approval ("always allow `npm test`", "allow edits for this session"). */
export interface PermissionSuggestion {
  type: string;
  /** Short words for the button, made by the core from the suggestion. */
  label: string;
  /** The suggestion as Claude Code sent it, echoed back when chosen. */
  raw: unknown;
}

export type Decision = "allow" | "allowSession" | "deny";

/** An image sent with a message (a screenshot, say): its bytes in base64. PNG, JPEG, GIF or WebP, at most 8 a message. */
export interface WbImage {
  mediaType: string;
  data: string;
  name: string | null;
}

/**
 * More about a task than its first reports said: its place in the agent tree and its progress (nexus-workbench
 * `TaskDetail`). Every field is absent when not said; a later report's absent field keeps the earlier value.
 */
export interface TaskDetail {
  /** The task whose agent started this one; absent: the session's own agent did, or the agent did not say. */
  parentTaskId?: string | null;
  /** 1 for a task the session's own agent started, N + 1 for one a depth-N sub-agent started. */
  depth?: number | null;
  model?: string | null;
  /** The agent's own name for it (a Codex nickname). */
  name?: string | null;
  /** Its latest action, in the agent's words. */
  activity?: string | null;
  /** It runs past the call that started it. */
  backgrounded?: boolean | null;
  /** One run of the task: a task that runs again keeps its id and gets a new run. */
  runId?: string | null;
  /** An update of a running task: it never starts, resumes or renames it. */
  progress?: boolean | null;
  /** Why it ended: "worker_restart" (the agent), "process_ended" or "app_closed" (Nexus wrote the report). */
  reason?: string | null;
}

export type WbEvent =
  /**
   * `images` is absent when there are none. `from` is absent on the person's own messages; a row with it was written
   * by a linked session, so readers that take a `user` row as the person's (the timeline's labels, editing a prompt,
   * the queue) skip it or show it as the session's.
   */
  | { kind: "user"; text: string; images?: WbImage[]; from?: Sender | null }
  /** A run of the agent process began: its Nexus session, Claude's own session id, the model it resolved. */
  | { kind: "session"; nexusSessionId: string | null; nativeId: string | null; model: string | null; via: Via; resumed: boolean }
  | { kind: "turnStart"; turnId: string }
  | { kind: "text"; id: string; text: string; parent: string | null }
  | { kind: "thinking"; id: string; text: string; parent: string | null }
  /** A tool call. `parent` is the tool call of the sub-agent (Task) that made it, if any. */
  | { kind: "tool"; id: string; name: string; input: unknown; parent: string | null }
  /** Text is capped at 64 KiB (`truncated` then true). Inline images are separate. `parent` names the sub-agent. */
  | { kind: "toolResult"; id: string; output: string; images?: WbImage[]; isError: boolean; truncated: boolean; parent?: string | null }
  | {
      kind: "approval";
      requestId: string;
      toolUseId: string | null;
      tool: string;
      input: unknown;
      /** Claude Code's own sentence for what it wants to do, when it gives one. */
      description: string | null;
      suggestions: PermissionSuggestion[];
      /** The sub-agent that asks: its Agent call's id, or its task id when its call is not known. Absent: the session's own agent, or not said. */
      parent?: string | null;
    }
  /** `expired`: the agent stopped before the person answered. */
  | { kind: "approvalResolved"; requestId: string; decision: Decision | "expired"; message: string | null }
  /** One model call's usage, priced with the Gateway catalog. */
  | {
      kind: "usage";
      model: string;
      inputTokens: number;
      outputTokens: number;
      cacheReadTokens: number;
      cacheWriteTokens: number;
      costUsd: number;
      /** The sub-agent that made the call: its Agent call's id. Absent: the session's own agent, or a Codex log written before the core kept it. */
      parent?: string | null;
    }
  | {
      kind: "turnEnd";
      turnId: string;
      /** `interrupted`: Stop, or the app quit during the turn. */
      status: "completed" | "interrupted" | "error";
      /** Claude Code's figure for the turn, when it reports one. */
      costUsd: number | null;
      durationMs: number;
      error: string | null;
      /** Where the agent's own conversation stood (its last message, the Codex turn), for a revert to go back to. */
      cursor?: string;
    }
  /**
   * An agent's report on one of its tasks (a sub-agent, a command it left running): Claude Code's `task_started`,
   * `task_updated` and `task_notification`, or a Codex child thread's start and end. It started, its state changed, or
   * it ended. Later events with the same `taskId` replace what earlier ones said; `taskId` links them, since an update
   * has no `toolUseId`. `description` is what the task does on its start, the agent's summary of how it went on its
   * end, and empty on an update.
   */
  | {
      kind: "task";
      taskId: string;
      /** The tool call that started it (the sub-agent's Agent call, the shell's Bash call), when said. */
      toolUseId: string | null;
      /** `local_bash`, `local_agent` and so on, when the agent says. */
      taskType: string | null;
      /** A sub-agent's type ("general-purpose", "Explore"), on its start. */
      agentType: string | null;
      status: "running" | "completed" | "failed" | "stopped";
      description: string;
      /** On a sub-agent's end: its tool calls, tokens and time. `totalTokens` is the size of its latest model call (input, cache and output), not the tokens it spent. */
      toolCount: number | null;
      totalTokens: number | null;
      durationMs: number | null;
      /** Its place in the agent tree and its progress, when the agent says. */
      detail?: TaskDetail | null;
    }
  /**
   * Claude Code's full list of the background tasks still running, each time it changes. A task that leaves it has
   * ended, even before its `task` report says how.
   */
  | { kind: "backgroundTasks"; tasks: { taskId: string; taskType: string | null; description: string }[] }
  /** The slash commands Claude Code offers, as it reported them on start, without the leading slash. */
  | { kind: "connection"; connection: SessionConnection }
  | { kind: "commands"; commands: string[] }
  /**
   * The project's setup script in the session's new worktree, run before the agent's first turn (Settings, Setup
   * scripts). Events with the same `id` follow it: each adds the `output` since the one before, the last says how it ended.
   */
  | { kind: "setup"; id: string; command: string; status: SetupStatus; output: string; exitCode: number | null; durationMs: number | null }
  /** The person took back the queued message `seq` (its `user` event) before the agent read it. */
  | { kind: "queueCancel"; seq: number }
  /** The person rewrote the queued message `seq`: the agent reads `text` instead. */
  | { kind: "queueEdit"; seq: number; text: string }
  /** The person moved the queued message `seq` just before the queued message `before`, or last when `before` is null. */
  | { kind: "queueMove"; seq: number; before: number | null }
  /** The person sent the queued message `seq` into the running turn ("Send now"): it no longer waits for a turn. */
  | { kind: "queueSteer"; seq: number }
  /**
   * The agent read the message `seq` (its `user` event): it said so as it took the message in (Claude Code's echo, a
   * Codex `userMessage` item). A message without one was not confirmed: an older log, or one the agent never got.
   */
  | { kind: "read"; seq: number }
  /**
   * The files were put back as they were when the turn ending at event `turn` ended (`wb_revert_to_turn`); `files`
   * changed. The turns between it and this event are reverted: the agent no longer has them, and they can't be reverted to.
   */
  | { kind: "revert"; turn: number; files: number }
  /** Start again before this user message, preserving the files and the dimmed history. */
  | { kind: "rewind"; before: number }
  /** Something the app says about the session ("Stopped when Nexus quit", "Agent stopped after 30 idle minutes"). */
  | { kind: "notice"; text: string }
  | { kind: "agentSwitched"; from: Agent; to: Agent; text: string }
  | { kind: "error"; text: string; detail: string | null };

/** `timedOut`: stopped after ten minutes. `stopped`: stopped by the person (`wb_setup_stop`). `failed` also covers a script that could not start. */
export type SetupStatus = "running" | "done" | "failed" | "timedOut" | "stopped";

export interface StoredEvent {
  threadId: string;
  /** From 1 per session, no gaps. */
  seq: number;
  at: number;
  event: WbEvent;
}

/** One full logical partial in `wb_thread_stream`; never a sent-text suffix. */
export interface ThreadStreamMessage {
  id: string;
  /** Null: the main answer; otherwise the sub-agent's tool call. */
  parent: string | null;
  text: string;
}

/** One source-authorized durable/partial cut from the owner (nexus-workbench stream_snapshot.rs). */
export interface ThreadStreamSnapshot {
  threadId: string;
  summary: ThreadSummary;
  /** Opaque process UUID, not an authorization marker. Null: no live process. */
  incarnation: string | null;
  revision: number;
  /** Latest durable sequence at this cut, which may be after this catch-up page. */
  throughSeq: number;
  /** At most 256 durable rows after the requested cursor, oldest first. */
  events: StoredEvent[];
  /** More durable rows remain; messages is empty until the reader catches up. */
  more: boolean;
  /** Ordered full text; empty clears the displayed partial, also while catching up. */
  messages: ThreadStreamMessage[];
}

/** A page of a session's log (`wb_thread_page`). */
export interface EventsPage {
  /** Contiguous by seq, oldest first. */
  events: StoredEvent[];
  /**
   * The latest page only: the latest slash commands, main-agent plan (TodoWrite) and background-task list from before
   * it, oldest first. Their state still holds; they are not rows of the timeline.
   */
  carried: StoredEvent[];
  /** Older events exist before `events`. */
  more: boolean;
}

/** A session whose prompts or answers match a search (`wb_search`), at its best-matching message. */
export interface SearchHit {
  threadId: string;
  /** The matching message's event. */
  seq: number;
  /** About 48 characters around the match; the matched text sits between "\u0002" and "\u0003". */
  snippet: string;
}

/** Streamed text of the assistant message `id` so far (`wb:delta`). Never stored: the `text` event with that id replaces it. */
export interface Delta {
  threadId: string;
  id: string;
  text: string;
  /** The tool call of the sub-agent this text belongs to; null: the main answer. Codex streams a child's text; Claude Code sends a sub-agent's whole. */
  parent?: string | null;
  /**
   * Absent: `text` is the whole text so far. A number: `text` follows that many UTF-16 units (a string's length) of
   * the text before. The core sends the whole text first, and again after the log is read (`wb_thread_events`).
   */
  offset?: number;
}

export interface PreviewProbe { url: string; reachable: boolean; frameError: string | null; error: string | null; }

export interface PullRequestStatus {
  number: number;
  title: string;
  url: string;
  draft: boolean;
  state: string;
  checks: { name: string; state: "pending" | "success" | "failure" | "skipped"; url: string | null }[];
  /** `APPROVED`, `CHANGES_REQUESTED` or `REVIEW_REQUIRED`; null when the repository asks for no review. */
  reviewDecision: string | null;
  /** Each reviewer's latest review. */
  reviews: { id: string; author: string; state: string; body: string; submittedAt: string | null }[];
  /** `MERGEABLE`, `CONFLICTING`, or `UNKNOWN` while GitHub computes it. */
  mergeable: string;
  /** Where it lives. Absent: GitHub. */
  host?: SourceHost;
}
/** A session's pull request as the core watches it. */
export interface PullWatch {
  threadId: string;
  /** The last read; kept through a later error. */
  status: PullRequestStatus | null;
  /** Why the last read failed; null after a read that worked. */
  problem: string | null;
  /** The core reads it every two minutes. */
  watching: boolean;
  /** A check failed, a review asked for changes or a conflict appeared since the person last looked at the session. */
  needsYou: boolean;
  /** The core sends the agent what to fix on its own, at most three times per session (`wakes`). */
  autoWake: boolean;
  wakes: number;
  /** The agent asked to be told of every change (its `watch_pull_request` tool); it shares `wakes`' cap. */
  agentWatch: boolean;
  readAt: number | null;
  /** Every pull request of the session the watch read, newest first; `status` is the one that most needs the person. */
  pulls?: PullRequestStatus[];
}
/**
 * An agent's open request for a secret (its `request_secret` tool). The value is
 * never in it: the card sends it to the core, which writes it to a private file and gives the agent only its path.
 */
export interface SecretRequest {
  id: string;
  threadId: string;
  /** What the secret is, as the agent named it. */
  name: string;
  /** Why the agent needs it. */
  reason: string;
}
export interface BrowserRequest { id: string; threadId: string; url: string; tab: string | null; at: number }
/** A file of a message: its bytes whole in `data` (base64, a piece or less), or uploaded in pieces already (`upload`). */
export type SentFile = { name: string; data: string } | { name: string; upload: string };

export interface Commands {
  wb_computer_list: { args: Record<string, never>; result: import("@nexus/kit").ComputerList };
  wb_computer_info: { args: Record<string, never>; result: import("./remote-computers").ComputerInfo };
  wb_computer_name_set: { args: { name: string }; result: import("./remote-computers").ComputerInfo };
  wb_computer_join_begin: { args: { link: string }; result: { inviteId: string; computerId: string; computerName: string; state: "pending" | "approved" | "cancelled" | "expired" } };
  wb_computer_join_poll: { args: { inviteId: string }; result: Commands["wb_computer_join_begin"]["result"] };
  wb_computer_join_allow: { args: { inviteId: string }; result: Commands["wb_code_control"]["result"] };
  wb_computer_request: { args: { computerId: string; requestId: string; operation: import("@nexus/kit").ComputerOperation }; result: import("./remote-computers").ComputerReceipt };
  wb_computer_request_get: { args: { computerId: string; requestId: string }; result: import("./remote-computers").ComputerReceipt };
  wb_local_history: { args: { cursor?: string; limit?: number }; result: import("./remote-computers").LocalHistoryPage };
  wb_synced_thread_page: { args: { threadId: string; before?: number; after?: number; limit?: number }; result: import("./remote-computers").SyncedPage };
  wb_synced_command: { args: { threadId: string; requestId: string; action: import("@nexus/kit").CodeAction }; result: import("@nexus/kit").CodeCommand };
  wb_synced_command_get: { args: { threadId: string; requestId: string }; result: import("@nexus/kit").CodeCommand };
  wb_phone_invitation_begin: { args: Record<string, never>; result: import("@nexus/kit").PhoneInvitation };
  wb_phone_invitation_poll: { args: { inviteId: string }; result: { state: "waiting" | "approving" | "connected" | "expired" | "cancelled"; deviceId: string | null; name: string | null; code: string | null } };
  wb_phone_invitation_approve: { args: { inviteId: string; deviceId: string; code: string }; result: Commands["wb_phone_invitation_poll"]["result"] };
  wb_phone_invitation_cancel: { args: { inviteId: string }; result: null };
  wb_code_control: { args: Record<string, never>; result: { available: boolean; enabled: boolean } };
  wb_code_control_set: { args: { enabled: boolean }; result: { available: boolean; enabled: boolean } };
  /** The session's pull request: the watch's last read, or a read now (which starts a watch when it finds one open). */
  wb_pull_watch: { args: { threadId: string }; result: PullWatch };
  /** Every pull request the core watches or read. */
  wb_pull_watches: { args: Record<string, never>; result: PullWatch[] };
  /** Queues the agent the failing checks with their links, the reviews that ask for changes, and a conflict. */
  wb_pull_watch_send: { args: { threadId: string }; result: PullWatch };
  /** The session's switch: send the agent that message on its own. */
  wb_pull_watch_auto: { args: { threadId: string; on: boolean }; result: PullWatch };
  /** Turns off the watch the session's agent armed; the person's own watch goes on. */
  wb_pull_watch_agent_off: { args: { threadId: string }; result: PullWatch };
  /** The open secret requests, one per session at most. */
  wb_secret_requests: { args: Record<string, never>; result: SecretRequest[] };
  /** Answers a secret request: `value` provides it, null declines it. */
  wb_secret_answer: { args: { id: string; value: string | null }; result: null };
  /** `machine`: the cloud machine of a session that runs there (its browser shows the machine's forwarded ports). */
  wb_browser: { args: { threadId: string; action: string; args: Record<string, unknown>; machine?: string }; result: unknown };
  wb_browser_requests: { args: Record<string, never>; result: BrowserRequest[] };
  wb_browser_decline: { args: { id: string }; result: null };
  wb_preview_probe: { args: { threadId: string; urls: string[]; machine?: string }; result: PreviewProbe[] };
  wb_overview: { args: Record<string, never>; result: Overview };
  /** The sessions with their live fields, without `wb_overview`'s agent check and Gateway call. */
  wb_threads: { args: Record<string, never>; result: ThreadSummary[] };
  wb_agent_models: { args: Record<string, never>; result: Overview["agents"] };
  /**
   * Installs the native nexus CLI for Nexus's agent from the signed-in Gateway, on this computer, once it answers the
   * agent's handshake. Its new entry; an error says why nothing changed (the Gateway publishes no build for it, say).
   */
  wb_nexus_install: { args: Record<string, never>; result: Overview["agents"][number] };
  /** The OS folder picker. Null when cancelled. */
  wb_pick_folder: { args: Record<string, never>; result: string | null };
  wb_project_add: { args: { path: string }; result: Project };
  /**
   * Clones `source` (a repository URL, or GitHub's `owner/repo`, found with the machine's `gh` sign-in) into
   * `<parent>/<its name>` (`~` is the machine's home), then opens it as a project. Over SSH first when that is the
   * URL or the person's `gh` protocol, then over HTTPS when the SSH key is refused. Refused when that folder holds
   * anything. Offered when the core's features have "project-clone".
   */
  wb_project_clone: { args: { source: string; parent: string; requestId: string }; result: Project };
  /** Where the clone `requestId` stands; null once it ended. */
  wb_project_clone_progress: { args: { requestId: string }; result: CloneProgress | null };
  /**
   * The folders in `path` on the machine, for Add a project's folder field (`~` is its home folder; a relative path is
   * refused). Visible folders first, then hidden ones, each in name order; at most 200 (`truncated`). Each names its
   * Git branch and the project it already is. Offered as the `project-dirs` feature.
   */
  wb_dirs: { args: { path: string }; result: Dirs };
  /** Where the machine's own setup stands. Only a cloud machine has one; anywhere else it is `idle`. */
  wb_cloud_setup: { args: Record<string, never>; result: CloudSetup };
  /** Runs a failed setup again, with a new token. A setup that has not failed goes on as it was. */
  wb_cloud_setup_retry: { args: Record<string, never>; result: CloudSetup };
  /** `~/nexus`, created if missing, as a project. */
  wb_project_default: { args: Record<string, never>; result: Project };
  /** Closes the folder. Its sessions are archived, not deleted, and come back if the folder is opened again. */
  wb_project_remove: { args: { projectId: string }; result: null };
  /**
   * Pins or unpins the project, and archives it or brings it back; a field left out stays as it is. Answers with the
   * project as saved. Offered when the core's features have "project-pin-archive".
   */
  wb_project_update: { args: { projectId: string; pinned?: boolean; archived?: boolean }; result: Project };
  /** The command run in each new worktree of the project before its first turn. Null: none. */
  wb_project_setup: { args: { projectId: string }; result: string | null };
  /** Sets it (trimmed; blank or null takes it away) and answers with what was saved. Applies to worktrees made from now on. */
  wb_project_setup_set: { args: { projectId: string; script: string | null }; result: string | null };
  /** The project's commands, in the person's order. Empty: none. */
  wb_project_scripts: { args: { projectId: string }; result: ProjectScript[] };
  /**
   * Replaces them (names and commands trimmed; an empty list takes them away) and answers with what was saved.
   * Refused when one has no name or command, or two share a name or a key.
   */
  wb_project_scripts_set: { args: { projectId: string; scripts: ProjectScript[] }; result: ProjectScript[] };
  /** The project's new-session defaults, kept in the store. Each one unset follows the last pick. */
  wb_project_defaults: { args: { projectId: string }; result: StoredProjectDefaults };
  /** Sets them (a model only with its agent; all unset takes them away) and answers with what was saved. */
  wb_project_defaults_set: { args: { projectId: string; defaults: StoredProjectDefaults }; result: StoredProjectDefaults };
  /** The project's answer about its own Claude settings (null: never asked) and what they would turn on. */
  wb_project_claude_trust: { args: { projectId: string }; result: ClaudeTrust };
  /** Keeps the answer. Untrusted, Claude and OpenCode sessions there load only the person's own settings. */
  wb_project_claude_trust_set: { args: { projectId: string; trusted: boolean }; result: ClaudeTrust };
  /** The organization's schedules on this computer, oldest first. */
  wb_schedules: { args: Record<string, never>; result: Schedule[] };
  /** Adds a schedule (no `scheduleId`) or replaces one, and answers with what was saved. Refused with a reason Settings shows. */
  wb_schedule_save: { args: { scheduleId?: string | null; schedule: ScheduleInput }; result: Schedule };
  wb_schedule_delete: { args: { scheduleId: string }; result: null };
  /** Run now: its session starts at once, even while it is off, counted against its daily cap. */
  wb_schedule_run: { args: { scheduleId: string }; result: ThreadSummary };
  /** Turns its webhook on with a new token, shown only in this answer, or off (null). Refused where webhooks are not available (a remote host). */
  wb_schedule_webhook: { args: { scheduleId: string; on: boolean }; result: WebhookSecret | null };
  /** Stops the session's running setup script with what it started; its waiting messages then go on to the agent. Nothing when none runs. */
  wb_setup_stop: { args: { threadId: string }; result: null };
  /** Fails, with nothing started, when `via` is `subscription` and the organization does not allow it. */
  wb_thread_create: {
    args: {
      /**
       * Stable across retries; the core creates at most one session for this UUID. A retry names the same project and
       * agent, or the core refuses it.
       */
      requestId?: string;
      projectId: string;
      agent: Agent;
      model: string | null;
      permissionMode: PermissionMode;
      effort: Effort | null;
      via: Via;
      /** Configured local provider instance on this computer. */
      provider?: string;
      /** The first message: the session starts with it. */
      text: string;
      images: WbImage[];
      files?: SentFile[];
      /**
       * Work in a new git worktree: its branch (null: named from the first message) and base (null: the default branch).
       * With `review`, that pull request of the repository instead, checked out under the reviews folder.
       */
      worktree: { branch: string | null; base: string | null; review?: number; branchPrefix?: string } | null;
      /** Whether its agent may start subsessions; absent: the computer's switch, on by default. */
      orchestration?: boolean;
      /** The session's unique name (ThreadSummary.name); absent or null: none. Refused when invalid or taken. */
      name?: string | null;
    };
    result: ThreadSummary;
  };
  /** Fails, removing nothing, when the worktree has uncommitted changes or unpushed commits, or the agent is running. */
  wb_worktree_remove: { args: { threadId: string }; result: ThreadSummary };
  /** This scope's sessions' worktrees, then the folders in the worktrees and reviews folders that no session names. */
  wb_worktrees: { args: Record<string, never>; result: ListedWorktree[] };
  /** Removes a worktree no session names (one `wb_worktrees` lists without a `threadId`), unless that would lose work. */
  wb_worktree_remove_orphan: { args: { path: string }; result: null };
  /** A host's sweep of its own sessions by the rules (the app sends its own on each connected server). */
  wb_worktree_cleanup: { args: { rules: CleanupRules }; result: CleanupReport };
  /**
   * The session's pull request `number` merged: it settles, unless a turn runs or the person keeps it listed. The
   * pull request watch of an app whose sessions run in its background service sends it there. Null: not settled.
   */
  wb_settle_merged: { args: { threadId: string; number: number; archive?: boolean }; result: ThreadSummary | null };
  /**
   * A check on the session's pull request failed: the hooks of the store's owner run (Settings › Hooks). The pull
   * request watch of an app whose sessions run in its background service sends it there. It adds no hook.
   */
  wb_check_failed: { args: { threadId: string; name: string; url: string | null }; result: null };
  /** The pull request watch found the branch's pull request: the session saves its link, when it has none. */
  wb_link_pull_request: { args: { threadId: string; url: string }; result: ThreadSummary | null };
  /**
   * Moves a session that works in its project's folder into a new worktree on a branch of its own, with what the folder
   * had not committed (staged, unstaged, untracked), and leaves the folder clean. The agent stops, and the next message
   * resumes it in the new folder. Fails, changing nothing, mid-turn or while another session works in the folder.
   * Offered when the core's features have "move-to-worktree".
   */
  wb_move_to_worktree: { args: { threadId: string }; result: ThreadSummary };
  /**
   * Commits what changed (as `title`), pushes the branch, and opens its pull request with the GitHub CLI against the
   * worktree's base, or finds the open one (the push updated it). Fails while the agent is in a turn.
   */
  wb_pull_request: { args: { threadId: string; title: string; body: string }; result: ThreadSummary };
  /** `repo` (in every git command here): a repository inside the session's folder, by the name `GitState.repos` gave. */
  wb_git_state: { args: { threadId: string; repo?: string }; result: GitState };
  /** Commits everything that changed as `message`, then pushes the branch when `push`; an empty message with `push` only pushes. Fails mid-turn. */
  wb_commit: { args: { threadId: string; repo?: string; message: string; push: boolean }; result: GitState };
  /**
   * What "Write it" drafts from (`kind`): what isn't committed (a commit, in `repo` as `wb_commit` takes it), or the
   * session's worktree branch against its base (a pull request). Fails when there is nothing to describe.
   */
  wb_draft_context: { args: { threadId: string; repo?: string; kind: DraftKind }; result: DraftContext };
  /**
   * The app's: one model call through Nexus Gateway with the person's token, drafting the text from `context`. `model`:
   * the session's on Gateway's route; null or `auto` lets Gateway pick (`nexus/auto`). Fails with Gateway's error.
   */
  wb_draft_write: { args: { context: DraftContext; model: string | null }; result: Draft };
  /** Project-folder sessions only. Fails mid-turn, or when git would overwrite uncommitted work. */
  wb_switch_branch: { args: { threadId: string; repo?: string; branch: string }; result: GitState };
  /** Fetches what the branch is compared against, then reads the state. Only reads mid-turn or within a minute of the last fetch. */
  wb_git_fetch: { args: { threadId: string; repo?: string }; result: GitState };
  /** Pulls the upstream: fast-forward only, or with `rebase` a rebase that is undone when it conflicts. Fails mid-turn. */
  wb_git_pull: { args: { threadId: string; repo?: string; rebase: boolean }; result: PullResult };
  /** A new branch at the current commit, checked out. Project-folder sessions only; fails mid-turn. */
  wb_branch_create: { args: { threadId: string; repo?: string; branch: string }; result: GitState };
  wb_editors: { args: Record<string, never>; result: Editor[] };
  /**
   * Each plan's limits on this computer: Claude Code's own login and each extra Claude account (oldest first), then
   * the same for Codex. The agents report them; an agent that is not installed has no entry. Takes a second or two.
   */
  wb_plans: { args: Record<string, never>; result: PlanUsage[] };
  /** A new, signed-out account of the agent: its id. `connect_agent` with the agent and the id signs it in. */
  wb_account_add: { args: { agent: Agent }; result: string };
  /** Signs the account out of its agent and removes it. Its sessions stay, and say so when they next start. */
  wb_account_remove: { args: { agent: Agent; account: string }; result: null };
  /** Where new sessions on the Claude plan start: an extra Claude account, or Claude Code's own login (null). */
  wb_account_use: { args: { account: string | null }; result: null };
  /** Whether sessions on the Claude plan move to the account with the most left at 90% of a limit. On by default. */
  wb_rotation: { args: Record<string, never>; result: boolean };
  wb_rotation_set: { args: { on: boolean }; result: null };
  /** Whether sessions show a system notification when they finish or need you (Settings). On by default. */
  wb_notifications: { args: Record<string, never>; result: boolean };
  wb_notifications_set: { args: { on: boolean }; result: null };
  /** The person's hooks and their last runs (Settings › Hooks). This computer's only. */
  wb_hooks: { args: Record<string, never>; result: HooksPage };
  /** Saves the hooks to settings.json; refuses an empty command and changes nothing then. */
  wb_hooks_set: { args: { hooks: Hook[] }; result: HooksPage };
  /** Whether this computer shows the organization's notifications, collected as its device (Settings). On by default. */
  wb_path_notifications: { args: Record<string, never>; result: boolean };
  wb_path_notifications_set: { args: { on: boolean }; result: null };
  /** The session the main window shows (null: none). No notification for it while the window is focused. */
  wb_viewing: { args: { threadId: string | null }; result: null };
  /** Where new sessions' worktrees and reviews go (Settings). */
  wb_worktree_roots: { args: Record<string, never>; result: WorktreeRoots };
  /** Changes one of them (`path` null: back to the default). Sessions already made keep theirs. */
  wb_worktree_roots_set: { args: { kind: "worktrees" | "reviews"; path: string | null }; result: WorktreeRoots };
  /** The cleanup rules, the branch prefix and the last sweep (Settings). */
  wb_worktree_cleanup_settings: { args: Record<string, never>; result: CleanupSettings };
  /** Saves the rules; the hourly sweep, and "Clean up now", use them. Fails past 365 days. */
  wb_worktree_cleanup_set: { args: { rules: CleanupRules }; result: CleanupSettings };
  /** A sweep now, on this computer and each connected server. Fails with every rule off. */
  wb_worktree_cleanup_run: { args: Record<string, never>; result: LastCleanup };
  /** What new worktrees' branches start with (null: back to `nexus/`; "": none). Fails on a prefix git refuses. */
  wb_branch_prefix_set: { args: { prefix: string | null }; result: CleanupSettings };
  /** The MCP servers every session gets (Settings). */
  wb_mcp_servers: { args: Record<string, never>; result: McpServerView[] };
  /** Replaces the list; `values` holds only new secret values (one left out keeps its saved value). */
  wb_mcp_servers_set: { args: { servers: McpServer[]; values: McpValues }; result: McpServerView[] };
  /** The open pull requests of the project's repository (GitHub CLI), for a review session. */
  wb_pull_requests: { args: { projectId: string }; result: PullRequestInfo[] };
  /** What a new worktree in the project can start from: the default base (`origin/main`) first, then the checked-out branch, then the other branches, most recent first. */
  wb_worktree_bases: { args: { projectId: string }; result: string[] };
  wb_open_in: { args: { threadId: string; repo?: string; editor: string }; result: null };
  /**
   * Opens one file of the session in `editor` (an id from `wb_editors`), at `line` and `col` (1-based) when the editor
   * takes them. `path`: absolute or relative to the session's folder; a file outside it (or a path with `..`) fails. A
   * terminal, no editor or one not installed: the system's default app for the file.
   */
  wb_open_output: { args: { threadId: string; path: string }; result: null };
  wb_open_file: { args: { threadId: string; path: string; editor?: string; line?: number; col?: number }; result: null };
  /** The turns whose changes were recorded (git folders only), by their `turnEnd` event's `seq`, oldest first. */
  wb_changed_turns: { args: { threadId: string }; result: number[] };
  /** One turn's changes (`turn`: its `turnEnd` seq), or null for the whole session against the worktree's base. */
  wb_changes: { args: { threadId: string; turn: number | null }; result: Changes };
  /** The files one turn changed (`turn`: its `turnEnd` seq), with lines added and removed, from its checkpoint. */
  wb_turn_files: { args: { threadId: string; turn: number }; result: TurnFile[] };
  /** The session's folder compared as `source` says. Offered when the core's features have "diff-sources". */
  wb_diff: { args: { threadId: string; source: DiffSource }; result: Changes };
  /** The base branch and newest commits `wb_diff` can compare against. Offered with "diff-sources" too. */
  wb_diff_targets: { args: { threadId: string }; result: DiffTargets };
  /** How many files reverting to the turn ending at event `turn` would change. Fails when it can't be reverted to. */
  wb_revert_preview: { args: { threadId: string; turn: number }; result: number };
  /**
   * Puts the session's files back as they were after that turn, and the agent forgets the turns after it (from its
   * next message). Fails, changing nothing, while a turn runs. Returns how many files changed.
   */
  wb_revert_to_turn: { args: { threadId: string; turn: number }; result: number };
  /** Rewind the conversation before a user message. Refused while the agent or its background work runs. */
  wb_rewind_message: { args: { threadId: string; seq: number }; result: void };
  wb_thread_events: { args: { threadId: string; afterSeq: number }; result: StoredEvent[] };
  /** Optional on older owners; unsupported readers keep using durable events. */
  wb_thread_stream: { args: Commands["wb_thread_events"]["args"]; result: ThreadStreamSnapshot };
  /**
   * A page of a session's log (page.rs): at least `limit` events (null: all) before `beforeSeq` (null: the latest page),
   * starting where no turn ran, so the turn in progress, its approvals and queued messages are all in it.
   */
  wb_thread_page: { args: { threadId: string; beforeSeq: number | null; limit: number | null }; result: EventsPage };
  /**
   * The files of a session's folder, or a project's (a new session's), relative with "/", for the composer's @-mentions:
   * the tracked and untracked files less the ignored ones (outside a repository, a walk that skips hidden and build
   * folders). At most 25,000 (`truncated` then); the core keeps a folder's list for 10 seconds.
   */
  wb_files: { args: { threadId: string } | { projectId: string }; result: { files: string[]; truncated: boolean } };
  /**
   * One file of the session's folder for the file viewer (`path` relative, with "/"). Refused when the path is absolute,
   * climbs with `..` or resolves through a link outside the folder. At most 1 MiB, cut at a line (`truncated`); a binary
   * file comes without `text`. Only on a core that says `project-files` in its features.
   */
  /**
   * A selected output on the session's machine. With `offset` (hosts with `file-preview-pieces`), an image or PDF
   * comes as the piece of at most 4 MiB from there, so a large file fits a remote connection's messages.
   */
  wb_file_preview: { args: { threadId: string; path: string; offset?: number }; result: FilePreview };
  wb_file_media: { args: { threadId: string; path: string; offset: number }; result: MediaChunk };
  wb_file_read: { args: { threadId: string; path: string }; result: FileText };
  /**
   * The lines of the session's files (those `wb_files` lists) that hold `query`, in any case unless `caseSensitive`.
   * Links, binary files and files over 1 MiB are skipped. It stops at 500 lines, 64 MiB read or 10 s (`truncated`).
   */
  wb_file_search: { args: { threadId: string; query: string; caseSensitive?: boolean }; result: FileSearchResult };
  /** The open pull requests of the session's (or project's) repository for the composer's "#" picker, kept 30 s. */
  wb_mention_prs: { args: { threadId: string } | { projectId: string }; result: PullRequestInfo[] };
  /** The session's pull request page. Fails with a `PullError`, as do the four below. */
  /** `url`: one of the session's pull requests (the page's picker); absent, the one its card shows. */
  wb_pull_page: { args: { threadId: string; url?: string }; result: PullPage };
  /** Replies to a review thread. */
  wb_pull_reply: { args: { threadId: string; url?: string; reviewThread: string; body: string }; result: null };
  /** Resolves a review thread, or reopens it. */
  wb_pull_resolve: { args: { threadId: string; url?: string; reviewThread: string; resolved: boolean }; result: null };
  /** Merges `head` with `method` as `action`; refused when it is blocked, the head moved, or the method is not allowed. */
  wb_pull_merge: { args: { threadId: string; url?: string; head: string; method: string; action: MergeAction }; result: null };
  wb_pull_auto_off: { args: { threadId: string; url?: string }; result: null };
  /**
   * The organization's sessions whose prompts or answers hold every word of `query` (any case, part of a word; words
   * under 3 letters are skipped), most recently active first, at most 20. Tool output, thinking and images are not searched.
   */
  wb_search: { args: { query: string }; result: SearchHit[] };
  /**
   * Sends a message. Starts (or resumes) the agent when none is running. While a turn runs, the message joins it at
   * the agent's next step: a `user` event, then `queueSteer`, then `read` once the agent took it in. With `queued`, it
   * waits for the turn instead, editable and cancellable, and its own turn starts (`turnStart`) when the one before
   * ends. If the agent stops before reaching it, a `notice` says so and the message has to be sent again.
   */
  wb_send: { args: { threadId: string; text: string; images: WbImage[]; files?: SentFile[]; queued?: boolean }; result: null };
  /**
   * One piece (about 4 MB, `CHUNK_BYTES`) of a file attached in pieces. Pieces go in order, the first at `offset` 0. They are kept
   * in a temporary file; when all `size` bytes are there (and match `sha256`, the whole file's hash in hex, when given) the
   * file is complete and a message names it as `{ name, upload: uploadId }`. A wrong or out-of-order piece deletes the
   * upload. An upload nobody sends or cancels is deleted after a day. `threadId` is the session the file is for (a new
   * session's is the one its `requestId` makes): only a message to it takes the file, and an upload without one is
   * accepted (it is kept until the one-day sweep) but a message can never take it. `projectId` only says which machine gets the call.
   */
  wb_attachment_chunk: {
    args: { uploadId: string; size: number; offset: number; data: string; sha256?: string; threadId?: string; projectId?: string };
    result: { received: number; done: boolean };
  };
  /** Deletes an upload, begun or finished (nothing there is fine). */
  wb_attachment_cancel: { args: { uploadId: string; threadId?: string; projectId?: string }; result: null };
  /** `suggestion` indexes the approval's `suggestions`; one out of range is ignored (nothing is remembered). */
  wb_project_orchestration_set: { args: { projectId: string; on: boolean }; result: Project };
  wb_session_start_overrides: { args: { clear: boolean }; result: number };
  wb_session_start_preference: { args: { projectId: string }; result: SessionStartPreference };
  wb_session_start_preference_set: { args: { projectId: string; preference: SessionStartPreference }; result: SessionStartPreference };
  wb_session_start_answer: { args: { threadId: string; requestId: string; choice: "new" | "here"; remember: boolean }; result: null };
  wb_approve: {
    /** Claude uses question text and joined labels. Codex uses question IDs and answer arrays. */
    args: { threadId: string; requestId: string; decision: Decision; message: string | null; suggestion: number | null; answers: Record<string, string | string[]> | null };
    result: null;
  };
  /**
   * A message still waiting behind the running turn (`seq`: its `user` event): take it back (`queueCancel`), rewrite
   * it (`queueEdit`, images kept), or send it into the running turn now (`queueSteer`; the agent reads it at its next
   * step, or as its next turn when the turn has no step left). Each fails once the agent has the message.
   */
  wb_queue_cancel: { args: { threadId: string; seq: number }; result: null };
  wb_queue_edit: { args: { threadId: string; seq: number; text: string }; result: null };
  /** Move the waiting message `seq` just before the waiting message `before`, or last when `before` is null. */
  wb_queue_move: { args: { threadId: string; seq: number; before: number | null }; result: null };
  wb_queue_steer: { args: { threadId: string; seq: number }; result: null };
  /** Stops the current turn; the agent keeps the conversation. */
  wb_interrupt: { args: { threadId: string }; result: null };
  /**
   * Stops one sub-agent or background command, when `ThreadSummary.capabilities.stop` lists its kind. Claude Code
   * also stops its foreground sub-agents and commands and lets its background sub-agents run on; Codex ends a child's
   * turn, and its command under way runs on. Rejects with the reason when no agent runs, the task is not running, or
   * the agent refuses (then the log gets a notice too). The task's own `stopped` row confirms the stop.
   */
  wb_task_stop: { args: { threadId: string; taskId: string }; result: null };
  /**
   * Moves a running foreground command (its tool call) to the background, when `capabilities.background` lists
   * "shell". `backgrounded: false`: the agent did not move it. Rejects with the reason when the command is not running
   * in the foreground (a quick one the agent never reported) or the agent refuses (then the log gets a notice too).
   */
  wb_task_background: { args: { threadId: string; toolUseId: string }; result: { backgrounded: boolean } };
  /** A background command's latest output (at most its last 8 KiB), read now and never stored, when `capabilities.output` lists its kind. */
  wb_connection: { args: { threadId: string }; result: SessionConnection };
  wb_compact: { args: { threadId: string; instructions?: string }; result: null };
  wb_task_output: { args: { threadId: string; taskId: string }; result: TaskOutput };
  /**
   * Ends the agent process (the session stays and resumes on the next message). `restart`: the palette's "Restart the
   * agent", which also puts a notice in the log; the next message's new process reads MCP servers and skills again.
   */
  wb_stop: { args: { threadId: string; restart?: boolean }; result: null };
  /** Stop the old agent and keep this thread and its files while preparing an approved provider handoff. */
  wb_thread_switch_agent: {
    args: {
      threadId: string;
      expectedAgent: Agent;
      agent: Agent;
      model: string | null;
      effort: Effort | null;
      via: Via;
      account: string | null;
    };
    result: ThreadSummary;
  };
  /**
   * A permission mode, model or effort change on a running agent is saved once Claude Code accepts it. Until then the
   * answer, and every `wb:thread` after it, carries it in `pending`.
   */
  wb_thread_update: {
    args: {
      threadId: string;
      title?: string;
      /** A model id, or "" for the organization's default (a JSON null would read as "leave it" on the Rust side). */
      model?: string;
      permissionMode?: PermissionMode;
      /** An effort level, or "" for the model's default (as `model`). */
      effort?: Effort | "";
      archived?: boolean;
      /** Whether its agent may start linked sessions. */
      orchestration?: boolean;
      /** Pinning alone leaves the session's last-active time as it was. */
      pinned?: boolean;
      /** "Continue at reset" on a session its plan's limit stopped (`limited`), or off again. Refused without a reset time. */
      continueAtReset?: boolean;
      /** Snoozed until this time (ms); 0 wakes it (a JSON null would read as "leave it"). Not activity, as a pin. */
      snoozedUntil?: number;
      /** "Keep in list": true brings a settled session back and keeps a merge from settling it. */
      keepListed?: boolean;
      /** Settles the session by hand (true), as a merge does, or brings a settled one back (false). Not activity. */
      settled?: boolean;
      /** The session's unique name, or "" to clear it. Refused when invalid or another session's. Claude Code takes it at its next start. */
      name?: string;
    };
    result: ThreadSummary;
  };
  /**
   * Deletes the session and its messages for good. Stops its agent first; removes the worktree and branch the app made
   * for it, and its change snapshots. Fails, deleting nothing, when the worktree has uncommitted changes or unpushed
   * commits. The project's own folder is never touched. Its side chats are deleted with it. Once its agent is stopped,
   * files that won't go (Windows keeps a folder a program holds open) don't keep the session: the result says what
   * stayed, null when nothing did.
   */
  wb_thread_delete: { args: { threadId: string }; result: string | null };
  /**
   * Forks the session: a new one that goes on from the turn that ended at event `turn` (one of `wb_changed_turns`;
   * null: the latest), in a worktree of its own holding the files as that turn left them. The conversation up to there
   * is copied into its log. Its agent starts with its first message. Needs a git repository and worktrees.
   */
  wb_thread_fork: {
    args: { threadId: string; turn: number | null; subscription?: { agent: "claude" | "codex"; account: string | null } };
    result: ThreadSummary;
  };
  /**
   * Sends a fork's result back to its source (`sourceThreadId`): a message with a link to the fork, its latest answer,
   * and where its changes are, queued behind the source's running turn as the person's message (it never interrupts
   * that turn). Fails when the session is not a fork, its source is gone, or it has no answer of its own yet.
   */
  wb_send_back: { args: { threadId: string }; result: null };
  /**
   * Opens a side chat on the session (a core whose features list "side-chats"): a fork of its conversation in its own
   * folder, with no worktree, flagged `sideChat`. It goes on from the session's own conversation while the session is
   * idle, else it is told what was said up to the last finished turn. It runs its first turn in Ask first or lower,
   * keeps no checkpoints, and its agent starts with its first message. Its "Bring back" is `wb_send_back`.
   */
  wb_side_chat: { args: { threadId: string }; result: ThreadSummary };
  /** Closes a side chat: deleted (true) when the person never wrote in it, kept (false) otherwise. */
  wb_side_chat_close: { args: { threadId: string }; result: boolean };
}

/** Events the core emits to every window, for the signed-in organization's sessions only. */
export interface Events {
  "wb:event": StoredEvent;
  "wb:delta": Delta;
  "wb:thread": ThreadSummary;
  "wb:projects": Project[];
  /** A session's pull request changed. This computer's sessions only. */
  "wb:pull": PullWatch;
  /** A worktree cleanup ran. */
  "wb:worktree-cleanup": LastCleanup;
  /** The open secret requests, all of them, after each change. */
  "wb:secrets": SecretRequest[];
  "wb:browser-requests": BrowserRequest[];
}

/** A mode's name and what it does; `agents`: where an agent does it differently (Codex's Auto), what it does there. */
export const PERMISSION_MODES: { mode: PermissionMode; label: string; hint: string; agents?: Partial<Record<Agent, string>> }[] = [
  { mode: "default", label: "Ask first", hint: "Asks before editing files or running commands" },
  {
    mode: "auto",
    label: "Auto",
    hint: "A reviewer model allows what it judges safe and asks you about the rest",
    // Codex's auto review answers every escalation itself (codex.rs `reviewer`): nobody is asked.
    agents: { codex: "Works in the folder. A reviewer model approves or denies network access and going outside it, without asking you" },
  },
  { mode: "acceptEdits", label: "Auto-accept edits", hint: "Edits files without asking, asks before commands" },
  { mode: "plan", label: "Plan only", hint: "Reads and plans, changes nothing" },
  { mode: "bypassPermissions", label: "Full access", hint: "Never asks. Use in a sandbox or a worktree you trust" },
];

/** What `mode` does with `agent` (without one: for any agent). */
export function modeHint(entry: (typeof PERMISSION_MODES)[number], agent?: Agent): string {
  return (agent && entry.agents?.[agent]) ?? entry.hint;
}

/** Reasoning efforts (null: the model's default). Filter these by the agent and selected model. */
export const EFFORTS: { effort: Effort | null; label: string }[] = [
  { effort: null, label: "Default" },
  { effort: "none", label: "None" },
  { effort: "minimal", label: "Minimal" },
  { effort: "low", label: "Low" },
  { effort: "medium", label: "Medium" },
  { effort: "high", label: "High" },
  { effort: "xhigh", label: "XHigh" },
  { effort: "max", label: "Max" },
  { effort: "ultra", label: "Ultra" },
];
