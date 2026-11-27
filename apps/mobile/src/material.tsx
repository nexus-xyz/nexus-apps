import { Host, type HostProps, type MaterialColors } from "@expo/ui/jetpack-compose";
import { useColorScheme } from "react-native";
import { useTheme, type Theme } from "@/theme";

// Android's Material 3 components (`@expo/ui/jetpack-compose`) in brand v4's metals. Material would colour them from
// the wallpaper (Material You) or its purple baseline; brand v4 puts no hue on a control (brand/v4.0/identity/color.md),
// so every role is set here to a theme value. Only `.android.tsx` files import this: iOS keeps its own components.

/** Every Material 3 role the components read, as a theme value (each a hex colour, as Compose takes them). */
export const materialColors = (theme: Theme) =>
  ({
  primary: theme.primary,
  onPrimary: theme.onPrimary,
  primaryContainer: theme.userBubble,
  onPrimaryContainer: theme.foreground,
  inversePrimary: theme.onPrimary,
  secondary: theme.muted,
  onSecondary: theme.onPrimary,
  secondaryContainer: theme.sidebarRow,
  onSecondaryContainer: theme.foreground,
  tertiary: theme.muted,
  onTertiary: theme.onPrimary,
  tertiaryContainer: theme.sidebarRow,
  onTertiaryContainer: theme.foreground,
  background: theme.screen,
  onBackground: theme.foreground,
  surface: theme.card,
  onSurface: theme.foreground,
  surfaceVariant: theme.userBubble,
  onSurfaceVariant: theme.muted,
  // Material tints a raised surface with this; the card's own colour keeps it a metal.
  surfaceTint: theme.card,
  inverseSurface: theme.foreground,
  inverseOnSurface: theme.screen,
  error: theme.danger,
  onError: theme.onPrimary,
  errorContainer: theme.dangerFaint,
  onErrorContainer: theme.danger,
  outline: theme.outline,
  outlineVariant: theme.border,
  scrim: "#000000",
  surfaceBright: theme.card,
  surfaceDim: theme.screen,
  surfaceContainerLowest: theme.card,
  surfaceContainerLow: theme.card,
  surfaceContainer: theme.card,
  surfaceContainerHigh: theme.userBubble,
  surfaceContainerHighest: theme.sidebarRow,
  }) as Partial<MaterialColors>;

/** A Compose host in the app's appearance and the theme's colours. */
export function MaterialHost(props: Omit<HostProps, "colorScheme" | "colorOverrides" | "seedColor">) {
  const theme = useTheme();
  const scheme = useColorScheme();
  return <Host {...props} colorScheme={scheme === "dark" ? "dark" : "light"} colorOverrides={materialColors(theme)} />;
}
