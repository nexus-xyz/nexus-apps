import { Appearance, useColorScheme } from "react-native";
import { load, save } from "./store";

// The phone layer of brand v5's product boards (Nexus AI P18 to P22, P31-M; 2026-11-02): the desktop's charcoal layer
// in dark and iOS's grouped greys in light. Everything people read is a grey, and the primary button is Black on light
// and White on dark, never a hue: the send button and an approval are the only filled controls. Text clears 4.5:1 on
// every ground it sits on. A route hue belongs only on routed work, and the phone shows none yet.
const light = {
  // A plain screen (a chat, a page) and a grouped one (Settings, the lists): its cards sit on it in White.
  screen: "#fbfbfc",
  grouped: "#f2f2f4",
  card: "#ffffff",
  foreground: "#1c1c1e", // 16.5:1 on the screen.
  muted: "#5c5c60", // The second line of a row. 6.6:1 on White.
  tertiary: "#6e6e73", // Times, section titles and hints. 5.1:1 on White, 4.5:1 on the grouped ground.
  border: "#dcdcdf", // The hairline under a row and around a field.
  borderSubtle: "#dcdcdf",
  // A card's hairline between its rows (`list.tsx`).
  separator: "#dcdcdf",
  // Links, checks and the selected option too, as Nexus UI's --nx-fg-strong.
  primary: "#000000",
  primaryFaint: "#f2f2f4", // A quiet fill behind a result.
  onPrimary: "#ffffff",
  // The boards' one fill: the search field, a round button, an icon's tile, an avatar, the chosen pill, your message.
  fill: "#e6e6e8",
  userBubble: "#e6e6e8",
  rowPressed: "#e6e6e8", // A row under the finger.
  danger: "#b63132", // Destructive actions only. 6.0:1.
  dangerFaint: "#fbf0f0",
  attention: "#9c570a",
  // The drawer, and what stands in for Liquid Glass where iOS draws none: frosted on iOS before 26, opaque with a
  // shadow on Android (a translucent view shows its elevation through).
  sidebar: "#f2f2f4",
  // A drawer row pressed, or the chat on screen.
  sidebarRow: "#e6e6e8",
  glass: "rgba(251,251,252,0.92)",
  glassSolid: "#f2f2f4",
  glassBorder: "#dcdcdf",
  // On a sheet, which is Liquid Glass or a solid card: the fill as a tint of what is under it, so a tile and the chosen
  // row stand out the same way on either. The opaque fill sits darker than the glass in dark, and reads as a hole.
  // Stacked (a tile on the chosen row) they still part.
  glassFill: "rgba(0,0,0,0.05)",
  glassChosen: "rgba(0,0,0,0.07)",
  // A segmented control's selected segment: White on the fill's track.
  thumb: "#ffffff",
  // A chart's third series and a meter's reserved part, beside a label and never text.
  chartMuted: "#aeaeb2",
  // A row's swipe actions (More, then Archive), under White text. The boards' greys, a step darker in light so the
  // labels clear 4.5:1.
  swipeMore: "#636366",
  swipeArchive: "#727276",
  // A switch that is on: the system green, the one place the phone keeps iOS's own colour (P21).
  success: "#30d158",
  // A text field's selection on Android, which fills it with this color as given: the text at 20%. iOS draws its own
  // shade of the caret's color.
  selection: "rgba(28,28,30,0.2)",
  // Android only. Material's outline role (`material.tsx`).
  outline: "#c7c7cc",
  // Android only. Material's ripple, a pressed state layer: the text at 10%. iOS fills the row with rowPressed instead.
  ripple: "rgba(28,28,30,0.1)",
  // Nexus ice blue, Nexus UI's one accent (--nx-ice): the composer's halo while a new chat waits for its first message
  // (`composer-halo.ts`). Never text.
  ice: "#5ac8f0",
  // A commented passage's mark (P20 Comment): the bar beside its quote. Never text on its own.
  mark: "#c79a00",
  // Brand v5's amber status dot (P73·H, `--nx-status-waiting`): someone must act on a payment. Never text.
  waiting: "#c79a00",
  // A Code session's marks, as the desktop draws them (GPUI `palette.rs`, `working` and `waiting`): ice while its agent
  // runs, amber while it waits on you, each darkened in light to keep 3:1 on the drawer. Never text.
  working: "#1d8fcb",
  needsYou: "#b07800",
};

