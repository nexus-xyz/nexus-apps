/**
 * Node-only access to the committed recipe guide MDX files under `kit/recipes/pages/`.
 * Kept separate from `@nexus/kit/recipes` so client bundles never pull in Node built-ins.
 */
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const PACKAGE_JSON = "@nexus/kit/package.json";

/** Resolve `@nexus/kit`'s `recipes/pages` directory across Node, Vitest, and Next.js server bundles. */
export function recipePagesDir(): string {
  try {
    const sibling = resolve(dirname(fileURLToPath(import.meta.url)), "..", "recipes", "pages");
    if (existsSync(sibling)) return sibling;
  } catch {
    // Bundled server chunks may rewrite `import.meta.url` to a non-file URL.
  }

  // Next.js / Turbopack server bundles relocate `import.meta.url` into `.next/server/...`,
  // so resolve `@nexus/kit/package.json` through Node's module loader instead.
  for (const base of [import.meta.url, join(process.cwd(), "package.json")]) {
    try {
      const pkg = createRequire(base).resolve(PACKAGE_JSON);
      const dir = join(dirname(pkg), "recipes", "pages");
      if (existsSync(dir)) return dir;
    } catch {
      // Try the next resolution root.
    }
  }

  throw new Error(`@nexus/kit recipe pages directory not found (resolved via ${PACKAGE_JSON})`);
}

/** Read a committed recipe guide MDX file by recipe id (e.g. `claude-code`). */
export function readRecipePage(id: string): string {
  return readFileSync(join(recipePagesDir(), `${id}.mdx`), "utf8");
}
