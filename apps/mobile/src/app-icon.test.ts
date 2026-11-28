import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const app = new URL("../", import.meta.url);
const { expo } = JSON.parse(readFileSync(new URL("app.json", app), "utf8"));

// A PNG's size, and whether it can hold transparency: an alpha channel (colour types 4 and 6) or a tRNS chunk.
function png(path: string) {
  const file = readFileSync(new URL(path, app));
  let transparent = [4, 6].includes(file[25]);
  for (let at = 8; at < file.length; at += 12 + file.readUInt32BE(at)) {
    if (file.toString("latin1", at + 4, at + 8) === "tRNS") transparent = true;
  }
  return { format: file.toString("latin1", 1, 4), width: file.readUInt32BE(16), height: file.readUInt32BE(20), transparent };
}

describe("app icon", () => {
  // scripts/app-icon.mjs draws it. The App Store refuses an icon with transparency.
  it.skipIf(!existsSync(new URL(expo.icon, app)))("is an opaque 1024 square where the App Store reads it", () => {
    for (const path of [expo.icon, expo.ios.icon.light]) {
      expect(png(path), path).toEqual({ format: "PNG", width: 1024, height: 1024, transparent: false });
    }
  });
});
