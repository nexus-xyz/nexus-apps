/**
 * A page that follows its live edits while it is open on the phone, as Nexus UI's frame does.
 * A frame of the page's live state (`/v/<pageId>/live`) reloads at each `version` and `preview` event, without a new
 * frame URL: the sandbox host ignores `r`, which only makes the WebView load the live state again.
 *
 * The events come from the page's own stream for its owner (`pageEvents`), and from the share's for a shared link
 * (`artifactEvents` by id, `publicArtifactEvents` by a public link's token). Gateway ends a share's stream once it is
 * revoked or expires, and refuses it again after, so a revoked link stops updating. A pinned share's frame is of one
 * version, so it follows nothing.
 */
import { CoreError, isArtifactId, type ArtifactsClient, type PageEvent, type PagePreviewEvent, type PageVersionEvent } from "@nexus/kit";

/** Open one stream of a page's events: resolves when it ends, rejects when it is refused or breaks. */
export type Follow = (onEvent: (event: PageEvent) => void, signal: AbortSignal) => Promise<void>;

/** A shared link's events, as its frame: by id with the sign-in, or by a public link's token. */
export const followShared =
  (client: Pick<ArtifactsClient, "artifactEvents" | "publicArtifactEvents">, key: string): Follow =>
  (onEvent, signal) =>
    isArtifactId(key) ? client.artifactEvents(key, onEvent, signal) : client.publicArtifactEvents(key, onEvent, signal);

/** Whether a frame URL shows the page's live state, the only frame that has edits to follow. */
export const isLiveFrame = (url: string) => /\/v\/[^/?#]+\/live\?/.test(url);

/** The revision a live frame reloads at for `event`: the version written, or the unsaved text's SHA-256. Else none. */
export function liveRevision(event: PageEvent): string | undefined {
  if (event.type === "version") return `v${(event.data as PageVersionEvent).version}`;
  if (event.type === "preview") return (event.data as PagePreviewEvent).contentSha256;
  return undefined;
}

/** The live frame's URL at `revision`. */
export const liveUrl = (url: string, revision: string | undefined) => (revision ? `${url}&r=${encodeURIComponent(revision)}` : url);

/** A frame URL that expires within this is minted again before a reload: a reload after its five minutes fails. */
export const MINT_AHEAD_MS = 30_000;

/** Whether a frame expiring at `expiresAt` (ISO) needs a new URL before the next reload. */
export const mintsAgain = (expiresAt: string, now: number = Date.now()) => !(Date.parse(expiresAt) - now > MINT_AHEAD_MS);

/** A stream that ends sooner than this after it opened counts as a failure, as in Nexus UI. */
export const STREAM_MIN_MS = 5_000;

/** The wait before a stream opens again: a second after one that lasted, doubling with each failure in a row. */
export const RETRY_FIRST_MS = 1_000;
export const RETRY_MAX_MS = 60_000;
/** After this many failures in a row, the page stops following until the screen opens again. */
export const FAILURES_MAX = 6;

export const retryDelay = (failures: number) => Math.min(RETRY_MAX_MS, RETRY_FIRST_MS * 2 ** failures);

/** Wait `ms`, or less when `signal` aborts. */
export const wait = (ms: number, signal: AbortSignal) =>
  new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
  });

/**
 * Follow a page while `signal` is open: each event to `onEvent`. A stream that ends (a Gateway copy's drain, a revoke
 * Gateway noticed) opens again after a second. Gateway's refusal (4xx but 429: a revoked, expired or pinned share, no
 * access) stops it for good, and the page stays as it is. A failure that passes (no network, the app in the background,
 * Gateway away or busy, a stream that ends at once) waits longer each time, and stops after `FAILURES_MAX` in a row.
 */
export async function followLive(
  follow: Follow,
  onEvent: (event: PageEvent) => void,
  signal: AbortSignal,
  now: () => number = Date.now,
  sleep: (ms: number, signal: AbortSignal) => Promise<void> = wait,
): Promise<void> {
  let failures = 0;
  while (!signal.aborted) {
    const opened = now();
    try {
      await follow(onEvent, signal);
      failures = now() - opened < STREAM_MIN_MS ? failures + 1 : 0;
    } catch (error) {
      if (error instanceof CoreError && error.status >= 400 && error.status < 500 && error.status !== 429) return;
      failures += 1;
    }
    if (signal.aborted || failures >= FAILURES_MAX) return;
    await sleep(retryDelay(failures), signal);
  }
}
