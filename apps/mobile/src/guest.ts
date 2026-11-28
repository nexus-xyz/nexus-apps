import { CoreError, type createCoreClient, type GuestChatMessage, type GuestChatStatus } from "@nexus/kit";

/**
 * Guest mode on the phone: a signed-out person opens on a guest Home and chats a few rounds through
 * Gateway's keyless guest routes (ADR 0019), as Nexus UI's guest mode does, then sees a sign-in card.
 * Gateway keeps no chat: the app holds the conversation and sends it whole each round. Each round carries a Cloudflare
 * Turnstile token from Nexus UI's challenge page in a web view (ADR 0019's 2026-11-24 amendment).
 *
 * Off unless the build sets `EXPO_PUBLIC_NEXUS_GUEST=on`. Off, a signed-out person meets the sign-in screen, as before.
 */
export const guestEnabled = (flag: string | undefined) => flag?.trim().toLowerCase() === "on";

/** Read whole, so Expo writes the value into the bundle. */
export const GUEST_ENABLED = guestEnabled(process.env.EXPO_PUBLIC_NEXUS_GUEST);

/** The apps' challenge page (ADR 0019 amendment): Nexus UI's, with the Turnstile widget and the guest site key. */
export const GUEST_CHALLENGE_URL = "https://nexus.inc/app/guest/challenge";
/** The page navigates here with `#<token>` or `#error=<code>`; the app cancels that navigation and reads the fragment. */
const TOKEN_URL = `${GUEST_CHALLENGE_URL}/token`;

/** A token is good for 300 seconds; the app drops one older than four minutes. */
export const TOKEN_MAX_AGE_MS = 4 * 60_000;

export type GuestRound = GuestChatMessage;

export interface GuestStatus {
  used: number;
  limit: number;
  /** The display name of the model every guest round runs on. */
  model: string;
}

/** Why a round did not run. `off`: guest chat is not configured, or this Gateway or Nexus UI has no guest route yet. */
export type GuestRefusal = "off" | "rounds_used" | "address_limit" | "paused" | "turnstile" | "invalid" | "failed";

export class GuestChatError extends Error {
  constructor(
    readonly reason: GuestRefusal,
    message: string,
  ) {
    super(message);
    this.name = "GuestChatError";
  }
}

const REASONS: Record<string, GuestRefusal> = {
  guest_chat_not_configured: "off",
  guest_rounds_used: "rounds_used",
  guest_address_limit: "address_limit",
  guest_chat_paused: "paused",
  turnstile_failed: "turnstile",
  guest_request_invalid: "invalid",
};

/** The refusal an error names: a `CoreError`'s code, or `off` for a Gateway without the route (404). */
export function refusalOf(error: unknown): GuestChatError {
  if (error instanceof GuestChatError) return error;
  if (!(error instanceof CoreError)) return new GuestChatError("failed", error instanceof Error ? error.message : String(error));
  const reason = (error.code ? REASONS[error.code] : undefined) ?? (error.status === 404 ? "off" : "failed");
  return new GuestChatError(reason, error.message);
}

type Core = Pick<ReturnType<typeof createCoreClient>, "guestChatStatus" | "streamGuestChatMessage">;

