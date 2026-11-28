import { Platform, type PressableAndroidRippleConfig } from "react-native";
import type { Theme } from "@/theme";

// How a control answers a touch: Material's ripple on Android, the row filled on iOS. Spread both on a Pressable:
// `android_ripple={ripple(theme)}` and `pressed && pressedFill(theme)` in its style.

const ANDROID = Platform.OS === "android";

/** Android's ripple, drawn inside the control's bounds. iOS draws none. */
export const ripple = (theme: Theme): PressableAndroidRippleConfig | undefined => (ANDROID ? { color: theme.ripple, foreground: true } : undefined);

/** iOS's pressed row. Android has the ripple instead, so it gets no fill. */
export const pressedFill = (theme: Theme) => (ANDROID ? null : { backgroundColor: theme.rowPressed });
