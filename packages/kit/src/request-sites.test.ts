import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { requestSites, requestSitesIn } from "./request-sites";

const kinds = (source: string, file?: string) => requestSites(source, file).map((site) => site.kind);

describe("the request sites of a source", () => {
  it("finds a call that sends a request, however it is reached", () => {
    expect(kinds('await fetch("https://a.invalid")')).toEqual(["fetch"]);
    expect(kinds("deps.fetch(url, init)")).toEqual(["fetch"]);
    expect(kinds("globalThis.fetch(url)")).toEqual(["fetch"]);
    expect(kinds("navigator.sendBeacon(url, body)")).toEqual(["beacon"]);
    expect(kinds("await FileSystem.uploadAsync(url, file)")).toEqual(["transfer"]);
    expect(kinds("new XMLHttpRequest()")).toEqual(["xhr"]);
    expect(kinds('new WebSocket("wss://a.invalid")')).toEqual(["socket"]);
    expect(kinds('new EventSource("/stream")')).toEqual(["event-source"]);
  });

  it("finds a call of the sender through syntax that does not change what it calls", () => {
    expect(kinds('deps["fetch"](url)')).toEqual(["fetch"]);
    expect(kinds("deps.fetch?.(url)")).toEqual(["fetch"]);
    expect(kinds("deps.fetch!(url)")).toEqual(["fetch"]);
    expect(kinds("(deps.fetch as Fetch)(url)")).toEqual(["fetch"]);
    expect(kinds("(0, fetch)(url)")).toEqual(["fetch"]);
    expect(kinds("(0, globalThis.fetch)(url)")).toEqual(["fetch"]);
  });

  // The holes of a scan that only sees a call: the sender is taken into a name, and every later call is of that name.
  it("finds a value that is the sender, as a site of its own", () => {
    expect(kinds("const get = deps.fetch;")).toEqual(["fetch-ref"]);
    expect(kinds('const get = deps["fetch"];')).toEqual(["fetch-ref"]);
    expect(kinds("const { fetch: get } = deps;")).toEqual(["fetch-ref"]);
    expect(kinds("const { fetch } = deps;")).toEqual(["fetch-ref"]);
    expect(kinds("function f({ fetch }: Deps) { return 1; }")).toEqual(["fetch-ref"]);
    expect(kinds("const get = fetch;")).toEqual(["fetch-ref"]);
    expect(kinds("const get = (0, fetch);")).toEqual(["fetch-ref"]);
    expect(kinds("const get = fetch.bind(globalThis);")).toEqual(["fetch-ref"]);
    expect(kinds("fetch.call(globalThis, url);")).toEqual(["fetch-ref"]);
    expect(kinds("deps.fetch.apply(deps, [url]);")).toEqual(["fetch-ref"]);
    expect(kinds("const deps = { fetch, now };")).toEqual(["fetch-ref"]);
    expect(kinds("const deps = { send: fetch };")).toEqual(["fetch-ref"]);
    expect(kinds("run(fetch);")).toEqual(["fetch-ref"]);
    expect(kinds("export async function served(fetcher: typeof fetch = fetch) { return fetcher(u); }")).toEqual(["fetch-ref"]);
    // A reference and a call are two sites, in the order they appear.
    expect(kinds("const get = deps.fetch; deps.fetch(url);")).toEqual(["fetch-ref", "fetch"]);
  });

  // A sender a default hands on, and one an import renames: every later call is of another name.
  it("finds a sender that a default binds, and one an import renames", () => {
    expect(kinds("const { send = fetch } = opts; send(url);")).toEqual(["fetch-ref"]);
    expect(kinds("function f({ get = fetch } = {}) { return get(url); }")).toEqual(["fetch-ref"]);
    expect(kinds("const [first = fetch] = list;")).toEqual(["fetch-ref"]);
    expect(kinds("const { fetch: send = other } = deps;")).toEqual(["fetch-ref"]);
    expect(kinds('import { fetch as get } from "undici"; get(url);')).toEqual(["fetch-ref"]);
    expect(kinds('export { fetch as send } from "undici";')).toEqual(["fetch-ref"]);
    // A heritage clause holds an expression, though the syntax tree calls it a type.
    expect(kinds("class Loader extends make(fetch) {}")).toEqual(["fetch-ref"]);
    expect(kinds("class Loader extends deps.fetch {}")).toEqual(["fetch-ref"]);
  });

  it("finds a Gateway path, in a literal, a template or a concatenation", () => {
    expect(kinds('const p = "/v1/spend";')).toEqual(["path"]);
    expect(kinds("const p = `${base}/v1/teams/${slug}`;")).toEqual(["path"]);
    expect(kinds('const p = base + "/v1/spend";')).toEqual(["path"]);
  });

  it("reports the line of each site, in order", () => {
    expect(requestSites('const a = 1;\nfetch("/x");\n\nconst p = "/v1/spend";')).toEqual([
      { kind: "fetch", line: 2 },
      { kind: "path", line: 4 },
    ]);
    expect(requestSites("const a = 1;\nconst get = deps.fetch;")).toEqual([{ kind: "fetch-ref", line: 2 }]);
  });

  // The control: what is not a site. Each of these has the words of one.
  it("ignores a comment, a type, a declaration and a path that is not Gateway's", () => {
    expect(kinds('// fetch("/v1/spend")\n/* new WebSocket(url) and "/v1/spend" */')).toEqual([]);
    expect(kinds("const f: typeof fetch = other;\ntype Send = typeof deps.fetch;\ninterface Deps { fetch: typeof fetch }")).toEqual([]);
    // A name a declaration gives, and a property a literal names, are not the sender.
    expect(kinds("function fetch() {}\nconst fetch2 = 1;\nclass A { fetch() {} fetch = 1; }\nconst o = { fetch: impl, fetch() {} };")).toEqual([]);
    expect(kinds("import { fetch } from 'x'; import fetch2 from 'y'; export { other };")).toEqual([]);
    // An import renamed to hold a type, and a type that is named in a heritage clause, are not a sender.
    expect(kinds("import type { fetch as Sender } from 'x'; import { type fetch as Sent } from 'y'; export type { fetch as Out } from 'z';")).toEqual([]);
    expect(kinds("interface Loader extends Base<typeof fetch> {}\nclass Page implements Shape<typeof fetch> {}\ntype Pick = import('x').fetch;")).toEqual([]);
    // A default that is not the sender.
    expect(kinds("const { send = other } = opts; function f({ get = send } = {}) { return get(url); }")).toEqual([]);
    expect(kinds('const a = <Loader fetch={impl} />;', "view.tsx")).toEqual([]);
    expect(kinds('if (typeof fetch === "function" && typeof deps.fetch !== "undefined") {}')).toEqual([]);
    expect(kinds('const docs = "/docs/v1.md"; const app = `${ui}/app/device`;')).toEqual([]);
    // A method every object has is not a site: the names are looked up in a map, not in an object that inherits them.
    expect(kinds("value.toString(); new Object(); item.constructor(); other.hasOwnProperty('x')")).toEqual([]);
  });

  it("reads a JavaScript source as it is, with no types", () => {
    expect(kinds("const get = deps.fetch; (0, fetch)(url);", "a.mjs")).toEqual(["fetch-ref", "fetch"]);
    expect(kinds("export default <Loader get={deps.fetch} />;", "a.jsx")).toEqual(["fetch-ref"]);
  });
});

