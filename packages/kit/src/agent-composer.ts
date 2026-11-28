/**
 * The Assistant's composer (brand v5 A1, A22, A23, A24·c), for every app: the Auto chip's model, what `@`
 * picks as context, and what `/` runs as a skill. The agent API serves them (`agents/slack/src/api/model.ts` and
 * `context.ts`); this module holds the wire types, their strict readers, the skills and the words.
 *
 * - `GET /agent/model` reads the person's saved model and the chat models their policy allows (Gateway's
 *   `/v1/models`). `PUT /agent/model` with `{ "model": "auto" | "fast" | "best" | "<model id>" }` saves it, as
 *   `!model`, `!fast` and `!best` pick for one question and `/nexus model` saves in Slack.
 * - `GET /agent/context` lists what `@` may pick: the person's connected accounts, errands, projects, people and
 *   other notes from memory, and recent conversations. Each part that fails carries its error (rule 8).
 * - An ask carries the picks as `context: [{ kind, id }]`, at most `AGENT_MAX_CONTEXT`, and a skill as `skill: "<id>"`.
 *   The agent API reads each pick from the caller's own data, so a pick never reaches data the person cannot see.
 *
 * The skills map to what the Assistant does already. Nexus Skills (`/skills`) are SKILL.md prompts for sessions and
 * chat; the Assistant cannot load one yet, so the menu lists none of them.
 */

export const MODEL_PATH = "/agent/model";
export const CONTEXT_PATH = "/agent/context";

/** The composer's placeholder, as A24·c writes it. */
export const COMPOSER_PLACEHOLDER = "Hand off anything, / for skills, @ for context";

/** One chat model the person's policy allows. `maker`: the catalog's `owned_by`. Pricing: USD per million tokens. */
export interface AgentModelOption {
  id: string;
  name: string;
  maker: string;
  pricing?: { input: number; output: number };
}

/** `GET /agent/model`'s answer: the saved model (`auto`, `fast`, `best` or a model id) and the models to pick from. */
export interface AgentModelView {
  model: string;
  models: AgentModelOption[];
  /** The `PUT` changed it. */
  saved?: true;
  /** The models list could not be read; the saved model still answers. */
  modelsError?: string;
}

/** The picks every person has, before the models their policy allows. */
export const MODEL_PICKS: readonly { id: "auto" | "fast" | "best"; label: string; hint: string }[] = [
  { id: "auto", label: "Auto", hint: "Nexus picks the model for each question" },
  { id: "fast", label: "Fast", hint: "The quickest model your policy allows" },
  { id: "best", label: "Best", hint: "The strongest model your policy allows" },
];

/** A model id as the agent API takes it. */
export const MODEL_ID = /^[A-Za-z0-9._:/-]{1,128}$/;

const isPrice = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0;

function modelOptionOf(item: unknown): AgentModelOption | undefined {
  if (!item || typeof item !== "object") return undefined;
  const { id, name, maker, pricing } = item as Record<string, unknown>;
  if (typeof id !== "string" || !MODEL_ID.test(id)) return undefined;
  const option: AgentModelOption = { id, name: typeof name === "string" && name ? name : id, maker: typeof maker === "string" && maker ? maker : (id.split("/")[0] ?? "other") };
  if (pricing && typeof pricing === "object") {
    const { input, output } = pricing as Record<string, unknown>;
    if (isPrice(input) && isPrice(output)) option.pricing = { input, output };
  }
  return option;
}

/** Reads `GET` or `PUT /agent/model`'s body strictly; undefined when it is not one. A bad model row is left out. */
export function modelViewOf(body: unknown): AgentModelView | undefined {
  if (!body || typeof body !== "object") return undefined;
  const { model, models, saved, modelsError } = body as Record<string, unknown>;
  if (typeof model !== "string" || !MODEL_ID.test(model) || !Array.isArray(models)) return undefined;
  return {
    model,
    models: models.flatMap((item) => modelOptionOf(item) ?? []),
    ...(saved === true ? { saved: true as const } : {}),
    ...(typeof modelsError === "string" && modelsError ? { modelsError } : {}),
  };
}

/** The chip's word for a saved model: Auto, Fast, Best, or the model's name when the list has it, else its id. */
export function modelChipLabel(model: string, models: readonly AgentModelOption[] = []): string {
  const pick = MODEL_PICKS.find((one) => one.id === model);
  if (pick) return pick.label;
  return models.find((one) => one.id === model)?.name ?? model.split("/").at(-1) ?? model;
}

