import { describe, expect, it } from "vitest";
import { activityChips, connectorLabel } from "./activity";

describe("an answer's activity chips", () => {
  it("lists skills, then each connector once with its reads, then the web, then the page", () => {
    const chips = activityChips({
      skills: [{ skillId: "s1", name: "brand-voice", version: 2 }],
      connectors: [
        { connector: "jira", operation: "search", input: {} },
        { connector: "github", operation: "issues", input: {} },
        { connector: "jira", operation: "issue", input: {} },
      ],
      sources: [
        { title: "A", url: "https://a.dev", query: "q" },
        { title: "B", url: "https://b.dev", query: "q" },
      ],
      page: { pageId: "pag_1", version: 3, title: "Decoder results", format: "html" },
    });
    expect(chips.map((c) => c.label)).toEqual(["Loaded brand-voice", "Read Jira twice", "Read GitHub", "2 web sources", "Wrote Decoder results"]);
  });

  it("has none for a plain answer", () => {
    expect(activityChips({})).toEqual([]);
  });

  it("names connectors as people read them", () => {
    expect(connectorLabel("linear")).toBe("Linear");
  });

  it("offers only links with real web hosts in the source sheet", () => {
    const sources = [
      { title: "Pricing", url: "https://www.nexus.inc/pricing?x=1", query: "pricing" },
      { title: "Invalid link", url: "javascript:alert(1)", query: "pricing" },
      { title: "Disguised authority", url: "https://nexus.inc@example.com/", query: "pricing" },
    ];
    expect(activityChips({ sources })).toEqual([
      { key: "web", kind: "web", label: "1 web source", sources: [{ ...sources[0], host: "nexus.inc" }] },
    ]);
  });
});
