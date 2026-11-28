import { createCoreClient } from "@nexus/kit";
import { describe, expect, it, vi } from "vitest";
import { addSuggestionToDraft, nextStepModel, requestNextStep } from "./chat-next-step";

const answer = { id: "chm_fixture", role: "assistant", status: "complete", mode: "chat", model: "test/model" } as const;
const suggestion = { suggestion: "Check the release notes.", model: "test/model", costUsd: 0.001 };

describe("chat next steps", () => {
  it("takes its model from a completed Chat answer", () => {
    expect(nextStepModel(answer)).toBe("test/model");
    expect(nextStepModel({ ...answer, id: "answer" })).toBeUndefined();
    expect(nextStepModel({ ...answer, role: "user" })).toBeUndefined();
    expect(nextStepModel({ ...answer, status: "failed" })).toBeUndefined();
    expect(nextStepModel({ ...answer, stopped: true })).toBeUndefined();
    expect(nextStepModel({ ...answer, mode: "ask" })).toBeUndefined();
    expect(nextStepModel({ ...answer, model: "nexus/auto" })).toBeUndefined();
  });

  it("keeps the draft and inserts the suggestion without sending a message", () => {
    expect(addSuggestionToDraft("", "Check the release notes.")).toBe("Check the release notes.");
    expect(addSuggestionToDraft("Existing draft.  \n", "Check the release notes.")).toBe("Existing draft.  \n\nCheck the release notes.");
  });

  it("does not start a paid request when consent is denied or the answer changes", async () => {
    const load = vi.fn(async () => suggestion);
    const controller = new AbortController();
    expect(await requestNextStep(async () => false, load, controller.signal)).toEqual({ kind: "cancelled" });
    expect(await requestNextStep(async () => {
      controller.abort();
      return true;
    }, load, controller.signal)).toEqual({ kind: "cancelled" });
    expect(load).not.toHaveBeenCalled();
  });

  it("uses the owner-scoped suggestion endpoint after consent", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response(JSON.stringify(suggestion)));
    const core = createCoreClient({ baseUrl: "https://gateway.test", apiKey: "fixture", fetch });
    const controller = new AbortController();
    const result = await requestNextStep(async () => true, () => core.suggestChatMessage("chs_fixture", controller.signal), controller.signal);

    expect(result).toEqual({ kind: "ready", text: "Check the release notes." });
    expect(fetch).toHaveBeenCalledExactlyOnceWith("https://gateway.test/v1/chat/sessions/chs_fixture/suggest", expect.objectContaining({
      method: "POST",
      body: "{}",
      signal: controller.signal,
    }));
  });

  it("distinguishes Gateway returning no suggestion from an unreadable response", async () => {
    const signal = new AbortController().signal;
    expect(await requestNextStep(async () => true, async () => ({ suggestion: null, model: null, costUsd: 0 }), signal)).toEqual({ kind: "absent" });
    await expect(requestNextStep(async () => true, async () => ({ ...suggestion, suggestion: "one\ntwo" }), signal)).rejects.toThrow("invalid suggestion");
  });
});
