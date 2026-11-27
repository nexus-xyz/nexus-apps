import { Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { GlassButton } from "./glass";
import { haptic } from "./haptics";
import { Icon } from "./icon";
import { SidebarButton } from "./top-bar";
import { font, mono, text, useTheme } from "./theme";

/**
 * Native sessions keep a real layout boundary above their timeline, including at large text sizes. Under the title,
 * as the desktop's top bar says it: where it runs, its agent and model, its state, and what it cost so far in mono.
 */
export function NativeSessionHeader({ title, origin, agent, status, cost, onSidebar, onDetails }: {
  title: string; origin: string; agent?: string | null; status?: string | null; cost?: string | null; onSidebar: () => void; onDetails: () => void;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const where = [origin, agent].filter(Boolean).join(" · ");
  return <View style={[styles.safe, { paddingTop: insets.top, backgroundColor: theme.screen, borderBottomColor: theme.separator }]}>
    <View style={styles.row}>
      <SidebarButton onPress={onSidebar} />
      <Pressable onPress={() => { haptic("tap"); onDetails(); }} style={styles.words} accessibilityRole="button" accessibilityLabel={[title, where, status, cost ? `${cost} so far` : null].filter(Boolean).join(", ")} accessibilityHint="Opens session details and settings">
        <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1}>{title}</Text>
        <View style={styles.subtitle}><Text style={[styles.origin, { color: theme.muted }]} numberOfLines={1} ellipsizeMode="middle">{where}</Text>
          {status ? <Text style={[styles.status, { color: theme.tertiary }]}>{status}</Text> : null}
          {cost ? <Text style={[styles.cost, { color: theme.tertiary }]}>{cost}</Text> : null}
        </View>
      </Pressable>
      <GlassButton label="Session details and settings" onPress={onDetails}><Icon name="more" size={19} color={theme.foreground} /></GlassButton>
    </View>
  </View>;
}
export function SessionDetails({ title, origin, project, agent }: { title: string; origin: string; project?: string; agent?: string | null }) {
  const theme = useTheme();
  const rows = [{ label: "Session", value: title }, { label: "Runs on", value: origin }, ...(project ? [{ label: "Project", value: project }] : []), ...(agent ? [{ label: "Agent", value: agent }] : [])];
  return <View style={styles.details}>{rows.map((row) => <View key={row.label} style={styles.detail} accessible accessibilityLabel={`${row.label}, ${row.value}`}>
    <Text style={[styles.detailLabel, { color: theme.muted }]}>{row.label}</Text><Text selectable style={[styles.detailValue, { color: theme.foreground }]}>{row.value}</Text>
  </View>)}</View>;
}
const styles = StyleSheet.create({
  safe: { borderBottomWidth: StyleSheet.hairlineWidth }, row: { minHeight: 64, flexDirection: "row", alignItems: "center", paddingHorizontal: 16, paddingVertical: 6, gap: 10 },
  words: { flex: 1, minWidth: 0, minHeight: 44, justifyContent: "center", gap: 3 }, title: { ...text.body, fontFamily: font.medium }, subtitle: { flexDirection: "row", alignItems: "center", gap: 10 },
  origin: { ...text.caption, fontFamily: font.regular, flexShrink: 1 }, status: { ...text.caption, fontFamily: font.regular, flexShrink: 0 },
  cost: { ...text.caption, fontFamily: mono, fontSize: 12, fontVariant: ["tabular-nums"], flexShrink: 0 }, details: { paddingHorizontal: 20, paddingVertical: 8, gap: 16 },
  detail: { gap: 4 }, detailLabel: { ...text.caption, fontFamily: font.regular }, detailValue: { ...text.body, fontFamily: font.regular },
});