/** `PUT /agent/model`'s body. */
export function modelBody(model: string): string {
  return JSON.stringify({ model });
}

/** What `@` may pick, in the menu's order. */
export const AGENT_CONTEXT_KINDS = ["connector", "errand", "project", "person", "note", "conversation"] as const;
export type AgentContextKind = (typeof AGENT_CONTEXT_KINDS)[number];

/** Each kind's group heading in the menu. */
export const CONTEXT_GROUPS: Record<AgentContextKind, string> = {
  connector: "Accounts",
  errand: "Errands",
  project: "Projects",
  person: "People",
  note: "Notes",
  conversation: "Conversations",
};

/** One pick as an ask carries it. */
export interface AgentContextRef {
  kind: AgentContextKind;
  id: string;
}

/** One row of the `@` menu: the pick, its label and a short detail. */
export interface AgentContextItem extends AgentContextRef {
  label: string;
  detail?: string;
}

/** `GET /agent/context`'s answer. `errors`: a part that could not be read, by kind (rule 8). */
export interface AgentContextView {
  items: AgentContextItem[];
  errors?: Partial<Record<AgentContextKind, string>>;
}

/** The most picks one ask carries. */
export const AGENT_MAX_CONTEXT = 8;
/** A pick's id: printable, 1 to 128 characters. A project's id is its name. */
export const CONTEXT_ID_CHARS = 128;
/** A menu label is cut to this many characters. */
export const CONTEXT_LABEL_CHARS = 80;

const CONTROL = /[\u0000-\u001F\u007F]/;
const isKind = (value: unknown): value is AgentContextKind => AGENT_CONTEXT_KINDS.includes(value as AgentContextKind);
const isId = (value: unknown): value is string => typeof value === "string" && value.trim().length > 0 && value.length <= CONTEXT_ID_CHARS && !CONTROL.test(value);

/** A label cut to `CONTEXT_LABEL_CHARS`, on one line. */
export function contextLabel(text: string): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > CONTEXT_LABEL_CHARS ? `${line.slice(0, CONTEXT_LABEL_CHARS - 1).trimEnd()}…` : line;
}

/** Reads `GET /agent/context`'s body; undefined when it is not one. A bad row is left out. */
export function contextViewOf(body: unknown): AgentContextView | undefined {
  if (!body || typeof body !== "object") return undefined;
  const { items, errors } = body as Record<string, unknown>;
  if (!Array.isArray(items)) return undefined;
  const view: AgentContextView = {
    items: items.flatMap((item): AgentContextItem[] => {
      if (!item || typeof item !== "object") return [];
      const { kind, id, label, detail } = item as Record<string, unknown>;
      if (!isKind(kind) || !isId(id) || typeof label !== "string" || !label.trim()) return [];
      return [{ kind, id, label: contextLabel(label), ...(typeof detail === "string" && detail.trim() ? { detail: contextLabel(detail) } : {}) }];
    }),
  };
  if (errors && typeof errors === "object") {
    const read = Object.entries(errors as Record<string, unknown>).filter((entry): entry is [AgentContextKind, string] => isKind(entry[0]) && typeof entry[1] === "string");
    if (read.length) view.errors = Object.fromEntries(read);
  }
  return view;
}

/**
 * An ask's `context`, read strictly: an array of at most `AGENT_MAX_CONTEXT` objects with exactly `kind` and `id`.
 * The same pick twice counts once. Absent is none. Anything else is an error the API answers 400 with.
 */
export function contextRefsOf(value: unknown): AgentContextRef[] | { error: string } {
  if (value === undefined) return [];
  if (!Array.isArray(value)) return { error: "context is a list of { kind, id }." };
  if (value.length > AGENT_MAX_CONTEXT) return { error: `context holds ${AGENT_MAX_CONTEXT} picks at most.` };
  const refs: AgentContextRef[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object" || Array.isArray(item)) return { error: "Each context pick is { kind, id }." };
    const keys = Object.keys(item);
    const { kind, id } = item as Record<string, unknown>;
    if (keys.some((key) => key !== "kind" && key !== "id")) return { error: "A context pick has only kind and id." };
    if (!isKind(kind)) return { error: `A context pick's kind is one of ${AGENT_CONTEXT_KINDS.join(", ")}.` };
    if (!isId(id)) return { error: `A context pick's id is 1 to ${CONTEXT_ID_CHARS} printable characters.` };
    if (!refs.some((ref) => ref.kind === kind && ref.id === id)) refs.push({ kind, id });
  }
  return refs;
}

