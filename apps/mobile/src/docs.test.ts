import { describe, expect, it } from "vitest";
import { docsLink, docsSource, splitTitle } from "./docs";

describe("docs reader", () => {
  it("reads a page's Markdown from Nexus UI", () => {
    expect(docsSource("https://ui.example/")).toBe("https://ui.example/docs.md");
    expect(docsSource("https://ui.example", "guides/github and teams")).toBe("https://ui.example/docs/guides/github%20and%20teams.md");
  });

  it("opens docs pages in the reader and the rest of the site in the browser", () => {
    expect(docsLink("/docs", "https://ui.example")).toBe("/docs");
    expect(docsLink("/docs/quickstart", "https://ui.example")).toBe("/docs?path=quickstart");
    expect(docsLink("/docs/guides/github-and-teams#setup", "https://ui.example")).toBe("/docs?path=guides%2Fgithub-and-teams");
    expect(docsLink("/docs/cli.md", "https://ui.example")).toBe("/docs?path=cli");
    expect(docsLink("/app", "https://ui.example")).toBe("https://ui.example/app");
    expect(docsLink("https://docs.anthropic.com", "https://ui.example")).toBeUndefined();
    expect(docsLink("#setup", "https://ui.example")).toBeUndefined();
    expect(docsLink("//evil.example", "https://ui.example")).toBeUndefined();
  });

  it("takes the first heading as the title", () => {
    expect(splitTitle("# Quickstart\n\n> Install.\n")).toEqual({ title: "Quickstart", body: "> Install.\n" });
    expect(splitTitle("No heading")).toEqual({ body: "No heading" });
  });
});
