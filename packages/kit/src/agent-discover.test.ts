import { describe, expect, it } from "vitest";
import { CONNECTOR_UNLOCKS, DISCOVER_IDEAS, DISCOVER_NEWS, seenNewsOf, unlocksOf, unseenNews } from "./agent-discover";

// Discover's static words (A15): the ideas, what each connector unlocks, and the dot on What's new.

describe("Discover's static lists", () => {
  it("has about eight ideas, each with its own id and a line Nexus can take as it is", () => {
    expect(DISCOVER_IDEAS.length).toBe(8);
    expect(new Set(DISCOVER_IDEAS.map((idea) => idea.id)).size).toBe(DISCOVER_IDEAS.length);
    for (const idea of DISCOVER_IDEAS) {
      expect(idea.title.length).toBeLessThanOrEqual(24);
      expect(idea.line.trim()).toBe(idea.line);
      // A line starts no command: it reads as the person's own words.
      expect(idea.line).not.toMatch(/^[!/]/);
    }
  });

  it("names a connector only where it has a line of what it unlocks", () => {
    for (const idea of DISCOVER_IDEAS) if (idea.connector) expect(CONNECTOR_UNLOCKS[idea.connector]).toBeTruthy();
  });

  it("keeps each What's new id unique", () => {
    expect(new Set(DISCOVER_NEWS.map((item) => item.id)).size).toBe(DISCOVER_NEWS.length);
  });
});

describe("What's new's dot", () => {
  const news = [
    { id: "a", title: "A", line: "a", action: { kind: "view" as const, view: "chat" as const } },
    { id: "b", title: "B", line: "b", action: { kind: "ask" as const, line: "b" } },
  ];

  it("marks the lines not seen yet, in order", () => {
    expect(unseenNews([], news)).toEqual(["a", "b"]);
    expect(unseenNews(["a"], news)).toEqual(["b"]);
    expect(unseenNews(["a", "b", "gone"], news)).toEqual([]);
  });

  it("reads what the device kept strictly: anything else is nothing seen", () => {
    expect(seenNewsOf(null)).toEqual([]);
    expect(seenNewsOf("not json")).toEqual([]);
    expect(seenNewsOf('{"a":1}')).toEqual([]);
    expect(seenNewsOf('["a", 2, "b"]')).toEqual(["a", "b"]);
  });
});

describe("what a connector unlocks", () => {
  it("says the kit's line, else the catalog's", () => {
    expect(unlocksOf("github", "Metadata")).toBe("Your pull requests, issues and reviews");
    expect(unlocksOf("acme-crm", "Accounts and contacts")).toBe("Accounts and contacts");
  });
});
