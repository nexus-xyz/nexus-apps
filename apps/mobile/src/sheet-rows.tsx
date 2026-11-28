import { Platform, Pressable, StyleSheet, Text, View } from "react-native";
import { pressedFill, ripple } from "@/press";
import { font, text, useTheme } from "@/theme";

/** A sheet's muted heading: what its rows act on, or the question they answer. */
export function SheetTitle({ children }: { children: React.ReactNode }) {
  const theme = useTheme();
  return (
    <Text style={[styles.title, { color: theme.muted }]} numberOfLines={2}>
      {children}
    </Text>
  );
}

export function SheetRow({ label, leading, onPress, tone = "normal" }: { label: string; leading?: React.ReactNode; onPress: () => void; tone?: "normal" | "danger" | "muted" | "primary" }) {
  const theme = useTheme();
  const color = { normal: theme.foreground, danger: theme.danger, muted: theme.muted, primary: theme.primary }[tone];
  return (
    <Pressable android_ripple={ripple(theme)} style={({ pressed }) => [styles.row, pressed && pressedFill(theme)]} onPress={onPress} accessibilityRole="button">
      {leading ? <View style={styles.leading}>{leading}</View> : null}
      <Text style={[styles.label, { color }]}>{label}</Text>
    </Pressable>
  );
}

// Material 3's list item in a sheet is 56 dp high and set in from the sheet's edge by 24 dp.
const ANDROID = Platform.OS === "android";
const styles = StyleSheet.create({
  title: { ...text.xs, fontFamily: font.medium, paddingHorizontal: ANDROID ? 24 : 20, paddingVertical: 10 },
  row: { minHeight: ANDROID ? 56 : 50, flexDirection: "row", alignItems: "center", paddingHorizontal: ANDROID ? 24 : 20 },
  leading: { width: 24, marginRight: 8, alignItems: "center" },
  label: { ...text.base, fontFamily: font.medium, flexShrink: 1 },
});
