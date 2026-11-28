import { describe, expect, it } from "vitest";
import { answerSessions, sessionCardLine, sessionUrl, type AgentSession } from "./agent-chat";

// The A1 session link card: an answer's sessions, read strictly, and the card's line.

const desktop: AgentSession = {
  where: "desktop",
  state: "ready",
  title: "Fix the failing decoder tests",
  repo: "acme/cortex-sim",
  branch: "decoder-gru",
  url: "nexus://new?prompt=Fix&repo=acme%2Fcortex-sim",
};
const synced: AgentSession = { where: "synced", state: "done", title: "Codex session", detail: "gpt-5.5", url: "https://ui.test/app?next=%2Fsynced%2Ft_1", id: "t_1" };

describe("an answer's session cards (A1)", () => {
  it("keeps a desktop draft link and an http(s) page, each once", () => {
    expect(answerSessions({ sessions: [desktop, synced, synced] })).toEqual([desktop, synced]);
  });

  it("drops an entry with an unknown place or state, no title, or a link it may not open", () => {
    const bad = [
      { ...synced, where: "cloud" },
      { ...synced, state: "queued" },
      { ...synced, title: "  " },
      { ...synced, url: "javascript:alert(1)" },
      // A synced session never opens a desktop link, and a desktop one opens only `nexus://new?`.
      { ...synced, url: desktop.url },
      { ...desktop, url: "nexus://settings" },
      { ...desktop, url: "https://ui.test" },
    ];
    expect(answerSessions({ sessions: bad })).toEqual([]);
    expect(answerSessions({})).toEqual([]);
    expect(sessionUrl("nexus://new?prompt=a", "synced")).toBeUndefined();
  });

  it("leaves out a repository that is not owner/name, and keeps the card", () => {
    expect(answerSessions({ sessions: [{ ...desktop, repo: "<script>" }] })[0]).not.toHaveProperty("repo");
  });

  it("writes the card's line as the board does: state, branch, repository", () => {
    expect(sessionCardLine({ ...desktop, state: "done" })).toEqual({ state: "Done", detail: "on decoder-gru", repo: "cortex-sim" });
    expect(sessionCardLine(synced)).toEqual({ state: "Done", detail: "gpt-5.5" });
    expect(sessionCardLine(desktop).state).toBe("Ready to open");
  });
});
