/**
 * The composer's halo, as Nexus UI draws it in Chat (ui/src/app/app/nexus-ui.css, "The composer's states"): an ice halo
 * on a new chat's composer, which waits for the first message (`.chat-home .composer`, and the Mac panel's), and a glow
 * in the text's colour that breathes while an answer comes in (`.composer-working`). A chat between answers has none.
 * The green Done halo is the desktop workbench's (Code); Chat does not draw it. No React Native here, so vitest runs it.
 */

export type Halo = "waiting" | "working" | null;

/** Working while an answer comes in; else waiting on a new chat that has sent nothing yet; else none. */
export function haloOf({ busy, chat, sent }: { busy: boolean; chat: string | null; sent: number }): Halo {
  if (busy) return "working";
  return !chat && !sent ? "waiting" : null;
}

/** The look of a halo: an edge colour (waiting only) and the shadows around the composer, in CSS's `box-shadow` syntax. */
export interface HaloLook {
  borderColor?: string;
  boxShadow: string;
}

/** The waiting halo: the edge 55% ice into the line (85% while you type), a 3 pt ice ring and a soft ice glow. */
export function waitingLook(ice: string, line: string, focused: boolean): HaloLook {
  return {
    borderColor: mix(ice, line, focused ? 0.85 : 0.55),
    boxShadow: `0 0 0 3px ${withAlpha(ice, 0.12)}, 0 0 28px -6px ${withAlpha(ice, 0.45)}`,
  };
}

/** The working glow at the top of its breath; `BREATH` in `motion.ts` fades it. */
export function workingLook(foreground: string): HaloLook {
  return { boxShadow: `0 0 0 3px ${withAlpha(foreground, 0.08)}, 0 0 30px -4px ${withAlpha(foreground, 0.32)}` };
}

/** `color-mix(in srgb, <hex> <alpha>, transparent)`: the colour at that opacity. */
export function withAlpha(hex: string, alpha: number): string {
  const [r, g, b] = channels(hex);
  return `rgba(${r},${g},${b},${alpha})`;
}

/** `color-mix(in srgb, <a> <share>, <b>)`: `share` of `a` and the rest of `b`, as hex. */
export function mix(a: string, b: string, share: number): string {
  const from = channels(a);
  const to = channels(b);
  return `#${from.map((c, i) => Math.round(c * share + to[i] * (1 - share)).toString(16).padStart(2, "0")).join("")}`;
}

function channels(hex: string): [number, number, number] {
  const value = /^#([0-9a-f]{6})$/i.exec(hex)?.[1];
  if (!value) throw new Error(`not a #rrggbb colour: ${hex}`);
  return [0, 2, 4].map((at) => parseInt(value.slice(at, at + 2), 16)) as [number, number, number];
}
