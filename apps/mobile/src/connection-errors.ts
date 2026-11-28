import { CoreError } from "@nexus/kit";

/** The phone is not paired, or not yet approved: only Connect phone fixes it. */
export class PairingError extends Error {
  /** `gateway`: Gateway vouches for the person's devices, so the fix is no scan or code. */
  constructor(message: string, readonly trust: "approvals" | "gateway" = "approvals") {
    super(message);
  }
}

/**
 * Why a phone-control screen cannot go on, so its title names the fix. `pairing`: connect this phone. `network`: Gateway
 * did not answer, and the screen tries again on its own. `failed`: a definite refusal or another error.
 */
export type FailureCause = "pairing" | "network" | "failed";

/** A failure that a later attempt may not repeat: no answer, a timeout, a 5xx, 408 or 429. */
export function transientError(error: unknown): boolean {
  if (error instanceof CoreError) return error.status >= 500 || error.status === 408 || error.status === 429;
  // fetch rejects with a TypeError when the network fails; a read deadline aborts with TimeoutError.
  if (error instanceof TypeError) return true;
  // A DOMException is not always an Error subclass, so read its name directly.
  const name = typeof error === "object" && error !== null && "name" in error ? error.name : undefined;
  return name === "TimeoutError" || name === "AbortError";
}

export function failureCause(error: unknown): FailureCause {
  if (error instanceof PairingError) return "pairing";
  return transientError(error) ? "network" : "failed";
}

export function failureTitle(cause: FailureCause): string {
  switch (cause) {
    case "pairing": return "Connect this phone";
    case "network": return "Can't reach Nexus right now";
    case "failed": return "Couldn't open this computer";
  }
}
