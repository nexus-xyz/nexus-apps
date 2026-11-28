#!/usr/bin/env node
/**
 * The native `nexus` CLI's manifest for a release in the downloads bucket: `cli/<channel>/<version>/latest.json` (the
 * version's own, written once) and its copy `cli/<channel>/latest.json` (the channel's pointer, what a rollback rewrites).
 * Used by .github/workflows/cli-release.yml (`release`); served at https://downloads.nexus.inc.
 *
 *   node scripts/cli-manifest.mjs --dir <binaries> --version 0.9.0 --build 0.9.0+abc1234 --channel stable \
 *     --base https://downloads.nexus.inc/cli/stable/0.9.0 [--commit <sha>] [--require darwin-arm64,linux-x64,…] > latest.json
 *
 * `<dir>` holds one file per platform, `nexus-<target>` (`nexus-win32-x64.exe` on Windows), with the target names the
 * Gateway and `nexus update` use (`<os>-<arch>`: darwin-arm64, darwin-x64, linux-x64, linux-arm64, win32-x64). Every
 * `--require`d target must be there, or nothing is written (exit 1).
 *
 * Schema 1 (fields are only ever added; a reader ignores the ones it does not know):
 *   {
 *     "schema": 1,
 *     "name": "nexus",
 *     "channel": "stable",
 *     "version": "0.9.0",                  the release, VERSION at its cli-v* tag
 *     "build": "0.9.0+abc1234",            what `nexus --version` prints
 *     "commit": "<40 hex>",
 *     "published_at": "2026-10-29T06:00:00Z",
 *     "targets": {
 *       "darwin-arm64": {"file": "nexus-darwin-arm64", "url": "https://…/nexus-darwin-arm64", "sha256": "<64 hex>", "size": 123},
 *       …
 *     }
 *   }
 * Each `targets` entry is written on one line, so a shell script can read one with grep, as the desktop installer
 * reads its manifest. The Gateway reads `cli/<channel>/<its own version>/latest.json` (NEXUS_CLI_DOWNLOADS).
 */
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

function arg(name) {
  const index = process.argv.indexOf(`--${name}`);
  return index > 0 ? process.argv[index + 1] : undefined;
}

const dir = arg("dir");
const version = arg("version");
const build = arg("build") ?? version;
const channel = arg("channel");
const commit = arg("commit");
const base = arg("base")?.replace(/\/$/, "");
const required = (arg("require") ?? "").split(",").filter(Boolean);
if (!dir || !version || !channel || !base) {
  console.error(
    "usage: cli-manifest.mjs --dir <binaries> --version <x.y.z[-pre]> --build <x.y.z+sha> --channel <stable> --base <https://…/cli/<channel>/<version>> [--commit <sha>] [--require <target,…>]",
  );
  process.exit(2);
}

// nexus-<os>-<arch>, with .exe on Windows; the Gateway's valid_target: lowercase letters and digits, one hyphen.
const NAME = /^nexus-([a-z0-9]{1,16}-[a-z0-9]{1,16})(\.exe)?$/;

const targets = {};
for (const file of readdirSync(dir).sort()) {
  const match = NAME.exec(file);
  const path = join(dir, file);
  if (!match || !statSync(path).isFile()) continue;
  const bytes = readFileSync(path);
  targets[match[1]] = {
    file,
    url: `${base}/${file}`,
    sha256: createHash("sha256").update(bytes).digest("hex"),
    size: bytes.length,
  };
}

const missing = required.filter((target) => !targets[target]);
if (missing.length > 0) {
  console.error(`cli-manifest: no binary for ${missing.join(", ")} in ${dir}`);
  process.exit(1);
}

const head = {
  schema: 1,
  name: "nexus",
  channel,
  version,
  build,
  ...(commit ? { commit } : {}),
  published_at: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
};
const lines = Object.entries(head).map(([key, value]) => `  ${JSON.stringify(key)}: ${JSON.stringify(value)},`);
const entries = Object.entries(targets).map(([target, entry]) => `    ${JSON.stringify(target)}: ${JSON.stringify(entry)}`);
process.stdout.write(`{\n${lines.join("\n")}\n  "targets": {\n${entries.join(",\n")}\n  }\n}\n`);
