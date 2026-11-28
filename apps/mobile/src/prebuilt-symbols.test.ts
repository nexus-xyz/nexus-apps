// A precompiled framework that imports a symbol another one lacks ends the app at launch. The check stops
// the build then, and fails closed when it finds nothing to check.
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// @ts-expect-error -- a plain .mjs script with no types of its own.
import { findArchives, missingSymbols, nmSymbols, problems } from "../scripts/prebuilt-symbols-lib.mjs";

const EMIT = "_$s15ExpoModulesCore12EventEmitterPAAE4emit5eventySS_tF";
const EVENT_RUNTIME = "_$s15ExpoModulesCore12EventEmitterPAAE12eventRuntime03forD00aB3JSI010JavaScriptG0CSgSS_tF";

const framework = (defines: string[], imports: string[]) => ({ defines: new Set(defines), imports: new Set(imports) });
const found = (name: string) => ({ name, archive: `node_modules/x/${name}.tar.gz`, binary: `/tmp/${name}` });

describe("missingSymbols", () => {
  it("finds an import the other framework does not define: TestFlight build 3's crash", () => {
    const frameworks = new Map([
      ["ExpoModulesCore", framework([EMIT], [])],
      ["ExpoFileSystem", framework([], [EMIT, EVENT_RUNTIME])],
    ]);
    expect(missingSymbols(frameworks)).toEqual([{ framework: "ExpoFileSystem", from: "ExpoModulesCore", symbol: EVENT_RUNTIME }]);
  });

  it("ignores the system's imports and a framework's own symbols", () => {
    const frameworks = new Map([
      ["ExpoModulesCore", framework([EMIT], ["_$s10Foundation4DataVN"])],
      ["ExpoFileSystem", framework(["_$s14ExpoFileSystem4FileCN"], [EMIT, "_$s14ExpoFileSystem4FileCN", "_objc_msgSend"])],
    ]);
    expect(missingSymbols(frameworks)).toEqual([]);
  });
});

describe("nmSymbols", () => {
  it("reads the symbol from defined and undefined lines alike", () => {
    const output = `000000000008104c T ${EMIT}\n                 U ${EVENT_RUNTIME}\n\n                 U _objc_msgSend\n`;
    expect(nmSymbols(output)).toEqual(new Set([EMIT, EVENT_RUNTIME, "_objc_msgSend"]));
  });
});

describe("problems", () => {
  it("fails closed when the scan found no framework at all", () => {
    expect(problems([], new Map())).toEqual([expect.stringMatching(/found no precompiled Expo framework/)]);
  });

  it("fails when an archive holds no binary where the check reads it", () => {
    const frameworks = new Map([["ExpoModulesCore", framework([EMIT], [])]]);
    const result = problems([found("ExpoModulesCore"), { ...found("ExpoFont"), binary: null }], frameworks);
    expect(result).toEqual([expect.stringMatching(/ExpoFont\.tar\.gz holds no ios-arm64 binary/)]);
  });

  it("fails on two copies of one framework", () => {
    const frameworks = new Map([["ExpoModulesCore", framework([EMIT], [])]]);
    const twice = [found("ExpoModulesCore"), { ...found("ExpoModulesCore"), archive: "node_modules/expo/node_modules/x/ExpoModulesCore.tar.gz" }];
    expect(problems(twice, frameworks)).toEqual([expect.stringMatching(/^two copies of ExpoModulesCore/)]);
  });

  it("names the missing import, and passes when every import is defined", () => {
    const broken = new Map([
      ["ExpoModulesCore", framework([EMIT], [])],
      ["ExpoFileSystem", framework([], [EVENT_RUNTIME])],
    ]);
    const both = [found("ExpoModulesCore"), found("ExpoFileSystem")];
    expect(problems(both, broken)).toEqual([`ExpoFileSystem imports ${EVENT_RUNTIME}, which ExpoModulesCore does not define`]);
    const fixed = new Map([...broken, ["ExpoModulesCore", framework([EMIT, EVENT_RUNTIME], [])]]);
    expect(problems(both, fixed)).toEqual([]);
  });
});

describe("findArchives", () => {
  let root: string;
  afterEach(() => rmSync(root, { recursive: true, force: true }));

  const archive = (pkg: string, name: string) => {
    const dir = join(root, "node_modules", pkg, "prebuilds", "output", "release", "xcframeworks");
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, `${name}.tar.gz`), "");
    return join(dir, `${name}.tar.gz`);
  };

  it("finds top-level, scoped and nested packages' release archives, and nothing else", () => {
    root = mkdtempSync(join(tmpdir(), "prebuilt-symbols-test-"));
    const expected = [
      archive("expo-modules-core", "ExpoModulesCore"),
      archive("@scope/thing", "Thing"),
      archive("expo/node_modules/expo-modules-core", "ExpoModulesCore"),
    ];
    const debug = join(root, "node_modules", "expo-font", "prebuilds", "output", "debug", "xcframeworks");
    mkdirSync(debug, { recursive: true });
    writeFileSync(join(debug, "ExpoFont.tar.gz"), "");
    expect(findArchives(join(root, "node_modules")).sort()).toEqual(expected.sort());
  });

  it("follows a package linked into node_modules, as a workspace or a file: dependency is", () => {
    root = mkdtempSync(join(tmpdir(), "prebuilt-symbols-test-"));
    const outside = join(root, "packages", "expo-font", "prebuilds", "output", "release", "xcframeworks");
    mkdirSync(outside, { recursive: true });
    writeFileSync(join(outside, "ExpoFont.tar.gz"), "");
    mkdirSync(join(root, "node_modules"));
    symlinkSync(join(root, "packages", "expo-font"), join(root, "node_modules", "expo-font"), "dir");
    symlinkSync(join(root, "nowhere"), join(root, "node_modules", "broken-link"), "dir");
    const found = findArchives(join(root, "node_modules"));
    expect(found).toEqual([join(root, "node_modules", "expo-font", "prebuilds", "output", "release", "xcframeworks", "ExpoFont.tar.gz")]);
  });

  it("stops at a link back to a folder above it, and counts the package once", () => {
    root = mkdtempSync(join(tmpdir(), "prebuilt-symbols-test-"));
    const font = join(root, "packages", "expo-font");
    mkdirSync(join(font, "prebuilds", "output", "release", "xcframeworks"), { recursive: true });
    writeFileSync(join(font, "prebuilds", "output", "release", "xcframeworks", "ExpoFont.tar.gz"), "");
    mkdirSync(join(root, "node_modules"));
    symlinkSync(font, join(root, "node_modules", "expo-font"), "dir");
    // The linked package's own node_modules links back to the root, whose node_modules holds the package again.
    mkdirSync(join(font, "node_modules"));
    symlinkSync(root, join(font, "node_modules", "loop"), "dir");
    expect(findArchives(join(root, "node_modules"))).toEqual([
      join(root, "node_modules", "expo-font", "prebuilds", "output", "release", "xcframeworks", "ExpoFont.tar.gz"),
    ]);
  });
});