describe("the request sites of a folder", () => {
  /** What `requestSitesIn` finds in a folder made of `files`. */
  function folder(files: Record<string, string>) {
    const root = mkdtempSync(join(tmpdir(), "request-sites-"));
    try {
      for (const [file, source] of Object.entries(files)) {
        mkdirSync(join(root, file, ".."), { recursive: true });
        writeFileSync(join(root, file), source);
      }
      return requestSitesIn(root);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  }

  it("reads every flavour of TypeScript and JavaScript, and leaves out tests and declarations", () => {
    expect(
      folder({
        "a.ts": 'fetch("/x");',
        "b/c.tsx": 'const p = "/v1/spend";',
        "d.js": "const get = deps.fetch;",
        "e.mjs": 'await fetch("/e");',
        "f.cjs": 'module.exports = () => fetch("/f");',
        "g.jsx": "export const G = () => <Loader get={deps.fetch} />;",
        "h.mts": 'await fetch("/h");',
        // A test, a declaration, a file with no site and a file of another kind are not listed.
        "a.test.ts": 'fetch("/v1/spend")',
        "i.test.mjs": 'fetch("/v1/spend")',
        "j.d.ts": 'declare const p: "/v1/spend";',
        "k.ts": "export const k = 1;",
        "l.json": '{ "path": "/v1/spend" }',
      }),
    ).toEqual({
      "a.ts": ["fetch"],
      "b/c.tsx": ["path"],
      "d.js": ["fetch-ref"],
      "e.mjs": ["fetch"],
      "f.cjs": ["fetch"],
      "g.jsx": ["fetch-ref"],
      "h.mts": ["fetch"],
    });
  });
});
