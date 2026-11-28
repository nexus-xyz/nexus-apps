/**
 * The module contract (03 §5). A product is a suite of capabilities; each capability is its own backend module (one
 * per pod) that serves `GET /manifest` with this shape. Nexus Core, Nexus UI and the `nexus` CLI are driven by it:
 * adding a capability means registering its URL with Core, not changing Core, the CLI or the UI.
 *
 * The Rust twin is `nexus-types::api::modules` (Nexus's Rust services); change both together.
 */

/** The verbs every product shares (03 §5). A module declares the subset it implements today. `status` (GET, and POST
 * with a draft) says what a capability will do and why (Nexus Review). */
export const VERBS = ["connect", "run", "report", "apply", "watch", "status"] as const;
export type Verb = (typeof VERBS)[number];

export function isVerb(value: string): value is Verb {
  return (VERBS as readonly string[]).includes(value);
}

/** One input a verb takes. The UI renders it as a field, the CLI as `--<name> <value>`. */
export interface ManifestInput {
  name: string;
  label: string;
  placeholder?: string;
  required?: boolean;
  /** How Nexus UI draws it; absent is `text`. A kind this version does not know is drawn as text. */
  kind?: InputKind;
  /** Where a `select` input's options come from. Unknown or absent: the input is drawn as text. */
  options_from?: OptionsSource;
  /** The input whose value the options are read for: they wait for it, and a new value there clears this one. */
  depends_on?: string;
  /** The verbs that take it (`["connect"]` for a setting). Absent: every verb. Nexus UI's run form draws only `run`'s. */
  verbs?: Verb[];
}

/** `text`: a text field. `select`: a searchable list (a combobox); a value that is not listed may still be typed. */
export type InputKind = "text" | "select";

/**
 * The lists a `select` input is filled from, each read by Nexus UI from a Gateway route the person may call:
 * - `repositories`: the organization's linked repositories (`GET /v1/github/repositories`), shown as `owner/name`,
 *   submitted as the URL.
 * - `pulls_open`: the open pull requests of the repository in `depends_on` (the github connector's `pulls_open`),
 *   shown as `#<number>`, submitted as the number.
 */
export type OptionsSource = "repositories" | "pulls_open";

/**
 * How Nexus UI renders the capability's tab. Each kind is one generic component in Nexus UI; a new capability that
 * reuses a kind needs no UI code.
 * - `pages`: start a run, follow its status, read the Markdown pages it produced and what it cost (`RunReport`).
 *
 * The MVP has one kind (an internal decision: the router and Wiki). Git tag `mvp-visibility-frozen` keeps the `score` kind Nexus
 * Visibility used.
 */
export type PanelKind = "pages";

/** A JSON Schema object, as used in OpenAPI 3.1. */
export type JsonSchema = Record<string, unknown>;

/** What a verb accepts and returns, merged by Core into `GET /v1/openapi.json`. */
export interface ManifestOperation {
  /** The permission-matrix action this verb needs; the Rust gateway refuses a manifest without one. */
  permission: string;
  summary: string;
  request?: JsonSchema;
  response?: JsonSchema;
}

export interface ModuleManifest {
  /** The product this capability belongs to. Several modules may declare the same product. */
  product: {
    /** The CLI noun and first API path segment: `nexus <slug> run`, `/v1/<slug>/…`. */
    slug: string;
    /** `Nexus <Word>` (03 §2). */
    name: string;
  };
  capability: {
    /** Second API path segment: `/v1/<product>/<capability>/run`. */
    slug: string;
    name: string;
    description: string;
  };
  verbs: Verb[];
  /** Inputs of `run` and `connect`. */
  inputs: ManifestInput[];
  panel: { kind: PanelKind };
  /** Schemas per verb, for the merged OpenAPI document. `report` is served as `GET report` and `GET report/{id}`. */
  operations: Partial<Record<Verb, ManifestOperation>>;
  /**
   * The GitHub events the Rust gateway forwards to this module at `POST events` (`ModuleEvent` in
   * `nexus-types::api::modules`). Absent: none. `push` is not one: it stays Nexus Wiki's own path.
   */
  events?: GithubEvent[];
  /**
   * `GET report` answers one page at a time: `limit`, `after` and `status` apply, and a page with more after it names
   * its `next_cursor` (`RunsPage`). Absent: every report is the whole list.
   */
  paged_reports?: boolean;
  /**
   * The module keeps data per linked repository: when one is unlinked, the Rust gateway sends it `POST forget` with
   * `{ "repo": <url> }`, and it deletes that data. Absent: it gets no `forget`.
   */
  forgets_repositories?: boolean;
  /**
   * The steps a job of `run` is made of, in order, for the Runtime to run one by one as Temporal activities. Absent:
   * the module runs its jobs itself.
   */
  steps?: ManifestStep[];
}

/**
 * One step of a job: `POST <path>` on the module, with the job's id in place of `{id}` and an `Idempotency-Key` derived
 * from the job and the step, so a retry returns the first result (Runtime A3). A step that answered `202` held on a
 * PO1 record is called again, once the record is confirmed, under that key plus `:{approvalId}` (A6.2): a fresh key,
 * so it runs and uses the record. Holding again on the same record there ends the job `approval_invalid`.
 */
export interface ManifestStep {
  name: string;
  summary: string;
  path: string;
  /** The step's time limit: on Temporal, the activity's `start_to_close`. The module bounds the step by it too. */
  timeoutSeconds: number;
  /** For a step run once per item: the most at once. An upper bound; the Runtime worker's activity cap decides. */
  maxParallel?: number;
}

/** A GitHub webhook event a module may subscribe to. */
export type GithubEvent = "pull_request" | "issue_comment" | "pull_request_review_comment";
