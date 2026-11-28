/**
 * The Assistant's thread, without React Native: where each of the person's messages is, the line under it, and the
 * messages that wait for the connection. The thread is a session transcript (ASSISTANT.md, "How the thread looks"),
 * so it shows no Delivered or Read: an agent has nothing to read. Only a message the agent never had says Not sent,
 * with Retry. A message sent while offline waits, and goes out when the connection is back.
 */

/** Where one of the person's messages is. `queued`: it goes out by itself when the connection is back. */
export type Receipt = { kind: "sending" } | { kind: "delivered" } | { kind: "failed"; queued?: true };

export const SENDING: Receipt = { kind: "sending" };
/** Not sent for a reason the person may fix (the agent refused it): only Retry sends it. */
export const NOT_SENT: Receipt = { kind: "failed" };
/** Not sent because the phone is offline: it goes out when the connection is back. */
export const QUEUED: Receipt = { kind: "failed", queued: true };

/** The line under the thread while the phone is offline (A1·C i). */
export const OFFLINE_LINE = "You’re offline. Messages send when you’re back.";

/**
 * Nexus's first message in an empty thread with no first open (the Assistant canvas, "First open"):
 * "Hi Maya. I'm Nexus. Ask me for anything." The profile header above it says where it lives.
 */
export const greetingOf = (firstName: string | undefined, assistant: string): string =>
  `${firstName ? `Hi ${firstName}.` : "Hi."} I’m ${assistant}. Ask me for anything.`;

/** The receipt after any event of the ask: the agent has the message. */
export function receiptAfter(receipt: Receipt): Receipt {
  return receipt.kind === "failed" ? receipt : { kind: "delivered" };
}

/**
 * The receipt once an ask ended without a reply. A message the agent never had is not sent: queued when the network
 * failed, waiting for Retry otherwise. One the agent had keeps its receipt, and the screen says what went wrong.
 */
export const receiptAfterFailure = (receipt: Receipt, network: boolean): Receipt => (receipt.kind === "sending" ? (network ? QUEUED : NOT_SENT) : receipt);

/** The line under a message of yours: Not sent with Retry, and nothing otherwise. */
export const receiptLine = (receipt: Receipt | undefined): string | undefined => (receipt?.kind === "failed" ? "Not sent · Retry" : undefined);

const pad = (n: number) => String(n).padStart(2, "0");

/** A time as the thread writes it: 24 hours, "08:02", on the phone's clock. */
export const clock = (at: number): string => {
  const date = new Date(at);
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
};

/** The first message that waits for the connection, in the thread's order. */
export function nextQueued(ids: string[], receipts: Record<string, Receipt>): string | undefined {
  return ids.find((id) => {
    const receipt = receipts[id];
    return receipt?.kind === "failed" && receipt.queued === true;
  });
}

/** The phone tries the connection again this often while offline, and each time the app comes back to the front. */
export const PROBE_MS = 5000;
