// The internal channel's version, written into app.json before `expo prebuild`.
//
// `expo prebuild` reads app.json, so the generated android/ carries whatever versionName and versionCode it finds
// there. The channel's build is <VERSION>-internal.<run number>, and Android's versionCode is the run number: a
// positive integer that rises with every build the channel serves. The committed app.json keeps the plain version,
// so this runs on the checkout in CI and is never committed.
//
//   node scripts/internal-version.mjs --version 0.1.0-internal.42 --code 42
import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// 0.1.0, or 0.1.0-internal.42: the plain version, or one of this channel's builds.
const VERSION = /^[0-9]+\.[0-9]+\.[0-9]+(-internal\.[0-9]+)?$/;

/** The app.json an internal build runs with. Pure, so `internal-version.test.ts` can check it. */
export function withInternalVersion(config, version, code) {
  if (typeof version !== "string" || !VERSION.test(version)) {
    throw new Error(`--version must be MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-internal.N, not ${JSON.stringify(version)}`);
  }
  if (!Number.isInteger(code) || code < 1) {
    throw new Error(`--code must be a positive integer, not ${JSON.stringify(code)}`);
  }
  if (!config || typeof config.expo !== "object" || config.expo === null) {
    throw new Error("app.json has no expo object");
  }
  return { ...config, expo: { ...config.expo, version, android: { ...config.expo.android, versionCode: code } } };
}

/** The --code text as a versionCode. Only plain digits: Number() alone also takes "1e1", "0x10" and " 7". */
export function parseCode(text) {
  if (typeof text !== "string" || !/^[1-9][0-9]*$/.test(text)) {
    throw new Error(`--code must be a positive integer, not ${JSON.stringify(text)}`);
  }
  return Number(text);
}

/** `--name value` pairs. An odd tail, or a value that is itself a flag, is an error rather than a silent undefined. */
export function parseArgs(argv) {
  const args = new Map();
  for (let i = 0; i < argv.length; i += 2) {
    const [flag, value] = [argv[i], argv[i + 1]];
    if (!flag.startsWith("--") || value === undefined || value.startsWith("--")) {
      throw new Error(`expected --flag value pairs, got ${argv.slice(i).join(" ") || "nothing"}`);
    }
    args.set(flag, value);
  }
  return args;
}

// Run only as a CLI, so the test can import the functions above without writing app.json. Compare paths, not a
// hand-built file:// URL: a checkout path with a space, # or % is escaped in import.meta.url, and a symlink is
// resolved there, so a string match would skip the edit and exit 0.
if (process.argv[1] && fileURLToPath(import.meta.url) === realpathSync(process.argv[1])) {
  try {
    const args = parseArgs(process.argv.slice(2));
    const path = join(dirname(dirname(fileURLToPath(import.meta.url))), "app.json");
    const next = withInternalVersion(JSON.parse(readFileSync(path, "utf8")), args.get("--version"), parseCode(args.get("--code")));
    writeFileSync(path, `${JSON.stringify(next, null, 2)}\n`);
    console.log(`app.json: version ${next.expo.version}, android.versionCode ${next.expo.android.versionCode}`);
  } catch (error) {
    console.error(`internal-version.mjs: ${error.message}`);
    console.error("usage: internal-version.mjs --version <version> --code <positive integer>");
    process.exit(1);
  }
}
