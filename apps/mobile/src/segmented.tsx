import { Pressable, StyleSheet, Text, View } from "react-native";
import { font, text, useTheme } from "@/theme";
import { CHROME_FONT_SCALE } from "@/text-size";
import { haptic } from "@/haptics";

/** Two to four choices side by side, the current one raised, as iOS's segmented control. Android draws a taller pill
 * whose thumb slides (`segmented.android.tsx`). */
export function Segmented<T extends string>({ label, options, value, onChange }: { label: string; options: { id: T; label: string }[]; value: T; onChange: (v: T) => void }) {
  const theme = useTheme();
  return (
    <View style={[styles.segments, { backgroundColor: theme.userBubble }]} accessibilityLabel={label}>
      {options.map((o) => (
        <Pressable
          key={o.id}
          style={[styles.segment, o.id === value && [styles.raised, { backgroundColor: theme.thumb }]]}
          onPress={() => {
            if (o.id === value) return;
            haptic("selection");
            onChange(o.id);
          }}
          // `tab` gets no iOS trait: a button, selected.
          accessibilityRole="button"
          accessibilityState={{ selected: o.id === value }}
          accessibilityLabel={`${label}: ${o.label}`}
        >
          <Text style={[styles.segmentText, { color: o.id === value ? theme.foreground : theme.muted }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {o.label}
          </Text>
        </Pressable>
      ))}
    </View>
  );
}

const styles = StyleSheet.create({
  segments: { flexDirection: "row", padding: 2, borderRadius: 10, borderCurve: "continuous" },
  segment: { flex: 1, minHeight: 32, borderRadius: 8, borderCurve: "continuous", alignItems: "center", justifyContent: "center", paddingHorizontal: 6 },
  raised: { shadowColor: "#000", shadowOpacity: 0.08, shadowRadius: 3, shadowOffset: { width: 0, height: 1 }, elevation: 1 },
  segmentText: { ...text.sm, fontFamily: font.medium },
});