// The desktop's charcoal layer, as the boards draw it in dark: the grouped ground a step below a plain screen, a card a
// step above it, and the fill a step above the card.
const dark: typeof light = {
  screen: "#18181a",
  grouped: "#161617",
  card: "#212123",
  foreground: "#ececec", // 15.0:1 on the screen.
  muted: "#a1a1a6", // 6.3:1 on a card.
  tertiary: "#8a8a8e", // 4.7:1 on a card, 5.3:1 on the grouped ground.
  border: "#2e2e30",
  borderSubtle: "#2e2e30",
  separator: "#2e2e30",
  primary: "#ffffff",
  primaryFaint: "#212123",
  onPrimary: "#000000",
  fill: "#2a2a2c",
  userBubble: "#2a2a2c",
  rowPressed: "#2a2a2c",
  danger: "#ef6663", // 5.2:1 on a card.
  dangerFaint: "#2e1615",
  attention: "#f7b955",
  sidebar: "#1c1c1e",
  sidebarRow: "#2a2a2c",
  glass: "rgba(33,33,35,0.9)",
  glassSolid: "#212123",
  glassBorder: "#2e2e30",
  glassFill: "rgba(255,255,255,0.07)",
  glassChosen: "rgba(255,255,255,0.1)",
  thumb: "#3a3a3c", // The selected segment, a step above its fill's track.
  chartMuted: "#48484c",
  swipeMore: "#48484a",
  swipeArchive: "#636366",
  success: "#30d158",
  selection: "rgba(236,236,236,0.2)",
  outline: "#3a3a3c",
  ripple: "rgba(236,236,236,0.1)",
  ice: "#68ccf5", // --nx-ice in dark.
  mark: "#ffd60a",
  waiting: "#ffd60a",
  working: "#68ccf5",
  needsYou: "#ffc53d",
};

export type Theme = typeof light;
export const useTheme = (): Theme => (useColorScheme() === "dark" ? dark : light);

/** The appearance chosen in Settings; `unspecified` follows the system. */
export type AppearanceChoice = "light" | "dark" | "unspecified";
const PREF = "pref:appearance";

/** The appearance chosen in Settings, as kept on the device. */
export const keptAppearance = (): AppearanceChoice => load<AppearanceChoice>(PREF) ?? "unspecified";

/** Use `appearance` app-wide (every `useColorScheme` follows it) and keep it on the device. */
export function setAppearance(appearance: AppearanceChoice) {
  Appearance.setColorScheme(appearance);
  save(PREF, appearance);
}

/** The appearance kept from last time, applied before the first screen draws. */
export function restoreAppearance() {
  const kept = load<AppearanceChoice>(PREF);
  if (kept === "light" || kept === "dark") Appearance.setColorScheme(kept);
}

/**
 * The families `useFonts` registers in the root layout, by brand v4's type (`brand/v4.0/identity/typography.md`): DM
 * Sans Regular to read, Medium for emphasis and buttons, never bold. `display` is DM Sans's 24 pt cut, for a title of
 * 21 points or more, the tighter cut the web gets from optical sizing.
 */
export const font = { regular: "DMSans_400Regular", italic: "DMSans_400Regular_Italic", medium: "DMSans_500Medium", display: "DMSans_24pt_400Regular" };
/** Geist Mono, for code, money and model ids. */
export const mono = "GeistMono_400Regular";

/** The type scale: size and line height. `body` (17), `secondary` (15) and `caption` (13) are the boards' iOS sizes. */
export const text = {
  xs: { fontSize: 13, lineHeight: 17 },
  sm: { fontSize: 14, lineHeight: 19 },
  base: { fontSize: 16, lineHeight: 23 },
  lg: { fontSize: 18, lineHeight: 23 },
  xl: { fontSize: 21, lineHeight: 28 },
  xxl: { fontSize: 26, lineHeight: 32 },
  caption: { fontSize: 13, lineHeight: 18 },
  secondary: { fontSize: 15, lineHeight: 20 },
  body: { fontSize: 17, lineHeight: 22 },
  // What you read in a chat: the question and the answer, 17 on 24 (the boards' 1.42).
  reading: { fontSize: 17, lineHeight: 24 },
  // iOS's large title in DM Sans: 34 on 41, tracked −0.025em.
  largeTitle: { fontSize: 34, lineHeight: 41, letterSpacing: -0.85 },
};
