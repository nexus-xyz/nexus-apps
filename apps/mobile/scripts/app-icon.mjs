// Draws the phone app's icons and splash from the Nexus mark (`ui/public/brand/nexus-mark.svg`, the same source as
// desktop's icon, `desktop/src-tauri/icons/icon.svg`) into `assets/`:
//
//   icon.png                     iOS light, and the fallback on every platform: the mark in Black on White, opaque
//                                (the App Store refuses an icon with transparency)
//   icon-dark.png                iOS dark: the mark in White on nothing; iOS draws its dark ground behind it
//   icon-tinted.png              iOS tinted: the mark in White on Black, in grey only; iOS tints it
//   android-icon-foreground.png  Android's adaptive icon: the mark in Black on nothing,
//   android-icon-background.png  on White,
//   android-icon-monochrome.png  and the mark alone for Android 13's themed icons (the launcher reads only its alpha)
//   splash-icon.png              the splash: the mark in Black, and in White for dark mode, on nothing; app.json
//   splash-icon-dark.png         sets the ground and the width
//
// The mark keeps desktop's proportion: 480 of the 824 points of desktop's tile. iOS rounds the corners of a full-bleed
// square, so the tile is the whole icon. Android shows at most the middle 72 of an adaptive icon's 108 dp, so the tile
// is that. The splash image is the mark alone, as wide as the image. White and Black are Brand v4's `--nx-white` and
// `--nx-black`.
//
//   cd eng/factory/services/frontend/core/mobile && node scripts/app-icon.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { crc32, deflateSync } from "node:zlib";

// Nexus UI's `public/brand/nexus-mark.svg`, byte for byte; Nexus UI moved to nexus-web-service.
const source = new URL("../../desktop-gpui/assets/marks/nexus.svg", import.meta.url);
const assets = new URL("../assets/", import.meta.url);

const WHITE = [255, 255, 255];
const BLACK = [0, 0, 0];
const SIDE = 1024;
const TILE_SHARE = 480 / 824;

// The mark's polygons ([x, y] points), in its 480 × 384 box.
const svg = readFileSync(source, "utf8");
const [boxWidth, boxHeight] = svg.match(/viewBox="0 0 (\S+) (\S+)"/).slice(1).map(Number);
const polygons = [...svg.matchAll(/<polygon points="([^"]+)"/g)].map(([, list]) => {
  const numbers = list.trim().split(/\s+/).map(Number);
  return Array.from({ length: numbers.length / 2 }, (_, i) => [numbers[2 * i], numbers[2 * i + 1]]);
});
if (polygons.length !== 4) throw new Error(`expected the mark's four polygons in ${source.pathname}, found ${polygons.length}`);

// How much of each pixel the mark covers, from 0 to 1, with the mark `width` pixels wide and centred on the square.
// Each polygon is filled on its own and the spans are joined, so the overlaps fill solid whichever way each piece
// winds. Each pixel row is sampled 16 times; along a row the coverage is exact.
function cover(width) {
  const scale = width / boxWidth;
  const left = (SIDE - width) / 2;
  const top = (SIDE - boxHeight * scale) / 2;
  const samples = 16;
  const coverage = new Float64Array(SIDE * SIDE);
  for (let row = 0; row < SIDE; row++) {
    for (let sample = 0; sample < samples; sample++) {
      const y = (row + (sample + 0.5) / samples - top) / scale;
      const spans = [];
      for (const polygon of polygons) {
        const crossings = [];
        polygon.forEach(([x1, y1], i) => {
          const [x2, y2] = polygon[(i + 1) % polygon.length];
          if (y1 <= y !== y2 <= y) crossings.push(x1 + ((y - y1) * (x2 - x1)) / (y2 - y1));
        });
        crossings.sort((a, b) => a - b);
        for (let i = 0; i + 1 < crossings.length; i += 2) {
          spans.push([left + crossings[i] * scale, left + crossings[i + 1] * scale]);
        }
      }
      spans.sort((a, b) => a[0] - b[0]);
      let run;
      for (const span of spans) {
        if (run && span[0] <= run[1]) run[1] = Math.max(run[1], span[1]);
        else {
          if (run) add(coverage, row, run, 1 / samples);
          run = [...span];
        }
      }
      if (run) add(coverage, row, run, 1 / samples);
    }
  }
  return coverage;
}

function add(coverage, row, [from, to], weight) {
  const start = Math.max(0, from);
  const end = Math.min(SIDE, to);
  if (end <= start) return;
  const first = Math.floor(start);
  const last = Math.floor(end);
  const offset = row * SIDE;
  if (first === last) {
    coverage[offset + first] += (end - start) * weight;
    return;
  }
  coverage[offset + first] += (first + 1 - start) * weight;
  for (let x = first + 1; x < last; x++) coverage[offset + x] += weight;
  if (last < SIDE) coverage[offset + last] += (end - last) * weight;
}

// A PNG of the mark in `ink` over `ground`: RGB when the ground is a colour, RGBA when there is none, grey when `grey`.
function draw(coverage, ink, ground, grey = false) {
  const channels = grey ? 1 : ground ? 3 : 4;
  const pixels = Buffer.alloc(SIDE * SIDE * channels);
  for (let i = 0; i < SIDE * SIDE; i++) {
    const share = Math.min(1, coverage[i]);
    if (ground) {
      for (let c = 0; c < channels; c++) pixels[i * channels + c] = Math.round(ground[c] + (ink[c] - ground[c]) * share);
    } else {
      pixels.set(ink, i * 4);
      pixels[i * 4 + 3] = Math.round(255 * share);
    }
  }
  const stride = SIDE * channels;
  const rows = Buffer.alloc((stride + 1) * SIDE);
  for (let y = 0; y < SIDE; y++) pixels.copy(rows, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  const header = Buffer.alloc(13);
  header.writeUInt32BE(SIDE, 0);
  header.writeUInt32BE(SIDE, 4);
  header[8] = 8;
  header[9] = { 1: 0, 3: 2, 4: 6 }[channels];
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("sRGB", Buffer.from([0])),
    chunk("IDAT", deflateSync(rows, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "latin1");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}

const ios = cover(SIDE * TILE_SHARE);
const android = cover(((SIDE * 72) / 108) * TILE_SHARE);
const splash = cover(SIDE);
const nothing = new Float64Array(SIDE * SIDE);

const images = {
  "icon.png": draw(ios, BLACK, WHITE),
  "icon-dark.png": draw(ios, WHITE),
  "icon-tinted.png": draw(ios, WHITE, BLACK, true),
  "android-icon-foreground.png": draw(android, BLACK),
  "android-icon-background.png": draw(nothing, BLACK, WHITE),
  "android-icon-monochrome.png": draw(android, WHITE),
  "splash-icon.png": draw(splash, BLACK),
  "splash-icon-dark.png": draw(splash, WHITE),
};
for (const [name, png] of Object.entries(images)) {
  writeFileSync(new URL(name, assets), png);
  console.log(`wrote assets/${name}`);
}
