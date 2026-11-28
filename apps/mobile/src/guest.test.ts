import { createCoreClient } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { challengeStep, END, endOf, freshToken, GUEST_CHALLENGE_URL, GuestChatError, guestChat, guestEnabled, noteOf, TOKEN_MAX_AGE_MS } from "./guest";

// Guest mode on the phone: the flag, Gateway's guest routes and their limits, the challenge page's
// handoff, and when the sign-in card ends guest chat.

const ID = "8b0d8f6e-3c1a-4d6e-9a43-0f1d2c3b4a59";

/** A kit client on a fake Gateway that answers each request with the next response. */
function gateway(...responses: Response[]) {
  const sent: { url: string; guest: string | null; body?: unknown }[] = [];
  const core = createCoreClient({
    baseUrl: "https://gateway.test",
    idempotency: true,
    fetch: async (input, init) => {
      const headers = new Headers(init?.headers);
      sent.push({ url: String(input), guest: headers.get("x-nexus-guest"), body: init?.body ? JSON.parse(String(init.body)) : undefined });
      return responses.shift() ?? new Response(null, { status: 500 });
    },
  });
  return { chat: guestChat(core, ID), sent };
}

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const refusal = (status: number, code: string) => json(status, { error: { code, message: code } });
const stream = (text: string, used: number, limit: number) =>
  new Response(
    [
      `data: ${JSON.stringify({ model: "deepseek/deepseek-v4-flash-0731", choices: [{ index: 0, delta: { content: text }, finish_reason: null }] })}\n\n`,
      `data: ${JSON.stringify({ model: "deepseek/deepseek-v4-flash-0731", choices: [{ index: 0, delta: {}, finish_reason: "stop" }] })}\n\n`,
      "data: [DONE]\n\n",
    ].join(""),
    { status: 200, headers: { "content-type": "text/event-stream", "x-nexus-guest-rounds-used": String(used), "x-nexus-guest-rounds-limit": String(limit) } },
  );

describe("the flag", () => {
  it("is on only for `on`, so a build without it opens on sign-in as before", () => {
    expect([undefined, "", "off", "1", "true"].map(guestEnabled)).toEqual([false, false, false, false, false]);
    expect([" on ", "ON", "on"].map(guestEnabled)).toEqual([true, true, true]);
  });
});

describe("guest chat on Gateway's guest routes", () => {
  it("reads the rounds under the guest id", async () => {
    const { chat, sent } = gateway(json(200, { enabled: true, rounds_used: 2, rounds_limit: 5, model: "DeepSeek V4 Flash 0731" }));
    expect(await chat.status()).toEqual({ used: 2, limit: 5, model: "DeepSeek V4 Flash 0731" });
    expect(sent).toEqual([{ url: "https://gateway.test/v1/guest/chat", guest: ID, body: undefined }]);
  });

  it("is off when Gateway has no guest chat set up, or no guest route", async () => {
    expect(await gateway(refusal(503, "guest_chat_not_configured")).chat.status()).toBeNull();
    expect(await gateway(refusal(404, "not_found")).chat.status()).toBeNull();
  });

  it("says when the address, not the guest, used today's rounds", async () => {
    const { chat } = gateway(json(200, { enabled: false, rounds_used: 1, rounds_limit: 5, model: "m" }));
    await expect(chat.status()).rejects.toMatchObject({ reason: "address_limit" });
  });

  it("sends the whole conversation with the token, and reads the rounds after it", async () => {
    const { chat, sent } = gateway(stream("Hello", 3, 5));
    let text = "";
    const messages = [{ role: "user" as const, content: "Hi" }];
    expect(await chat.send(messages, "tok", (piece) => (text += piece))).toEqual({ used: 3, limit: 5 });
    expect(text).toBe("Hello");
    expect(sent).toEqual([{ url: "https://gateway.test/v1/guest/chat/messages", guest: ID, body: { messages, turnstile: "tok" } }]);
  });

  it("names each refusal", async () => {
    const cases = [
      ["guest_rounds_used", 429, "rounds_used"],
      ["guest_address_limit", 429, "address_limit"],
      ["guest_chat_paused", 429, "paused"],
      ["turnstile_failed", 400, "turnstile"],
      ["guest_request_invalid", 400, "invalid"],
      ["guest_chat_not_configured", 503, "off"],
      ["turnstile_unavailable", 502, "failed"],
    ] as const;
    for (const [code, status, reason] of cases) {
      await expect(gateway(refusal(status, code)).chat.send([{ role: "user", content: "Hi" }], "tok", () => {}), code).rejects.toMatchObject({ reason });
    }
    await expect(gateway().chat.send([], "tok", () => {})).rejects.toBeInstanceOf(GuestChatError);
  });
});

