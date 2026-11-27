import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const src = new URL("./", import.meta.url);
const files = (readdirSync(src, { recursive: true }) as string[]).filter((file) => /\.tsx?$/.test(file) && !["controls.tsx", "switch.tsx"].includes(file));

// React Native's own Switch and TextInput type-check like the wrappers but draw iOS green and blue.
const DIRECT = /import\s*\{[^}]*\b(Switch|TextInput)\b[^}]*\}\s*from\s*"react-native"/;

describe("controls", () => {
  it("are the only Switch and TextInput the app imports from react-native", () => {
    expect(files.filter((file) => DIRECT.test(readFileSync(new URL(file, src), "utf8")))).toEqual([]);
  });
});
