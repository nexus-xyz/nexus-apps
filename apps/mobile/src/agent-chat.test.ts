import { agentProblem, type AgentMessage } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { agentsApiUrl, agentTarget, answerLine, entriesOf, phoneProblem, sourcesOf } from "./agent-chat";
import { usd } from "./meta";

// The Assistant on the phone (S2.5): where the API is, the words for each failure, and what an answer shows.

type Answer = Extract<AgentMessage, { kind: "answer" }>;
const answer = (more: Partial<Answer> = {}): Answer => ({ kind: "answer", text: "Hi", model: "anthropic/claude-sonnet-5", costUsd: 0.0123, ...more });
const short = (id: string) => id.slice(id.lastIndexOf("/") + 1);

describe("where the Assistant's client calls", () => {
  it("is the agent API by default, as before", () => {
    for (const flag of [undefined, "", "off", "1", "true"]) {
      expect(agentTarget(flag, "https://agents.example.test", "https://api.nexus.test"), String(flag)).toEqual({ baseUrl: "https://agents.example.test" });
    }
    expect(agentTarget(undefined, undefined, "https://api.nexus.test")).toBeUndefined();
  });

  it("is the signed-in Gateway with EXPO_PUBLIC_NEXUS_AGENT_VIA_GATEWAY=on, with or without the agent API's URL", () => {
    for (const agents of ["https://agents.example.test", undefined]) {
      expect(agentTarget(" on ", agents, "https://api.nexus.test")).toEqual({ baseUrl: "https://api.nexus.test", viaGateway: true });
    }
    expect(agentTarget("on", "https://agents.example.test", undefined)).toBeUndefined();
  });
});

describe("where the agent API is", () => {
  it("takes an https URL, without its trailing slash", () => {
    expect(agentsApiUrl(" https://agents.example.test/ ")).toBe("https://agents.example.test");
  });

  it("takes plain http only to this machine or a private network", () => {
    for (const url of ["http://127.0.0.1:8787", "http://localhost:4621", "http://192.168.1.20:4621", "http://10.0.0.2"]) {
      expect(agentsApiUrl(url), url).toBe(url);
    }
    for (const url of ["http://agents.example.com", "http://localhost.evil.com", "http://192.168.1.5.evil.com", "ftp://x", "https://"]) {
      expect(agentsApiUrl(url), url).toBeUndefined();
    }
  });

  it("is not set when the build has no URL, so the screen says the agent is not available here", () => {
    expect(agentsApiUrl(undefined)).toBeUndefined();
    expect(agentsApiUrl("  ")).toBeUndefined();
    expect(agentsApiUrl("agents.example.test")).toBeUndefined();
  });
});

describe("what the screen says when an ask fails", () => {
  it("asks to sign in again in the app on 401", () => {
    expect(phoneProblem({ status: 401, error: "credential_refused", detail: "Sign in again with `nexus auth login`." })).toEqual({
      title: "Sign in again",
      detail: "Nexus refused this sign-in. Sign out in Account, sign in again, then ask again.",
      signIn: true,
    });
  });

  it("shows the API's own hint on 403, and plain words on 429", () => {
    // The 403 title is the kit's (`agentProblem`), shared with Nexus UI and desktop: the phone keeps it and the hint.
    expect(phoneProblem({ status: 403, error: "not_a_person", detail: "Use your own key." })).toEqual({ title: agentProblem(403, {}).title, detail: "Use your own key." });
    expect(phoneProblem({ status: 429, detail: "You asked more than 10 questions in a minute." }).detail).toBe("Wait a moment, then ask again.");
  });
});

describe("an answer", () => {
  it("names the model, why it ran, the cost, the notes and the conversation's total", () => {
    expect(answerLine(answer({ choice: { keyword: "best" }, notesUsed: 1, conversation: { answers: 2, costUsd: 0.05 } }), short)).toEqual([
      "claude-sonnet-5",
      "you asked for best",
      usd(0.0123),
      "1 note about you",
      `${usd(0.05)} in this conversation`,
    ]);
    // A turn read back has no cost.
    expect(answerLine(answer({ costUsd: Number.NaN }), short)).toEqual(["claude-sonnet-5"]);
  });

  it("lists its web sources once each, and never a script link", () => {
    const sources = [
      { title: "Runbook", url: "https://wiki.example.com/runbook" },
      { title: "Again", url: "https://wiki.example.com/runbook" },
      { title: "", url: "https://slack.example.com/archives/C1/p1" },
      { title: "Bad", url: "javascript:alert(1)" },
    ];
    expect(sourcesOf(answer({ sources }))).toEqual([
      { title: "Runbook", url: "https://wiki.example.com/runbook" },
      { title: "https://slack.example.com/archives/C1/p1", url: "https://slack.example.com/archives/C1/p1" },
    ]);
    expect(sourcesOf(answer())).toEqual([]);
  });
});

it("reads a conversation back as your questions and the agent's answers", () => {
  expect(
    entriesOf([
      { role: "user", text: "Hi", at: "2026-11-04T10:00:00.000Z" },
      { role: "assistant", text: "Hello", at: "2026-11-04T10:00:02.000Z", model: "openai/gpt-6", costUsd: 0.001 },
    ]),
  ).toEqual([
    { id: "h0", role: "user", text: "Hi" },
    // Read back: the API keeps no notes used or model choice on a turn, so the screen shows no line under it.
    { id: "h1", role: "agent", messages: [{ kind: "answer", text: "Hello", model: "openai/gpt-6", costUsd: 0.001 }], readBack: true },
  ]);
});

it("restores answer sources and the pending approval card, preserving the original reply", () => {
  const messages: AgentMessage[] = [
    answer({ sources: [{ title: "Runbook", url: "https://wiki.example.com/runbook" }], choice: { keyword: "best" } }),
    { kind: "pending_action", card: "c1", write: { connector: "github", operation: "create_issue", summary: "Open an issue", effect: "reversible", arguments: { title: "Bug" } }, approveUrl: "https://nexus.inc/app/acme/approve/a1", detail: "Review this write." },
  ];
  const turns = [{ role: "assistant" as const, text: "Hi", model: "m", costUsd: 0.01, at: "2026-11-09T10:00:00Z", messages }];
  expect(entriesOf(turns)).toEqual([{ id: "h0", role: "agent", messages }]);
});
