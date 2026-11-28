/** Signed, encrypted computer operations. Project choices and prompts occur only inside ciphertext. */
export interface ComputerScope {
  gateway: string;
  tenantId: string;
  personId: string;
}

export type ExecutionOrigin =
  | { kind: "computer"; computerId: string; name: string }
  | { kind: "cloud"; machineId: string; name?: string }
  | { kind: "unknown" };

export interface ComputerPresence {
  version: 1;
  scope: ComputerScope;
  computerId: string;
  runnerDeviceId: string;
  name: string;
  generation: number;
  issuedAt: number;
  expiresAt: number;
  enabled: boolean;
  /**
   * As the computer signed it. A newer computer may list a capability this phone does not know: it stays as text,
   * so the signature over the list still verifies, and nothing here acts on it.
   */
  capabilities: ("projects" | "start" | "settings" | "changes" | (string & {}))[];
  signature: string;
}

export interface ComputerList {
  computers: ComputerPresence[];
  serverTime: number;
}

export interface ComputerRequestEnvelope {
  version: 1;
  scope: ComputerScope;
  computerId: string;
  id: string;
  senderDeviceId: string;
  runnerDeviceId: string;
  generation: number;
  issuedAt: number;
  expiresAt: number;
  lockedKey: string;
  sealed: string;
  signature: string;
}

export interface ComputerReplyEnvelope {
  version: 1;
  scope: ComputerScope;
  computerId: string;
  requestId: string;
  requestHash: string;
  runnerDeviceId: string;
  recipientDeviceId: string;
  lockedKey: string;
  sealed: string;
  signature: string;
}

export interface ComputerRequest {
  envelope: ComputerRequestEnvelope;
  requestHash: string;
  state: "queued" | "claimed" | "replied" | "expired";
  reply: ComputerReplyEnvelope | null;
  serverTime: number;
}

/**
 * `request` on `GET /v1/computers/stream`: a computer request's new stored state. Ids and state, never
 * ciphertext: read the receipt with `computerRequest`.
 */
export interface ComputerRequestChanged {
  computerId: string;
  id: string;
  state: ComputerRequest["state"];
}

export interface ComputerRequestClaim {
  runnerDeviceId: string;
  signature: string;
}

export interface ComputerStartSettings {
  agent: string;
  model: string | null;
  via: string;
  permissionMode: string;
  effort: string | null;
  workspace: "folder" | "worktree";
}

export interface ComputerProjectChoice {
  id: string;
  name: string;
  locationHint: string;
  revision: string;
  settings: ComputerStartSettings;
  payer?: ComputerPayer;
}

export type ComputerAgent = "claude" | "codex" | "nexus";
export type ComputerVia = "nexus" | "subscription";
export type ComputerEffort = "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";
export interface ComputerPayer {
  kind: "plan" | "apiKey" | "gateway" | "unknown";
  label: string;
  detail: string | null;
}
export interface ComputerModelChoice { id: string; name: string; efforts: ComputerEffort[] }
export interface ComputerRouteChoices {
  via: ComputerVia;
  payer: "plan" | "apiKey" | "gateway";
  label: string;
  detail: string | null;
  state: "usable" | "unavailable" | "unknown";
  models: ComputerModelChoice[];
  defaultModel: string | null;
}
export interface ComputerAgentChoices { agent: ComputerAgent; name: string; routes: ComputerRouteChoices[] }
export interface ComputerModelSettings { model: string | null; effort: ComputerEffort | null }
export interface ComputerSelection extends ComputerModelSettings {
  via: ComputerVia;
  payer: "plan" | "apiKey" | "gateway";
}
export interface ComputerSessionSettings {
  threadId: string;
  revision: string;
  /** `unknown`: a newer computer's agent that this phone cannot name. Such a session is never editable here. */
  agent: ComputerAgent | "unknown";
  /** `unknown`: a newer computer's route that this phone cannot name. Such a session is never editable here. */
  via: ComputerVia | "unknown";
  model: string | null;
  effort: ComputerEffort | null;
  /** The computer runs a level this phone does not know, read as `effort: null`. An update without an explicit level would replace it. */
  effortUnknown?: true;
  payer: ComputerPayer;
  /** `effortUnknown`: the requested level is one this phone does not know, so `effort` is left out. */
  pending: { model?: string | null; effort?: ComputerEffort | null; effortUnknown?: true } | null;
  models: ComputerModelChoice[];
  editable: boolean;
  reason: string | null;
}

export type ComputerOperation =
  | { kind: "describeProjects" }
  | { kind: "describeBranches"; projectId: string; revision: string }
  | { kind: "describeHandoff"; threadId: string }
  | { kind: "startSession"; projectId: string; revision: string; text: string; selection?: ComputerSelection; baseBranch?: string; handoff?: { sourceThreadId: string; agent: ComputerAgent } }
  | { kind: "describeSession"; threadId: string }
  | { kind: "readSessionChanges"; threadId: string }
  | { kind: "compactSession"; threadId: string }
  | { kind: "updateSession"; threadId: string; revision: string; settings: ComputerModelSettings };

export interface ComputerSessionChanges {
  threadId: string;
  files: string[];
  patch: string;
  truncated: boolean;
}

export type ComputerBranchList = { projectId: string; revision: string; branches: string[] };

export type ComputerHandoffChoices = { threadId: string; project: ComputerProjectChoice; agents: ComputerAgentChoices[] };

export type ComputerResult =
  | ({ kind: "handoffChoices" } & ComputerHandoffChoices)
  | ({ kind: "branches" } & ComputerBranchList)
  | { kind: "projects"; projects: ComputerProjectChoice[]; agents?: ComputerAgentChoices[] }
  | { kind: "created"; threadId: string; origin: ExecutionOrigin; launch: "starting" | "running" | "idle" | "error" | "interrupted" | "unknown" }
  | { kind: "choicesChanged"; projects: ComputerProjectChoice[]; agents?: ComputerAgentChoices[] }
  | { kind: "sessionSettings"; session: ComputerSessionSettings }
  | ({ kind: "sessionChanges" } & ComputerSessionChanges)
  | { kind: "sessionUpdated"; session: ComputerSessionSettings }
  | { kind: "compacted"; threadId: string }
  | { kind: "sessionChanged"; session: ComputerSessionSettings }
  | { kind: "refused"; code: string; message: string }
  | { kind: "uncertain"; threadId: string | null };

/** A QR capability, never a Gateway request body. Its secret is a one-use invitation, not a device key. */
export interface PhoneInvitation {
  version: 1 | 2;
  scope: ComputerScope;
  inviteId: string;
  computerId: string;
  computerName: string;
  inviterDeviceId: string;
  encryptionKey: string;
  signingKey: string;
  expiresAt: number;
  secret: string;
  signature: string;
}

export interface PhonePairingEnvelope {
  version: 1;
  scope: ComputerScope;
  id: string;
  inviteId: string;
  inviterDeviceId: string;
  senderDeviceId: string;
  expiresAt: number;
  lockedKey: string;
  sealed: string;
  signature: string;
}

export type PhonePairingDecision = {
  scope: ComputerScope;
  computerId: string;
  inviteId: string;
  inviterDeviceId: string;
  expiresAt: number;
  signature: string;
} & ({ state: "approved"; requestId: string; senderDeviceId: string } | { state: "cancelled" });

export interface PhonePairingRequest {
  envelope: PhonePairingEnvelope;
  state: "pending" | "finished";
  decision?: PhonePairingDecision;
}
