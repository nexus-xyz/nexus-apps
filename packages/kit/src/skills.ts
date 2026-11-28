/**
 * Skills (Rust gateway only): a person's own SKILL.md files in versions that never change. The wire types
 * of `/v1/skills*`, of the promotions: a request to share one version with the organization or a team,
 * which an approver who is not the requester answers (`/v1/skill-promotions*`), and of the library: the
 * approved skills a person may load (`/v1/skill-library`).
 */

export type SkillCreatedVia = "web" | "cli" | "api";

/** `SkillSummary`: a skill, as its person lists it. */
export interface SkillSummary {
  /** `skl_…`. */
  id: string;
  /** The latest version's name: 1 to 64 characters of `a-z`, `0-9` and single hyphens. */
  name: string;
  /** The latest version's description: 1 to 1024 characters. */
  description: string;
  latestVersion: number;
  createdAt: string;
  /** When the latest version was written. For a library skill: when its latest approved version was approved. */
  updatedAt: string;
}

/** `SkillVersion`: one version, which never changes. */
export interface SkillVersion {
  skillId: string;
  version: number;
  /** This version's name and description. */
  name: string;
  description: string;
  /** SHA-256 of the SKILL.md, lowercase hex. */
  contentSha256: string;
  /** Size in bytes (UTF-8). */
  bytes: number;
  /** How the version was written. For a library version: how its author wrote the version that was approved. */
  createdVia: SkillCreatedVia;
  /** When the version was written. For a library version: when it was approved. */
  createdAt: string;
}

/** `GET /v1/skills/{id}/versions/{version}`: one version and its SKILL.md. */
export interface SkillVersionDetail extends SkillVersion {
  /** The SKILL.md, exactly as it was written. */
  content: string;
}

/** `GET /v1/skills/{id}`: the skill and its versions, newest first. */
export interface SkillDetail extends SkillSummary {
  versions: SkillVersion[];
}

/** `GET /v1/skills`: most recently written first. A person keeps at most 200, so there is no cursor. */
export interface SkillList {
  skills: SkillSummary[];
}

/** `POST /v1/skills` and `/versions`. */
export interface SkillWritten {
  skill: SkillSummary;
  version: SkillVersion;
}

/** `DELETE /v1/skills/{id}`. */
export interface SkillDeleted {
  deleted: boolean;
  id: string;
  /** How many versions went with it. */
  versions: number;
}

export interface CreateSkillRequest {
  /** The SKILL.md: YAML front matter with a `name` and a `description`, then the instructions. At most 1 MiB. */
  content: string;
}

export interface WriteSkillVersionRequest {
  content: string;
  /** The version this was built on: a 409 `skill_version_conflict` when it is no longer the latest. */
  baseVersion?: number;
}

/** Where a skill was loaded or delivered. */
export type SkillLoadSurface = "web_chat" | "nexus_claude" | "nexus_codex";

/** `loaded` in web chat, `delivered` by a launcher. A record never says the skill ran. */
export type SkillLoadKind = "loaded" | "delivered";

/** `SkillLoad`: which skill and version a session loaded or was delivered. It holds no skill text. */
export interface SkillLoad {
  /** `sld_…`. */
  id: string;
  skillId: string;
  /** The name of the version that was served; the record keeps it after the skill is deleted. */
  skillName: string;
  version: number;
  /** The SHA-256 of the SKILL.md that was served. With the skill and the version it names the bytes. */
  contentSha256: string;
  surface: SkillLoadSurface;
  kind: SkillLoadKind;
  /** The chat session, or the agent session the launcher opened. */
  sessionId: string;
  createdAt: string;
}

/** `POST /v1/skill-loads` (the records just written) and `GET /v1/skill-loads` (one session's, in the order written). */
export interface SkillLoadList {
  loads: SkillLoad[];
}

/** `POST /v1/skill-loads`: a launcher reports the skills it delivered to the agent session it opened. */
export interface RecordSkillLoadsRequest {
  surface: "nexus_claude" | "nexus_codex";
  sessionId: string;
  /**
   * 1 to 900 entries, a skill once: your own skills and the library skills you may load. A library skill's version must be
   * approved and not withdrawn, not necessarily the latest. A call is all or none, so send a larger delivery in several.
   */
  skills: { skillId: string; version: number }[];
}

/** Where a promotion goes: the organization's library, or a team's (and the teams under it). */
export type SkillPromotionScope = "org" | "team";

/** `held` until an approver answers; `approved`, `refused` and `withdrawn` are where it ends (an approved one may be withdrawn). */
export type SkillPromotionState = "held" | "approved" | "refused" | "withdrawn";

/** What the approver's client says they opened before they decided. Reported, not proved. */
export interface SkillPromotionReviewed {
  content: boolean;
  diff: boolean;
}

/**
 * `SkillPromotion`: one request to promote a version of a person's skill, and how it ended. It holds no skill text.
 * A field that is not set yet is left out.
 */
export interface SkillPromotion {
  /** `spr_…`. */
  id: string;
  state: SkillPromotionState;
  scope: SkillPromotionScope;
  /** The target team's id, for scope `team`. */
  teamId?: string;
  /** The snapshot's name and description: what the version said when it was requested. */
  name: string;
  description: string;
  /** The snapshot's SHA-256, lowercase hex. */
  contentSha256: string;
  /** The snapshot's size in bytes (UTF-8). */
  bytes: number;
  /** The author's skill (`skl_…`) and version the snapshot was taken from, and how and when that version was written. */
  sourceSkillId: string;
  sourceVersion: number;
  sourceWrittenVia: SkillCreatedVia;
  sourceWrittenAt: string;
  /** The requester's user id, how the request was made, and when. */
  requestedBy: string;
  requestedVia: SkillCreatedVia;
  requestedAt: string;
  note?: string;
  /** Set once an approver has approved or refused it. */
  decidedBy?: string;
  decidedAt?: string;
  decisionReason?: string;
  /** Whether the approver was the requester. Always false: nobody approves their own request. */
  selfApproved: boolean;
  reviewed?: SkillPromotionReviewed;
  /** Set once approved: the library skill (`skl_…`) and the version this promotion is of it. */
  librarySkillId?: string;
  libraryVersion?: number;
  /** Set once withdrawn. */
  withdrawnBy?: string;
  withdrawnAt?: string;
  withdrawnReason?: string;
}

