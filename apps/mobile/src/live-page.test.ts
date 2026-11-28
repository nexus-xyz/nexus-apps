import { CoreError, type PageEvent } from "@nexus/kit";
import { describe, expect, it, vi } from "vitest";
import { FAILURES_MAX, followLive, followShared, isLiveFrame, liveRevision, liveUrl, mintsAgain, RETRY_FIRST_MS, RETRY_MAX_MS, retryDelay, wait, type Follow } from "./live-page";

const version = (n: number): PageEvent => ({ type: "version", data: { pageId: "pag_1", version: n, title: "Q3", format: "html" } });
const preview = (sha: string): PageEvent => ({ type: "preview", data: { pageId: "pag_1", version: 2, contentSha256: sha } });

describe("a page that follows its live edits on the phone", () => {
  it("follows a shared link by id with the sign-in, and anything else by a public link's token", async () => {
    const client = { artifactEvents: vi.fn(async () => undefined), publicArtifactEvents: vi.fn(async () => undefined) };
    const signal = new AbortController().signal;
    const onEvent = () => undefined;
    await followShared(client, "art_1")(onEvent, signal);
    await followShared(client, "Xy9_token-0123456789abcdefghijklmnopq")(onEvent, signal);
    expect(client.artifactEvents).toHaveBeenCalledWith("art_1", onEvent, signal);
    expect(client.publicArtifactEvents).toHaveBeenCalledWith("Xy9_token-0123456789abcdefghijklmnopq", onEvent, signal);
  });

  it("follows only a frame of the live state, never a pinned version", () => {
    expect(isLiveFrame("https://p-abc.pages.test/v/pag_1/live?exp=1&sig=ab")).toBe(true);
    expect(isLiveFrame("https://p-abc.pages.test/v/pag_1/3?exp=1&sig=ab")).toBe(false);
    expect(isLiveFrame("https://p-abc.pages.test/v/live/3?exp=1&sig=ab")).toBe(false);
  });

  it("reloads at each version and preview, and at nothing else", () => {
    expect(liveRevision(version(3))).toBe("v3");
    expect(liveRevision(preview("ab12"))).toBe("ab12");
    expect(liveRevision({ type: "doc", data: {} })).toBeUndefined();
    expect(liveRevision({ type: "awareness", data: {} })).toBeUndefined();
    const url = "https://p-abc.pages.test/v/pag_1/live?exp=1&sig=ab";
    expect(liveUrl(url, undefined)).toBe(url);
    expect(liveUrl(url, "ab12")).toBe(`${url}&r=ab12`);
  });

  it("mints a frame URL again only when it is about to expire", () => {
    const now = Date.parse("2026-10-29T12:00:00.000Z");
    expect(mintsAgain("2026-10-29T12:04:00.000Z", now)).toBe(false);
    expect(mintsAgain("2026-10-29T12:00:20.000Z", now)).toBe(true);
    expect(mintsAgain("not a time", now)).toBe(true);
  });

  it("passes each event on, and opens a stream again a second after one that lasted", async () => {
    let clock = 0;
    const seen: PageEvent[] = [];
    const waits: number[] = [];
    const streams = [
      async (onEvent: (event: PageEvent) => void) => {
        onEvent(version(2));
        onEvent(preview("ab12"));
        clock += 60_000; // a Gateway copy drained it after a minute
      },
      async (onEvent: (event: PageEvent) => void) => {
        onEvent(version(3));
        throw new CoreError(404, "gone", "artifact_not_found");
      },
    ];
    const follow = vi.fn<Follow>(async (onEvent) => streams.shift()?.(onEvent));
    await followLive(follow, (event) => seen.push(event), new AbortController().signal, () => clock, async (ms) => void waits.push(ms));
    expect(seen).toEqual([version(2), preview("ab12"), version(3)]);
    expect(follow).toHaveBeenCalledTimes(2);
    expect(waits).toEqual([RETRY_FIRST_MS]);
  });

  it("a revoked link stops updating: its stream ends, and Gateway refuses it again", async () => {
    // The stream ends within the second after the revoke; reopening it is a 404, so no event follows.
    let clock = 0;
    const seen: PageEvent[] = [];
    let opened = 0;
    const follow: Follow = async (onEvent) => {
      opened += 1;
      if (opened > 1) throw new CoreError(404, "no artifact at this link", "artifact_not_found");
      onEvent(preview("before"));
      clock += 30_000;
    };
    await followLive(follow, (event) => seen.push(event), new AbortController().signal, () => clock, async () => undefined);
    expect(seen).toEqual([preview("before")]);
    expect(opened).toBe(2);
  });

  it("waits longer after each failure that passes, and gives up after a few in a row", async () => {
    const waits: number[] = [];
    const failures = [new TypeError("Network request failed"), new CoreError(503, "full", "page_events_full"), new CoreError(429, "slow down")];
    const follow = vi.fn<Follow>(async () => {
      const failure = failures.shift();
      if (failure) throw failure;
      // Then streams that end at once: failures too.
    });
    await followLive(follow, () => undefined, new AbortController().signal, () => 0, async (ms) => void waits.push(ms));
    expect(follow).toHaveBeenCalledTimes(FAILURES_MAX);
    expect(waits).toEqual([2_000, 4_000, 8_000, 16_000, 32_000]);
    expect(retryDelay(10)).toBe(RETRY_MAX_MS);
  });

  it("stops at once on a refusal, and when the screen closes", async () => {
    for (const status of [401, 403, 409]) {
      const follow = vi.fn<Follow>(async () => {
        throw new CoreError(status, "refused");
      });
      await followLive(follow, () => undefined, new AbortController().signal, () => 0, async () => undefined);
      expect(follow).toHaveBeenCalledTimes(1);
    }
    const closed = new AbortController();
    closed.abort();
    const never = vi.fn<Follow>(async () => undefined);
    await followLive(never, () => undefined, closed.signal);
    expect(never).not.toHaveBeenCalled();
    // Closing the screen while it waits ends the wait.
    const leaving = new AbortController();
    const waiting = wait(60_000, leaving.signal);
    leaving.abort();
    await waiting;
  });
});
