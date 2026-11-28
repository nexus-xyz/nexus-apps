// The logic of `prebuilt-symbols.mjs`, with no side effects, so `src/prebuilt-symbols.test.ts` can import it.
import { existsSync, readdirSync, realpathSync, statSync } from "node:fs";
import { join } from "node:path";

/** Where a package keeps its release archives: `<package>/prebuilds/output/release/xcframeworks/<Name>.tar.gz`. */
const RELEASE = join("prebuilds", "output", "release", "xcframeworks");

/** The symbol names in `nm` output: the last word of each line ("0000… T _$s…" or "U _$s…"). */
export function nmSymbols(output) {
  return new Set(
    output
      .split("\n")
      .map((line) => line.trim().split(/\s+/).pop())
      .filter(Boolean),
  );
}

/** A folder, or a link to one: npm links a workspace or a `file:` dependency into node_modules. */
const isFolder = (path) => {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
};

/** The folders in `dir`, links to folders included. */
const folders = (dir) => readdirSync(dir).filter((name) => !name.startsWith(".") && isFolder(join(dir, name)));

/**
 * Every release archive in `nodeModules`, nested and linked packages included: npm nests a package when two versions
 * are needed, and a nested copy of a framework is the mismatch this check looks for. Each folder is read once, by its
 * real path, so a link back to a folder above it ends the walk instead of looping, and a package linked from two places
 * is not counted as two copies.
 */
export function findArchives(nodeModules, seen = new Set()) {
  const firstVisit = (path) => {
    const real = realpathSync(path);
    if (seen.has(real)) return false;
    seen.add(real);
    return true;
  };
  if (!isFolder(nodeModules) || !firstVisit(nodeModules)) return [];
  const packages = folders(nodeModules)
    .flatMap((name) =>
      name.startsWith("@") ? folders(join(nodeModules, name)).map((scoped) => join(nodeModules, name, scoped)) : [join(nodeModules, name)],
    )
    .filter(firstVisit);
  return packages.flatMap((pkg) => {
    const dir = join(pkg, RELEASE);
    const own = existsSync(dir) ? readdirSync(dir).filter((f) => f.endsWith(".tar.gz")).map((f) => join(dir, f)) : [];
    return [...own, ...findArchives(join(pkg, "node_modules"), seen)];
  });
}

/**
 * The imports each framework cannot find: `frameworks` maps a framework's name to the symbols it defines and the
 * symbols it imports. A Swift symbol names its module after `$s` with the name's length, so an import belongs to
 * another framework here when it starts with `_$s<length><name>`. An extension's member carries the name of the module
 * that declares the extension, which is the module that defines it, so those imports are checked too. Imports that are
 * not Swift (C, Objective-C) and the system's are not this check's.
 */
export function missingSymbols(frameworks) {
  const missing = [];
  for (const [name, { imports }] of frameworks) {
    for (const [other, { defines }] of frameworks) {
      if (other === name) continue;
      const prefix = `_$s${other.length}${other}`;
      for (const symbol of imports) {
        if (symbol.startsWith(prefix) && !defines.has(symbol)) missing.push({ framework: name, from: other, symbol });
      }
    }
  }
  return missing;
}

/**
 * Why the build must stop, from what the scan found: each archive with its framework's name and its binary (`null`
 * when the archive held none where expected), and the symbols of each binary read. It fails closed: no archive, an
 * archive with no binary or two copies of one framework stop the build too, so a change in Expo's layout cannot turn
 * the check into a pass.
 */
export function problems(found, frameworks) {
  if (found.length === 0) {
    return ["found no precompiled Expo framework under node_modules. If Expo moved them, update scripts/prebuilt-symbols-lib.mjs"];
  }
  const out = [];
  for (const { archive, binary } of found) {
    if (!binary) out.push(`${archive} holds no ios-arm64 binary where this check reads it`);
  }
  const byName = new Map();
  for (const { name, archive } of found) byName.set(name, [...(byName.get(name) ?? []), archive]);
  for (const [name, archives] of byName) {
    if (archives.length > 1) out.push(`two copies of ${name}: ${archives.join(", ")}`);
  }
  for (const { framework, from, symbol } of missingSymbols(frameworks)) {
    out.push(`${framework} imports ${symbol}, which ${from} does not define`);
  }
  return out;
}
