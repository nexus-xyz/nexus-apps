/**
 * The radius of an iPhone screen's own corners, in points, by its portrait size. The sidebar's card (`drawer-card.tsx`)
 * rounds to it, so the chat reads as the screen itself sliding away, as Claude's app does. iOS has no public API for it.
 * The values are Apple's display corner radii for each model. A size not listed (an iPad, a new model) gets
 * `FALLBACK`; a phone with a Home button has square corners.
 */
const RADII: Record<string, number> = {
  "375x812": 39, // iPhone X, XS, 11 Pro; the 12 and 13 mini share the size and round to 44.
  "414x896": 41.5, // iPhone XR, 11, XS Max, 11 Pro Max.
  "390x844": 47.33, // iPhone 12, 12 Pro, 13, 13 Pro, 14, 16e.
  "428x926": 53.33, // iPhone 12 Pro Max, 13 Pro Max, 14 Plus.
  "393x852": 55, // iPhone 14 Pro, 15, 15 Pro, 16.
  "430x932": 55, // iPhone 14 Pro Max, 15 Plus, 15 Pro Max, 16 Plus.
  "402x874": 62, // iPhone 16 Pro, 17, 17 Pro.
  "420x912": 62, // iPhone Air.
  "440x956": 62, // iPhone 16 Pro Max, 17 Pro Max.
};

/** A screen with rounded corners that the table does not know. */
export const FALLBACK = 44;

/** `width` and `height` are the screen's size in points; `rounded` is whether it has a home indicator. */
export function displayCornerRadius(width: number, height: number, rounded: boolean): number {
  if (!rounded) return 0;
  const key = `${Math.round(Math.min(width, height))}x${Math.round(Math.max(width, height))}`;
  return RADII[key] ?? FALLBACK;
}