/** `GET /v1/skill-promotions`: newest request first. It holds no skill text. */
export interface SkillPromotionList {
  promotions: SkillPromotion[];
  /** Set while more follow: send it as `cursor` for the next page. */
  nextCursor?: string;
}

/**
 * The last approved version of the same library skill, with its SKILL.md, for a diff. It holds what someone who loads the
 * library reads: the version, its name, description, hash, approval time and text.
 */
export interface SkillPromotionPrevious {
  /** The earlier promotion's id (`spr_…`). */
  id: string;
  name: string;
  description: string;
  /** The SHA-256 of `content`, lowercase hex. */
  contentSha256: string;
  /** The version it is of its library skill. */
  libraryVersion: number;
  /** When it was approved. */
  decidedAt: string;
  /** The SKILL.md, exactly as that version held it. */
  content: string;
  /** The whole record: who asked for the version, who approved it and why. Left out unless you approve it. */
  promotion?: SkillPromotion;
}

/** `GET /v1/skill-promotions/{id}`: the promotion and its snapshot. */
export interface SkillPromotionDetail extends SkillPromotion {
  /** The SKILL.md exactly as the version held it when it was requested. */
  content: string;
  /** The last approved version of the same library skill before this one; null when there is none or the caller does not load that library. */
  previous: SkillPromotionPrevious | null;
}

/** `POST /v1/skill-promotions/{id}/withdraw`: the promotion the call named, and every promotion it withdrew (that one included, oldest first). */
export interface SkillPromotionWithdrawn extends SkillPromotion {
  withdrawnIds: string[];
}

/** `POST /v1/skills/{id}/versions/{version}/promotions`. */
export interface RequestSkillPromotionRequest {
  scope: SkillPromotionScope;
  /** The target team, a slug or an id. With scope `team` only, and you must be in it. */
  team?: string;
  /** Why, for the approvers. At most 500 characters. */
  note?: string;
}

/** `POST /v1/skill-promotions/{id}/decision`. */
export interface DecideSkillPromotionRequest {
  decision: "approved" | "refused";
  /** Required to refuse, so the author knows what to change. At most 500 characters. */
  reason?: string;
  reviewed?: Partial<SkillPromotionReviewed>;
}

/** `POST /v1/skill-promotions/{id}/withdraw`. */
export interface WithdrawSkillPromotionRequest {
  reason?: string;
  /** Withdraw every approved version of the promotion's library skill. Name an approved version. */
  allVersions?: boolean;
}

/** `GET /v1/skill-promotions`: the filters. */
export interface SkillPromotionsQuery {
  state?: SkillPromotionState;
  scope?: SkillPromotionScope;
  /** A team slug or id: only promotions to that team. */
  team?: string;
  /** Only the requests you made. */
  mine?: boolean;
  /** 1 to 200 (default 50). */
  limit?: number;
  cursor?: string;
}

/**
 * The typed errors of the promotion operations (`error.code`), beside the C1 skill codes and the matrix's
 * `role_required`. Branch on the code, not the message. The matrix answers before a promotion is read: a module key gets
 * `module_key`, a service account or an agent link `role_required`, and an operator `operator_scope`. A cursor this API
 * did not return is `invalid_cursor`, as on the other lists.
 */
export type SkillPromotionErrorCode =
  | "invalid_promotion"
  | "invalid_cursor"
  | "person_session_required"
  | "personal_org"
  | "not_in_team"
  | "unknown_team"
  | "promotion_exists"
  | "promotion_limit"
  | "promotion_not_found"
  | "promotion_decided"
  | "self_approval"
  | "requester_left"
  | "team_deleted"
  | "name_taken"
  | "library_limit";

/**
 * `SkillLibraryEntry`: a library skill you may load, at the version a session loads. It holds no skill text: read it with
 * `skillVersion` or `exportSkill`, using `id` and `version`. `skill` and `skillVersion` also take a library skill's id,
 * and list its approved versions only.
 */
export interface SkillLibraryEntry {
  /** The library skill, `skl_…`. */
  id: string;
  name: string;
  description: string;
  /** `org`: the organization's library. `team`: a team's, which also reaches the teams under it. */
  scope: SkillPromotionScope;
  /** The team (`team_…`), for scope `team`. */
  teamId?: string;
  /** The highest approved version that nobody withdrew: the one a session loads. */
  version: number;
  /** When that version was approved. */
  approvedAt: string;
  /**
   * Another skill of this name keeps it from loading, so neither is delivered or loaded. Left out when false. A skill of
   * yours with this name collides too when any library skill of the name has this flag. When none has, your skill has the
   * library's bytes and the library copy serves it. The wire lists carry no SHA-256, so read this, not the names alone.
   */
  duplicateName?: boolean;
}

/** `GET /v1/skill-library`: the organization's skills first, then the teams', all by name (not grouped by team). There is no cursor: a library holds at most 200 organization skills and 50 for each team. */
export interface SkillLibraryList {
  skills: SkillLibraryEntry[];
}

/** The largest SKILL.md, in bytes of UTF-8. */
export const MAX_SKILL_BYTES = 1024 * 1024;
