import { describe, expect, it } from "vitest";
import { launchSession } from "./session-place";

describe("restoring the last primary session on launch", () => {
  it("reopens chat and synced sessions through their own viewer", () => {
    expect(launchSession({ id: "chat_42", source: "chat" })).toEqual({ pathname: "/session/[id]", params: { id: "chat_42", source: "chat" } });
    expect(launchSession({ id: "0192f3a4-0000-7000-8000-000000000003", source: "synced" })).toMatchObject({ params: { source: "synced" } });
  });

  it("opens a fresh session with missing or unusable saved state", () => {
    expect(launchSession(undefined)).toBe("/session/new");
    expect(launchSession({ id: "https://example.test/", source: "chat" })).toBe("/session/new");
  });
});
