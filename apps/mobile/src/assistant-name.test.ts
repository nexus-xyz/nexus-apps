import type { AgentProfile } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { assistantNameOf, DEFAULT_ASSISTANT_NAME, known, nameOf, setName, subscribe } from "./assistant-name";

// The Assistant's name on the phone: the person's choice, Nexus by default.

const profile = (more: Partial<AgentProfile>): AgentProfile =>
  ({ id: "p1", owner: "person", ownerId: "u1", name: "Juno", avatar: null, tone: null, greeting: null, language: null, labels: [], instructions: "", tools: null, ...more }) as AgentProfile;

describe("the Assistant's name", () => {
  it("is the person's own profile's name, trimmed, and Nexus without one", () => {
    expect(assistantNameOf([profile({ owner: "team", name: "Support" }), profile({ name: " Juno " })])).toBe("Juno");
    expect(assistantNameOf([profile({ name: "  " })])).toBe(DEFAULT_ASSISTANT_NAME);
    expect(assistantNameOf([profile({ owner: "team", name: "Support" })])).toBe("Nexus");
    expect(assistantNameOf(null)).toBe("Nexus");
  });

  it("is kept per account, and every reader hears a change", () => {
    let heard = 0;
    const stop = subscribe(() => (heard += 1));
    expect(nameOf("acme")).toBe("Nexus");
    expect(known("acme")).toBe(false);
    setName("acme", "Juno");
    setName("acme", "Juno");
    expect(nameOf("acme")).toBe("Juno");
    expect(nameOf("other")).toBe("Nexus");
    expect(known("acme")).toBe(true);
    expect(heard).toBe(1);
    stop();
  });
});
