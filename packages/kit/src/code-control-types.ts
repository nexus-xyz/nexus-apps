/** Encrypted commands for the owner's paired devices. Gateway never receives an action in plaintext. */
export interface CodeControl {
  runnerDeviceId: string | null;
  enabled: boolean;
  /** The runner's lease, in Unix milliseconds. */
  expiresAt: number | null;
  /**
   * Gateway's clock when it answered, in Unix milliseconds. An older Gateway leaves it out. `CoreClient.codeControl`
   * records it with the time the answer arrived, so `controlNow` reads Gateway's clock later (`code-control-clock.ts`).
   */
  serverTime?: number;
}

export interface CodeControlLease {
  runnerDeviceId: string;
  enabled: boolean;
  expiresAt: number;
  signature: string;
}

export type CodeAction =
  | { kind: "send"; text: string }
  | { kind: "stop" }
  | { kind: "approve"; requestId: string; decision: "allow" | "allowSession" | "deny"; message: string | null; answers: Record<string, string | string[]> | null };

export interface CodeCommandEnvelope {
  id: string;
  senderDeviceId: string;
  runnerDeviceId: string;
  expiresAt: number;
  lockedKey: string;
  sealed: string;
  signature: string;
  /**
   * The person lets the command wait for a computer that is asleep, for up to 24 hours. Signed as
   * `nexus-code-command-v2`, so a runner that verifies only v1 never runs it. Absent means false.
   */
  waits?: boolean;
}

/** `cancelled`: its sender took it back before a computer claimed it. */
export type CodeCommandState = "queued" | "claimed" | "completed" | "rejected" | "expired" | "cancelled";

/** The body of `cancelCodeCommand`, signed by the device that sent the command. */
export interface CodeCommandCancel {
  senderDeviceId: string;
  signature: string;
}

export interface CodeCommand extends CodeCommandEnvelope {
  threadId: string;
  state: CodeCommandState;
}

/**
 * `command`: a Code command's new stored state, on the thread's stream and on `GET /v1/computers/stream`,
 * for the thread's person only. Ids and state, never ciphertext: read the receipt with `codeCommand`.
 */
export interface CodeCommandChanged {
  threadId: string;
  id: string;
  runnerDeviceId: string;
  state: CodeCommandState;
}

/** `commands`: first on a thread's stream that carries `command` events. A stream without it carries none. */
export interface CodeCommandsLive {
  threadId: string;
}

export interface CodeCommandClaim {
  runnerDeviceId: string;
  signature: string;
}

export interface CodeCommandResult extends CodeCommandClaim {
  state: "completed" | "rejected";
}
