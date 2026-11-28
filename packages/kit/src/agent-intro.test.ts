import { describe, expect, it } from "vitest";
import { createAgentClient } from "./agent-chat";
import { firstNameOf, introHeadline, introOf, introReads, INTRO_STARTERS, type AgentIntro } from "./agent-intro";

// Assistant's first open (A19): the agent API's intro, read strictly, and the words every app shows.

const BODY = {
  firstOpen: true,
  line: "Hand me anything: questions, drafts and research.",
  starters: ["List my open Linear issues", "Draft a polite reply", "What do you remember about me?"],
  reads: ["Linear", "GitHub"],
  accounts: true,
  discover: true,
};
const INTRO: AgentIntro = { line: BODY.line, starters: BODY.starters, reads: BODY.reads, accounts: true, discover: true };

describe("the first open, read strictly", () => {
  it("reads the intro, `seen` once shown, and nothing that is neither", () => {
    expect(introOf(BODY)).toEqual(INTRO);
    expect(introOf({ firstOpen: false })).toBe("seen");
    for (const body of [undefined, null, "x", {}, { firstOpen: true }, { firstOpen: true, line: "  ", starters: [] }, { firstOpen: true, line: "Hi", starters: "no" }]) {
      expect(introOf(body), JSON.stringify(body)).toBeUndefined();
    }
  });

  it("drops a starter or a name that is not text, repeats, past four starters, and links the API did not say apply", () => {
    const read = introOf({ ...BODY, starters: ["a", 1, "a", "b", "c", "d", "e", "x".repeat(301)], reads: ["Linear", {}, "Linear"], accounts: "yes", discover: undefined });
    expect(read).toEqual({ line: BODY.line, starters: ["a", "b", "c", "d"], reads: ["Linear"], accounts: false, discover: false });
    expect((read as AgentIntro).starters).toHaveLength(INTRO_STARTERS);
  });
});

describe("the words around it", () => {
  it("greets the person by their first name, or says who it is", () => {
    expect(introHeadline(INTRO, firstNameOf("Maya Chen"))).toBe("Hi Maya. Hand me anything: questions, drafts and research.");
    expect(introHeadline(INTRO, firstNameOf("  "))).toBe("Hi, I'm Nexus. Hand me anything: questions, drafts and research.");
    expect(firstNameOf(undefined)).toBeUndefined();
  });

  it("says what the organization's Nexus can read, and nothing when it reads nothing", () => {
    expect(introReads(INTRO, { name: "Acme", personal: false })).toBe("Acme's Nexus can read Linear and GitHub.");
    expect(introReads({ ...INTRO, reads: ["Gmail", "Google Calendar", "GitHub"] }, { name: "Personal", personal: true })).toBe("Nexus can read Gmail, Google Calendar and GitHub.");
    expect(introReads({ ...INTRO, reads: ["Linear"] })).toBe("Nexus can read Linear.");
    expect(introReads({ ...INTRO, reads: [] }, { name: "Acme" })).toBeUndefined();
  });
});

describe("the client", () => {
  const client = (answer: (path: string, method: string) => Response) => {
    const calls: string[] = [];
    const agent = createAgentClient({
      baseUrl: "https://agents.test/",
      apiKey: "nxk_ann",
      fetch: async (input, init) => {
        const path = new URL(String(input)).pathname;
        calls.push(`${init?.method} ${path} ${new Headers(init?.headers).get("authorization")}`);
        return answer(path, String(init?.method));
      },
    });
    return { agent, calls };
  };

  it("reads the first open, `seen`, `off` on a 404, and a failure, with the person's own key", async () => {
    const { agent, calls } = client(() => Response.json(BODY));
    expect(await agent.intro()).toEqual({ kind: "intro", intro: INTRO });
    expect(calls).toEqual(["GET /agent/intro Bearer nxk_ann"]);
    expect(await client(() => Response.json({ firstOpen: false })).agent.intro()).toEqual({ kind: "seen" });
    expect(await client(() => Response.json({ error: "not_found" }, { status: 404 })).agent.intro()).toEqual({ kind: "off" });
    expect(await client(() => Response.json({ error: "intro_unavailable" }, { status: 503 })).agent.intro()).toEqual({ kind: "failed", status: 503 });
    expect(await client(() => Response.json({ firstOpen: "maybe" })).agent.intro()).toEqual({ kind: "failed", status: 502 });
  });

  it("keeps that the person saw it", async () => {
    const { agent, calls } = client(() => Response.json({ seen: true }));
    expect(await agent.introSeen()).toBe(true);
    expect(calls).toEqual(["POST /agent/intro/seen Bearer nxk_ann"]);
    expect(await client(() => Response.json({ error: "x" }, { status: 503 })).agent.introSeen()).toBe(false);
  });
});
