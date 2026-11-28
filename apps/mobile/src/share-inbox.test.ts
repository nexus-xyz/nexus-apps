import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { APP_GROUP, contentsOf, destinationsFor, draftOf, INBOX_LIMITS, parseImport, skippedLine, sortOut, withExisting } from "./share-inbox";

const NOW = Date.parse("2026-11-24T12:00:00Z");
const DAY = 24 * 60 * 60 * 1000;

const json = (patch: Record<string, unknown> = {}) =>
  JSON.stringify({ version: 1, id: "A1", createdAt: new Date(NOW - 1000).toISOString(), account: "acct", text: "Read this", urls: [], images: [], skipped: [], ...patch });

describe("parseImport", () => {
  it("reads what the extension writes", () => {
    const shared = parseImport(
      "A1",
      json({ urls: ["https://example.com/a"], images: [{ file: "image-1.heic", name: "IMG_0001.HEIC", type: "image/heic", bytes: 2048 }], skipped: [{ name: "report.pdf", reason: "unsupported" }] }),
    );
    expect(shared).toEqual({
      id: "A1",
      createdAt: NOW - 1000,
      account: "acct",
      text: "Read this",
      urls: ["https://example.com/a"],
      images: [{ file: "image-1.heic", name: "IMG_0001.HEIC", type: "image/heic", bytes: 2048 }],
      skipped: [{ name: "report.pdf", reason: "unsupported" }],
    });
  });

  it("refuses what is not an import", () => {
    expect(parseImport("A1", "{")).toBeNull();
    expect(parseImport("A1", json({ version: 2 }))).toBeNull();
    // The folder's name and the file's id must agree, and a folder name never walks out of the inbox.
    expect(parseImport("A2", json())).toBeNull();
    expect(parseImport("../A1", json({ id: "../A1" }))).toBeNull();
    expect(parseImport("A1", json({ createdAt: "yesterday" }))).toBeNull();
    expect(parseImport("A1", json({ text: "  " }))).toBeNull();
  });

  it("keeps web links only, and image files only in the import's own folder", () => {
    const shared = parseImport(
      "A1",
      json({
        urls: ["https://a.dev/x", "https://a.dev/x", "file:///private/var/secret.txt", "javascript:alert(1)", 7],
        images: [
          { file: "../account.json", name: "x", bytes: 10 },
          { file: "image-2.jpg", name: "x", bytes: INBOX_LIMITS.imageBytes + 1 },
          { file: "image-3.png", bytes: 10 },
        ],
      }),
    );
    expect(shared?.urls).toEqual(["https://a.dev/x"]);
    expect(shared?.images).toEqual([{ file: "image-3.png", name: "Photo", type: undefined, bytes: 10 }]);
  });

  it("cuts text and lists to the inbox's limits", () => {
    const shared = parseImport("A1", json({ text: "x".repeat(INBOX_LIMITS.textCharacters + 5), urls: Array.from({ length: 20 }, (_, n) => `https://a.dev/${n}`) }));
    expect(shared?.text).toHaveLength(INBOX_LIMITS.textCharacters);
    expect(shared?.urls).toHaveLength(INBOX_LIMITS.urls);
  });

  it("reads an import made while signed out as no one's", () => {
    expect(parseImport("A1", json({ account: null }))?.account).toBeNull();
    expect(parseImport("A1", json({ account: "" }))?.account).toBeNull();
  });
});

describe("sortOut", () => {
  it("keeps this account's and no one's imports, oldest first, and drops the rest", () => {
    const entries = [
      { id: "B", json: json({ id: "B", createdAt: new Date(NOW - 2000).toISOString() }) },
      { id: "A", json: json({ id: "A", createdAt: new Date(NOW - 5000).toISOString(), account: null }) },
      { id: "C", json: json({ id: "C", account: "someone-else" }) },
      { id: "D", json: json({ id: "D", createdAt: new Date(NOW - 8 * DAY).toISOString() }) },
      { id: "E", json: "not json" },
    ];
    const { pending, drop } = sortOut(entries, "acct", NOW);
    expect(pending.map((p) => p.id)).toEqual(["A", "B"]);
    expect(drop).toEqual([
      { id: "C", why: "other-account" },
      { id: "D", why: "expired" },
      { id: "E", why: "unreadable" },
    ]);
  });

  it("keeps an import until its seventh day ends", () => {
    const entry = (age: number) => [{ id: "A1", json: json({ createdAt: new Date(NOW - age).toISOString() }) }];
    expect(sortOut(entry(7 * DAY), "acct", NOW).pending).toHaveLength(1);
    expect(sortOut(entry(7 * DAY + 1), "acct", NOW).drop).toEqual([{ id: "A1", why: "expired" }]);
  });
});