describe("the sign-in card", () => {
  const status = (used: number, limit = 5) => ({ used, limit, model: "DeepSeek V4 Flash 0731" });

  it("waits for the status, then shows when guest chat is off", () => {
    expect(endOf(null, undefined, false)).toBeNull();
    expect(endOf(null, null, false)).toBe("off");
    // The challenge page answered 404: Nexus UI has no guest site key, so no round can pass.
    expect(endOf(null, status(0), true)).toBe("off");
  });

  it("shows once the rounds are used, and not before", () => {
    expect(endOf(null, status(4), false)).toBeNull();
    expect(endOf(null, status(5), false)).toBe("rounds_used");
  });

  it("shows for a refusal that ends guest chat, with words for each", () => {
    expect(endOf("paused", status(1), false)).toBe("paused");
    for (const reason of ["rounds_used", "address_limit", "paused", "off"] as const) expect(END[reason]?.title).toBeTruthy();
    // A refused check or a failed answer leaves the composer, with a note to try again.
    expect(END.turnstile).toBeUndefined();
    expect(END.failed).toBeUndefined();
  });

  it("counts down the rounds under the composer, as Nexus UI does", () => {
    expect(noteOf(0, status(0))).toBe("You are signed out. You have 5 rounds on DeepSeek V4 Flash 0731, and this chat is not saved.");
    expect(noteOf(2, status(1))).toBe("1 of 5 rounds used. This chat disappears when you close the app. Sign in to keep it.");
    expect(noteOf(6, status(3))).toBe("2 rounds left. Sign in to keep going with this chat.");
    expect(noteOf(8, status(4))).toBe("1 round left. Sign in to keep going with this chat.");
  });
});

describe("the challenge page's handoff", () => {
  it("loads the challenge page and Cloudflare's frames, and nothing else", () => {
    expect(challengeStep(GUEST_CHALLENGE_URL)).toBe("load");
    expect(challengeStep("https://challenges.cloudflare.com/cdn-cgi/challenge-platform/x", false)).toBe("load");
    expect(challengeStep("https://nexus.inc/app/login")).toBe("block");
    expect(challengeStep("https://evil.test/app/guest/challenge")).toBe("block");
    expect(challengeStep("nexus://connect", false)).toBe("block");
  });

  it("reads the token or Turnstile's error from the fragment", () => {
    expect(challengeStep(`${GUEST_CHALLENGE_URL}/token#tok`)).toEqual({ token: "tok" });
    expect(challengeStep(`${GUEST_CHALLENGE_URL}/token#error=300030`)).toEqual({ error: "300030" });
    expect(challengeStep(`${GUEST_CHALLENGE_URL}/token#`)).toEqual({ error: "empty" });
    // Without a fragment the address is a plain page, not a handoff.
    expect(challengeStep(`${GUEST_CHALLENGE_URL}/token`)).toBe("block");
  });

  it("drops a token older than four minutes", () => {
    expect(freshToken({ token: "t", at: 0 }, TOKEN_MAX_AGE_MS - 1)).toBe("t");
    expect(freshToken({ token: "t", at: 0 }, TOKEN_MAX_AGE_MS)).toBeNull();
    expect(freshToken(null, 0)).toBeNull();
  });
});
