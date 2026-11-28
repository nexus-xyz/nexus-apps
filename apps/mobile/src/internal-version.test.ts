// The internal channel's version written into app.json. A wrong versionCode cannot be taken back: the
// channel serves it, and Android refuses an update whose code does not rise.
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

// @ts-expect-error -- a plain .mjs script with no types of its own.
import { parseArgs, parseCode, withInternalVersion } from "../scripts/internal-version.mjs";

const app = { expo: { name: "Nexus", version: "0.1.0", android: { package: "xyz.nexus.android" } } };

describe("withInternalVersion", () => {
  it("sets the version and the versionCode, and keeps the rest", () => {
    const next = withInternalVersion(app, "0.1.0-internal.42", 42);
    expect(next.expo.version).toBe("0.1.0-internal.42");
    expect(next.expo.android.versionCode).toBe(42);
    expect(next.expo.android.package).toBe("xyz.nexus.android");
    expect(next.expo.name).toBe("Nexus");
  });

  it("does not change the config it was given", () => {
    withInternalVersion(app, "0.1.0-internal.42", 42);
    expect(app.expo.version).toBe("0.1.0");
    expect("versionCode" in app.expo.android).toBe(false);
  });

  it("takes a plain version too", () => {
    expect(withInternalVersion(app, "1.2.3", 1).expo.version).toBe("1.2.3");
  });

  it("refuses a version that is not a version", () => {
    for (const bad of ["", "1.2", "v1.2.3", "1.2.3-internal", "1.2.3-rc.1", undefined, 123]) {
      expect(() => withInternalVersion(app, bad as string, 1)).toThrow(/--version/);
    }
  });

  it("refuses a versionCode Android would not take", () => {
    for (const bad of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(() => withInternalVersion(app, "1.2.3", bad)).toThrow(/--code/);
    }
  });

  it("refuses an app.json with no expo object", () => {
    for (const bad of [{}, { expo: null }, { expo: "nexus" }, null]) {
      expect(() => withInternalVersion(bad, "1.2.3", 1)).toThrow(/expo/);
    }
  });
});

describe("parseArgs", () => {
  it("reads --flag value pairs", () => {
    expect(parseArgs(["--version", "1.2.3", "--code", "7"])).toEqual(
      new Map([
        ["--version", "1.2.3"],
        ["--code", "7"],
      ]),
    );
  });

  it("refuses a flag with no value, instead of a silent undefined", () => {
    expect(() => parseArgs(["--version"])).toThrow(/pairs/);
    expect(() => parseArgs(["--version", "--code", "7"])).toThrow(/pairs/);
    expect(() => parseArgs(["1.2.3"])).toThrow(/pairs/);
  });
});

describe("parseCode", () => {
  it("reads plain digits", () => {
    expect(parseCode("42")).toBe(42);
  });

  it("refuses what Number() would quietly accept", () => {
    for (const bad of ["1e1", "0x10", " 7", "7 ", "07", "0", "-1", "1.5", "", undefined]) {
      expect(() => parseCode(bad)).toThrow(/--code/);
    }
  });
});

// The CLI itself, from a copy in a folder whose name holds a space and a #: import.meta.url escapes both, which once
// made the script skip its edit and exit 0 while the workflow shipped the committed version.
describe("the CLI", () => {
  const script = fileURLToPath(new URL("../scripts/internal-version.mjs", import.meta.url));
  let root = "";

  afterEach(() => {
    if (root) rmSync(root, { recursive: true, force: true });
  });

  function run(...args: string[]) {
    root = mkdtempSync(join(tmpdir(), "internal version #"));
    mkdirSync(join(root, "scripts"));
    copyFileSync(script, join(root, "scripts", "internal-version.mjs"));
    writeFileSync(join(root, "app.json"), `${JSON.stringify(app, null, 2)}\n`);
    const result = spawnSync(process.execPath, [join(root, "scripts", "internal-version.mjs"), ...args], {
      encoding: "utf8",
    });
    return { status: result.status, stderr: result.stderr, app: JSON.parse(readFileSync(join(root, "app.json"), "utf8")) };
  }

  it("writes the version into app.json", () => {
    const result = run("--version", "0.1.0-internal.42", "--code", "42");
    expect(result.status).toBe(0);
    expect(result.app.expo.version).toBe("0.1.0-internal.42");
    expect(result.app.expo.android.versionCode).toBe(42);
  });

  it("fails, and leaves app.json alone, on a code that is not plain digits", () => {
    const result = run("--version", "0.1.0-internal.10", "--code", "1e1");
    expect(result.status).toBe(1);
    expect(result.stderr).toMatch(/--code/);
    expect(result.app).toEqual(app);
  });
});
