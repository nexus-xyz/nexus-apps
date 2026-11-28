import { describe, expect, it } from "vitest";
import { cannotSendQueued, queueFailure, restoreQueue, storeQueue } from "./chat-queue";

describe("a saved mobile Chat queue", () => {
  it("restores existing text drafts in order and keeps their request identities on subsequent launches", () => {
    const ids = ["first", "second"];
    const legacy = restoreQueue(["First draft", "Second draft"], () => ids.shift() ?? "unexpected");
    const saved = storeQueue(legacy, "stopped");
    expect(saved.held).toBe("stopped");
    expect(restoreQueue(saved, () => "new")).toMatchObject([
      { content: "First draft", id: "first" },
      { content: "Second draft", id: "second" },
    ]);
  });

  it("remembers selected request options and holds the draft if image data was kept only in memory", () => {
    const saved = storeQueue([{
      id: "img-1", content: "Read this screenshot.", now: true,
      attempt: { model: "nexus/auto", mode: "chat", team: "mobile" },
      images: [{ id: "local", mediaType: "image/png", name: "Screenshot.png", bytes: 8, uri: "data:image/png;base64,1234" }],
    }], null);
    const [restored] = restoreQueue(saved, () => "new");
    expect(restored).toMatchObject({ id: "img-1", attempt: { model: "nexus/auto", mode: "chat", team: "mobile" } });
    expect(restored?.images).toBeUndefined();
    expect(restored?.now).toBeUndefined();
    expect(restored && cannotSendQueued(restored)).toBe(true);
  });
});

describe("a Gateway send without a streamed acknowledgement", () => {
  it.each([
    ["idempotency_already_sent", "accepted"],
    ["idempotency_stream_replay", "accepted"],
    ["idempotency_interrupted", "unconfirmed"],
    ["idempotency_in_progress", "retry"],
    [undefined, "retry"],
  ] as const)("treats %s as %s", (code, status) => {
    expect(queueFailure(code)).toBe(status);
  });
});