/** Guest chat for one guest id, on a keyless kit client. The same calls and readings as Nexus UI's `guest-chat.ts`. */
export function guestChat(core: Core, guestId: string) {
  return {
    /**
     * The rounds this guest has used, or null when guest chat is off. Rejects with `address_limit` when this guest
     * has rounds left but its address does not.
     */
    async status(signal?: AbortSignal): Promise<GuestStatus | null> {
      let status: GuestChatStatus;
      try {
        status = await core.guestChatStatus(guestId, signal);
      } catch (error) {
        const refusal = refusalOf(error);
        if (refusal.reason === "off") return null;
        throw refusal;
      }
      // `enabled` is false only for a round limit: this guest's, or its address's.
      if (!status.enabled && status.rounds_used < status.rounds_limit) throw new GuestChatError("address_limit", "This network used its guest rounds for today.");
      return { used: status.rounds_used, limit: status.rounds_limit, model: status.model };
    },
    /**
     * One round: the conversation so far, the last message the guest's. `onText` gets the answer as it arrives.
     * Resolves with the rounds after this one, or undefined when Gateway did not say. Sent once, never retried.
     */
    async send(messages: GuestRound[], turnstile: string, onText: (text: string) => void): Promise<{ used: number; limit: number } | undefined> {
      try {
        const { roundsUsed, roundsLimit } = await core.streamGuestChatMessage(guestId, { messages, turnstile }, (chunk) => {
          const text = chunk.choices?.[0]?.delta.content;
          if (text) onText(text);
        });
        return roundsUsed === null || roundsLimit === null ? undefined : { used: roundsUsed, limit: roundsLimit };
      } catch (error) {
        throw refusalOf(error);
      }
    },
  };
}

/**
 * What the challenge web view does with a navigation: load the challenge page and its frames (Cloudflare's), block
 * any other page, or hand over the token or Turnstile's error code, which the app reads and cancels.
 */
export type ChallengeStep = "load" | "block" | { token: string } | { error: string };

export function challengeStep(url: string, isTopFrame = true): ChallengeStep {
  if (url.startsWith(`${TOKEN_URL}#`)) {
    const fragment = decodeURIComponent(url.slice(TOKEN_URL.length + 1));
    if (fragment.startsWith("error=")) return { error: fragment.slice("error=".length) || "unknown" };
    return fragment ? { token: fragment } : { error: "empty" };
  }
  if (!isTopFrame) return url.startsWith("https://") || url === "about:blank" ? "load" : "block";
  return url === GUEST_CHALLENGE_URL || url.startsWith(`${GUEST_CHALLENGE_URL}?`) ? "load" : "block";
}

/** A kept token, if it is still young enough to send. */
export const freshToken = (kept: { token: string; at: number } | null, now: number) => (kept && now - kept.at < TOKEN_MAX_AGE_MS ? kept.token : null);

/** The sign-in card's words for each refusal that ends guest chat; the others leave the composer with a note. */
export const END: Partial<Record<GuestRefusal, { title: string; body: string }>> = {
  rounds_used: { title: "You used your free rounds", body: "Sign in to keep going with this chat. It is free to create an account." },
  address_limit: { title: "This network used its free rounds for today", body: "Sign in to keep going. It is free to create an account." },
  paused: { title: "Guest chat is resting for today", body: "Sign in to keep going. It is free to create an account." },
  off: { title: "Sign in to chat with Nexus", body: "It is free to create an account." },
};

/**
 * Which card ends guest chat, if any: a refusal that ended it, guest chat off (once the status is known), or the
 * rounds used. `status` undefined: still loading.
 */
export function endOf(ended: GuestRefusal | null, status: GuestStatus | null | undefined, off: boolean): GuestRefusal | null {
  if (ended) return ended;
  if (status === undefined) return null;
  if (off || status === null) return "off";
  return status.used >= status.limit ? "rounds_used" : null;
}

/** The note under the composer, as Nexus UI words it. */
export function noteOf(rounds: number, status: GuestStatus): string {
  const left = status.limit - status.used;
  if (rounds === 0) return `You are signed out. You have ${left} ${left === 1 ? "round" : "rounds"} on ${status.model}, and this chat is not saved.`;
  if (left <= 2) return `${left} ${left === 1 ? "round" : "rounds"} left. Sign in to keep going with this chat.`;
  return `${status.used} of ${status.limit} rounds used. This chat disappears when you close the app. Sign in to keep it.`;
}

/** The note after a round that failed but left guest chat open. */
export const problemOf = (reason: GuestRefusal) =>
  reason === "turnstile" ? "The check that you are a person did not pass. Try again." : "The answer did not arrive. Try again.";
