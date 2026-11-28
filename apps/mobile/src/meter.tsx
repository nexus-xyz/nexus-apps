import { StyleSheet, View } from "react-native";
import type { Theme } from "@/theme";

/**
 * The budget meter: spent, then reserved (held for calls in flight), as fractions of the budget from `meter()`. Nexus
 * UI's `.meter`: a Platinum track, Black spent, Nickel reserved.
 */
/** A meter; `text` says its value in words for a screen reader ("$0.40 of $1.00 left"). */
export function Meter({ theme, spent, reserved, label, text, danger }: { theme: Theme; spent: number; reserved: number; label: string; text?: string; danger?: boolean }) {
  // `danger`: a limit reached (invitations used up), drawn in the danger colour as Nexus UI's `data-tone`.
  return (
    <View style={[styles.track, { backgroundColor: theme.border }]} accessibilityRole="progressbar" accessibilityLabel={label} accessibilityValue={{ min: 0, max: 100, now: Math.round(spent * 100), text }}>
      <View style={[styles.fill, { width: `${spent * 100}%`, backgroundColor: danger ? theme.danger : theme.primary }]} />
      <View style={[styles.fill, { width: `${reserved * 100}%`, backgroundColor: theme.chartMuted }]} />
    </View>
  );
}

const styles = StyleSheet.create({
  track: { height: 8, borderRadius: 4, overflow: "hidden", flexDirection: "row", marginTop: 4 },
  fill: { height: "100%" },
});
