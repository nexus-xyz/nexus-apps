import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createArtifactsClient, createCoreClient } from "@nexus/kit";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import { requestSitesIn } from "../../../packages/kit/src/request-sites";
import { docsSource } from "./docs";

/**
 * The phone app's calls to Gateway against `openapi.json`. The app builds none of its own: every Gateway call is a
 * method of `@nexus/kit`'s two clients (`auth.tsx` makes them), and `kit-internal/src/contract.test.ts` holds every method of
 * those to the contract. This test holds the rest of the chain, which nothing else runs (the app's typecheck is not in
 * CI, only its logic tests are):
 * - the app sends no request except the two it is known to: the kit's `fetch` option, and Nexus UI's docs;
 * - every method it calls on a client is one the kit's client has, so a method renamed in the kit fails here.
 */

const SRC = fileURLToPath(new URL(".", import.meta.url));
const BASE = "http://gateway.invalid";

/** The three names the app gives the kit's clients: `core` and `artifacts` from `useAuth()`, `client` in the share helpers. */
type Held = "core" | "artifacts" | "client";
const HELD: readonly string[] = ["core", "artifacts", "client"];

/** What a source reads off `core`, `artifacts` and `client` (alone, or the last name of a path such as `auth.core`). */
function clientMembers(source: string, file = "source.tsx"): { object: Held; name: string }[] {
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, file.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const found: { object: Held; name: string }[] = [];
  const visit = (node: ts.Node) => {
    if (ts.isPropertyAccessExpression(node)) {
      const held = ts.isIdentifier(node.expression) ? node.expression.text : ts.isPropertyAccessExpression(node.expression) ? node.expression.name.text : "";
      if (HELD.includes(held)) found.push({ object: held as Held, name: node.name.text });
    }
    ts.forEachChild(node, visit);
  };
  visit(tree);
  return found;
}

const core = new Set(Object.keys(createCoreClient({ baseUrl: BASE })));
/** The app has a list it calls `artifacts`: a list's methods are not the client's. */
const lists = new Set(Object.getOwnPropertyNames(Array.prototype));
const artifacts = new Set([...Object.keys(createArtifactsClient({ baseUrl: BASE })), ...lists]);

/** The members read that the kit's client behind that name does not have, as `object.name`. */
const lacking = (members: { object: Held; name: string }[]) => members.filter(({ object, name }) => !(object === "core" ? core : artifacts).has(name)).map(({ object, name }) => `${object}.${name}`);

/** Every source of the app, by path from `src/`: its tests and declarations left out. */
const sources = (readdirSync(SRC, { recursive: true }) as string[]).filter((file) => /\.tsx?$/.test(file) && !/\.(test|d)\.tsx?$/.test(file)).sort();

describe("the phone app's calls to Gateway against openapi.json", () => {
  it("sends no request of its own but the kit's fetch option and Nexus UI's docs", () => {
    expect(requestSitesIn(SRC)).toEqual({
      // `watching` in `auth.tsx` wraps the clients' `fetch` to report a 401: the request is the kit's, passed on as it came.
      "auth.tsx": ["fetch"],
      // `docs.tsx` reads Nexus UI's Markdown docs from its origin: a page of the site, not a Gateway operation.
      "app/(app)/(main)/docs.tsx": ["fetch"],
      // `dictation.tsx` opens the speech-to-text socket whose address the Gateway's `VoiceToken` gives: not a Gateway operation.
      "dictation.tsx": ["socket"],
    });
  });

  it("passes the kit's request on unchanged, and asks Nexus UI's site for the docs, not Gateway's API", () => {
    expect(readFileSync(join(SRC, "auth.tsx"), "utf8").match(/\bfetch\([^)]*\)/g)).toEqual(["fetch(input, init)"]);
    expect(readFileSync(join(SRC, "app/(app)/(main)/docs.tsx"), "utf8").match(/\bfetch\(\w+\(/g)).toEqual(["fetch(docsSource("]);
    for (const path of [undefined, "guides", "guides/github and teams"]) expect(new URL(docsSource("https://ui.invalid", path)).pathname).toMatch(/^\/docs(\/.*)?\.md$/);
  });

  it("calls, on the kit's clients, only methods they have", () => {
    const members = sources.flatMap((file) => clientMembers(readFileSync(join(SRC, file), "utf8"), file).map((member) => ({ ...member, file })));
    // The calls are read from the source: dozens, across the screens, on the three names.
    expect(members.length).toBeGreaterThan(80);
    expect(new Set(members.map((member) => member.object))).toEqual(new Set(["core", "artifacts", "client"]));
    expect(lacking(members)).toEqual([]);
  });

  // The control: each check above can fail. A method the kit does not have, on any of the three names, is lacking; a
  // list's own method on `artifacts` is not; and the request sites are read from the syntax tree (kit/src/request-sites.test.ts).
  it("lacks a method the kit does not have, and nothing a list or a client has", () => {
    const source = "core.me(); core.madeUp(); auth.core?.spend(); artifacts.pages(); artifacts.filter(Boolean); artifacts.madeUp(); client.sharePage(); client.alsoMadeUp(); other.core.moved();";
    expect(lacking(clientMembers(source))).toEqual(["core.madeUp", "artifacts.madeUp", "client.alsoMadeUp", "core.moved"]);
    // A method of the other client is the other client's: `core` has no `sharePage`.
    expect(lacking(clientMembers("core.sharePage(); core.me();"))).toEqual(["core.sharePage"]);
  });
});