/** One built-in skill: what `/` lists, and the message the agent already understands. */
export interface AgentSkill {
  id: string;
  label: string;
  hint: string;
  /** The skill reads the words after it. Without, they are left out. */
  takesText: boolean;
}

/**
 * The Assistant's skills, each mapped to a command it already obeys: `brief now`, `errand:`, `watch`, `remember that`,
 * `forget that`, `!web`, and a hand-off to a cloud session through its `start_session` tool.
 */
export const ASSISTANT_SKILLS: readonly AgentSkill[] = [
  { id: "brief", label: "brief", hint: "Today's brief", takesText: false },
  { id: "errand", label: "errand", hint: "Start an errand", takesText: true },
  { id: "watch", label: "watch", hint: "Tell me when something changes", takesText: true },
  { id: "remember", label: "remember", hint: "Save a note", takesText: true },
  { id: "forget", label: "forget", hint: "Forget the last note", takesText: false },
  { id: "web", label: "web", hint: "Search the web", takesText: true },
  { id: "handoff", label: "handoff", hint: "Hand off to a session", takesText: true },
];

const SKILL_ID = /^[a-z]{1,32}$/;

/** A skill id as an ask carries it, or undefined when it names none. */
export function skillOf(value: unknown): AgentSkill | undefined {
  return typeof value === "string" && SKILL_ID.test(value) ? ASSISTANT_SKILLS.find((skill) => skill.id === value) : undefined;
}

/** The message a skill sends, with the person's words; undefined when the skill needs words and has none. */
export function skillMessage(skill: AgentSkill, text: string): string | undefined {
  const words = text.trim();
  if (skill.takesText && !words) return undefined;
  switch (skill.id) {
    case "brief":
      return "brief now";
    case "errand":
      return `errand: ${words}`;
    case "watch":
      return `watch ${words}`;
    case "remember":
      return `remember that ${words}`;
    case "forget":
      return "forget that";
    case "web":
      return `!web ${words}`;
    case "handoff":
      return `Hand this off to a cloud session with start_session: ${words}`;
    default:
      return undefined;
  }
}

/** The menu a composer opens: `/` at the start of the message, `@` at the start or after a space. */
export interface ComposerTrigger {
  kind: "skill" | "context";
  /** What was typed after the sign, to filter the menu. */
  query: string;
  /** Where the sign is in the text. */
  start: number;
}

/**
 * The menu the text before the caret opens, or undefined. `/` counts only as the message's first character, and `@`
 * only at the start or after white space; either ends at white space, so a finished word closes the menu.
 */
export function composerTrigger(text: string, caret: number): ComposerTrigger | undefined {
  const before = text.slice(0, caret);
  const at = /(^|\s)@([^\s@]{0,40})$/.exec(before);
  if (at) return { kind: "context", query: at[2]!, start: before.length - at[2]!.length - 1 };
  const slash = /^\/([a-z]{0,32})$/i.exec(before);
  if (slash) return { kind: "skill", query: slash[1]!, start: 0 };
  return undefined;
}

/** The rows whose label (or detail, for context) starts a word with `query`, any case; all of them for no query. */
export function filterContext(items: readonly AgentContextItem[], query: string): AgentContextItem[] {
  const q = query.toLowerCase();
  if (!q) return [...items];
  const words = (text: string) => text.toLowerCase().split(/[\s/._-]+/);
  return items.filter((item) => words(item.label).some((word) => word.startsWith(q)) || item.label.toLowerCase().startsWith(q) || (item.detail ? words(item.detail).some((word) => word.startsWith(q)) : false));
}

export function filterSkills(query: string): AgentSkill[] {
  const q = query.toLowerCase();
  return ASSISTANT_SKILLS.filter((skill) => skill.id.startsWith(q));
}

/** ↑ and ↓ in a menu of `size` rows: it wraps. */
export function moveActive(active: number, size: number, step: 1 | -1): number {
  if (size <= 0) return 0;
  return (active + step + size) % size;
}
