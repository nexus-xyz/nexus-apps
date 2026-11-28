import type { PageComment } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { deployDetail, initialsOf, kindOf, ofType, openThreads, provenance, threadsOf } from "./library";

const comment = (id: string, at: string, more: Partial<PageComment> = {}): PageComment => ({ id, pageId: "pag_1", mine: false, body: id, version: 1, createdAt: at, ...more });

describe("kinds and the type pills", () => {
  it("reads Markdown as a doc and HTML or React as a site", () => {
    expect([kindOf("md"), kindOf("html"), kindOf("jsx")]).toEqual(["doc", "site", "site"]);
  });

  it("keeps every row for All and a kind's rows for its pill", () => {
    const rows = [{ kind: "doc" as const }, { kind: "site" as const }, { kind: "chat" as const }];
    expect(ofType(rows, "all")).toHaveLength(3);
    expect(ofType(rows, "doc")).toEqual([{ kind: "doc" }]);
  });
});

describe("provenance", () => {
  it("says the kind, the version and how it was written", () => {
    expect(provenance("doc", 7, "chat_tool")).toBe("Doc · v7 · written in a chat");
    expect(provenance("site", 8, "restore", 6)).toBe("Site · v8 · restored from v6");
    expect(provenance("doc", 1, undefined)).toBe("Doc · v1");
  });

  it("starts a deploy's line with a capital", () => {
    expect(deployDetail("restore", 6)).toBe("Restored from v6");
    expect(deployDetail(undefined)).toBeUndefined();
  });
});

describe("threads", () => {
  const comments = [
    comment("a", "2026-11-05T09:00:00Z", { quote: "stays inside the 5 ms budget" }),
    comment("a2", "2026-11-05T09:05:00Z", { threadId: "a" }),
    comment("a1", "2026-11-05T09:02:00Z", { threadId: "a" }),
    comment("b", "2026-11-05T10:00:00Z", { resolvedAt: "2026-11-05T11:00:00Z" }),
    comment("c", "2026-11-05T08:00:00Z"),
  ];

  it("groups replies under their first comment, oldest first", () => {
    expect(threadsOf(comments)[0]!.replies.map((c) => c.id)).toEqual(["a1", "a2"]);
  });

  it("lists open threads first, newest first, then the resolved", () => {
    expect(threadsOf(comments).map((t) => t.root.id)).toEqual(["a", "c", "b"]);
    expect(openThreads(comments)).toBe(2);
  });
});

describe("initialsOf", () => {
  it("takes two words' initials, or an email's first letter", () => {
    expect(initialsOf("Ana Ruiz")).toBe("AR");
    expect(initialsOf("ana@acme.example")).toBe("A");
    expect(initialsOf("maya.chen@acme.example")).toBe("MC");
    expect(initialsOf(undefined)).toBe("?");
  });
});
