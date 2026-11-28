// Every precompiled Expo framework finds the Swift symbols it imports from the others.
//
// Expo ships some iOS modules as prebuilt XCFrameworks. A module built against a newer expo-modules-core than the one
// installed imports a symbol that core lacks, and iOS ends the app at launch ("DYLD Symbol missing") before any
// JavaScript runs. TestFlight build 3 crashed that way. EAS runs this after `npm ci` on an iOS build
// (`eas-build-post-install` in package.json), so a mismatched lockfile fails the build instead of the phone. It needs
// macOS's `nm` and `tar`, as EAS's iOS builder has. The logic is in `prebuilt-symbols-lib.mjs`; this file only runs it,
// so the check always runs when called and fails closed.
//
//   node scripts/prebuilt-symbols.mjs
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { findArchives, nmSymbols, problems } from "./prebuilt-symbols-lib.mjs";

function run(tool, args) {
  try {
    return execFileSync(tool, args, { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  } catch (error) {
    if (error.code === "ENOENT") throw new Error(`needs \`${tool}\` (macOS, with Xcode's command line tools)`);
    throw error;
  }
}

const app = dirname(dirname(fileURLToPath(import.meta.url)));
const work = mkdtempSync(join(tmpdir(), "prebuilt-symbols-"));
try {
  const found = [];
  const frameworks = new Map();
  for (const archive of findArchives(join(app, "node_modules"))) {
    const name = basename(archive, ".tar.gz");
    const into = join(work, String(found.length));
    mkdirSync(into);
    run("tar", ["-xzf", archive, "-C", into]);
    const binary = join(into, `${name}.xcframework`, "ios-arm64", `${name}.framework`, name);
    const exists = existsSync(binary);
    found.push({ name, archive, binary: exists ? binary : null });
    if (exists) frameworks.set(name, { defines: nmSymbols(run("nm", ["-gU", binary])), imports: nmSymbols(run("nm", ["-u", binary])) });
  }
  const errors = problems(found, frameworks);
  for (const error of errors) console.error(`prebuilt-symbols: ${error}`);
  console.log(`prebuilt-symbols: ${frameworks.size} frameworks, ${errors.length} problems`);
  if (errors.length) {
    console.error("When an import is missing, align the Expo packages: npx expo install --fix");
    process.exitCode = 1;
  }
} catch (error) {
  console.error(`prebuilt-symbols: ${error.message}`);
  process.exitCode = 1;
} finally {
  rmSync(work, { recursive: true, force: true });
}