describe("drafts", () => {
  it("puts each link after the text, unless the text already has it", () => {
    expect(draftOf({ text: "Look", urls: ["https://a.dev/1", "https://a.dev/2"] })).toBe("Look\n\nhttps://a.dev/1\nhttps://a.dev/2");
    expect(draftOf({ text: "See https://a.dev/1", urls: ["https://a.dev/1"] })).toBe("See https://a.dev/1");
    expect(draftOf({ text: "", urls: ["https://a.dev/1"] })).toBe("https://a.dev/1");
  });

  it("never overwrites an unsent draft, and adds an import once", () => {
    expect(withExisting("", "Shared")).toBe("Shared");
    expect(withExisting("My question  ", "Shared")).toBe("My question\n\nShared");
    expect(withExisting("My question\n\nShared", "Shared")).toBe("My question\n\nShared");
    expect(withExisting("My question", "")).toBe("My question");
  });
});

describe("words", () => {
  it("says what an import holds", () => {
    expect(contentsOf({ text: "a", urls: ["u", "v"], images: [{ file: "image-1.jpg", name: "a", bytes: 1 }] })).toBe("Text, 2 links and 1 photo");
    expect(contentsOf({ text: "", urls: ["u"], images: [] })).toBe("1 link");
    expect(contentsOf({ text: " ", urls: [], images: [] })).toBe("Nothing Nexus can use");
  });

  it("says what was left out and why", () => {
    expect(skippedLine([])).toBeUndefined();
    expect(skippedLine([{ name: "report.pdf", reason: "unsupported" }, { name: "RAW.dng", reason: "too-large" }])).toBe(
      "Not saved: report.pdf (Nexus takes text, links and photos only); RAW.dng (larger than 25 MB).",
    );
    const many = Array.from({ length: 5 }, (_, n) => ({ name: `p${n}.jpg`, reason: "too-many" as const }));
    expect(skippedLine(many)).toMatch(/ and 2 more items\.$/);
  });
});

describe("destinationsFor", () => {
  it("offers the Assistant only for words it takes", () => {
    const words = { text: "Hi", urls: [], images: [] };
    expect(destinationsFor(words, true)).toEqual([{ to: "chat", label: "New chat" }, { to: "assistant", label: "Assistant", why: undefined }]);
    expect(destinationsFor(words, false)[1].why).toBe("This build has no Assistant.");
    expect(destinationsFor({ ...words, images: [{ file: "image-1.jpg", name: "a", bytes: 1 }] }, true)[1].why).toMatch(/doesn't take photos/);
    expect(destinationsFor({ ...words, text: "x".repeat(8_001) }, true)[1].why).toMatch(/at most 8,000 characters/);
  });
});

/** The numbers a Swift file sets with `static let name = value`. */
function swiftConstants(path: string): Record<string, string | number> {
  const source = readFileSync(fileURLToPath(new URL(path, import.meta.url)), "utf8");
  const values: Record<string, string | number> = {};
  for (const [, name, value] of source.matchAll(/static let (\w+)\s*=\s*([^\n/]+)/g)) {
    const text = value.trim();
    values[name] = text.startsWith('"') ? JSON.parse(text) : text.split("*").reduce((n, part) => n * Number(part.trim().replace(/_/g, "")), 1);
  }
  return values;
}

describe("the app, the extension and the module agree", () => {
  const app = JSON.parse(readFileSync(fileURLToPath(new URL("../app.json", import.meta.url)), "utf8"));

  it("on the App Group", () => {
    expect(app.expo.ios.entitlements["com.apple.security.application-groups"]).toEqual([APP_GROUP]);
    expect(swiftConstants("../targets/share/ShareInbox.swift").appGroup).toBe(APP_GROUP);
    expect(swiftConstants("../modules/share-inbox/ios/ShareInboxModule.swift").appGroup).toBe(APP_GROUP);
  });

  it("on the inbox's limits", () => {
    const swift = swiftConstants("../targets/share/ShareInbox.swift");
    for (const [name, value] of Object.entries(INBOX_LIMITS)) expect([name, swift[name]]).toEqual([name, value]);
  });
});
