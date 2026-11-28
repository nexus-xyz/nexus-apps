import { describe, expect, it } from "vitest";
import { ruleRef } from "./model-policy";

describe("model policy", () => {
  it("says where a decision came from", () => {
    expect(ruleRef({ level: "none" } as never)).toBe("no rule restricts it");
    expect(ruleRef({ level: "organization", list: "deny", pattern: "openai/*" } as never)).toBe("organization · deny list · openai/*");
    expect(ruleRef({ level: "team", subject: "core", list: "allow" } as never)).toBe("team core · allow list");
    expect(ruleRef({ level: "team", list: "allow" } as never)).toBe("team · allow list");
  });
});
