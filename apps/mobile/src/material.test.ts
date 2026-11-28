import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const src = new URL("./", import.meta.url);
const files = (readdirSync(src, { recursive: true }) as string[]).filter((file) => /\.tsx?$/.test(file) && !file.endsWith(".test.ts"));
const importing = (pattern: RegExp) => files.filter((file) => pattern.test(readFileSync(new URL(file, src), "utf8")));

// Jetpack Compose is Android only: a file iOS bundles must not import it, or iOS would ask for views it does not have.
describe("material", () => {
  it("reaches the bundle only through .android.tsx files", () => {
    expect(importing(/from "@expo\/ui\/jetpack-compose/).filter((file) => file !== "material.tsx" && !file.endsWith(".android.tsx"))).toEqual([]);
    expect(importing(/from "@\/material"/).filter((file) => !file.endsWith(".android.tsx"))).toEqual([]);
  });
});
