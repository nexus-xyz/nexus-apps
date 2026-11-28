import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import type { ConnectCardView } from "@/assistant-cards";
import { haptic } from "@/haptics";
import { open } from "@/markdown";
import { font, type Theme } from "@/theme";

/**
 * The Connect card (A6·e, AIPDAsstConnect panel e): a question needs an account the person has not connected, so under
 * Nexus's bubble the thread shows the service's tile, "Connect Linear", one line of what Nexus would do with it, Not
 * now and Connect. The app has no makers' marks, so the tile holds the service's first letter, as the board's does.
 * Connect opens the connector's card on Nexus UI's Integrations page in the in-app browser, where the person signs
 * in at the service; the phone itself connects nothing (`integrations.tsx`). Not now hides the card on this screen.
 */
export function ConnectCard({ view, theme }: { view: ConnectCardView; theme: Theme }) {
  const [hidden, setHidden] = useState(false);
  if (hidden) return null;
  return (
    <View style={[styles.card, { backgroundColor: theme.card, borderColor: theme.border }]} accessibilityRole="summary" accessibilityLabel={`${view.title}. ${view.line}`}>
      <View style={styles.head}>
        <View style={[styles.tile, { backgroundColor: theme.fill }]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
          <Text style={[styles.initial, { color: theme.foreground }]}>{view.initial}</Text>
        </View>
        <View style={styles.words}>
          <Text style={[styles.title, { color: theme.foreground }]}>{view.title}</Text>
          <Text style={[styles.line, { color: theme.muted }]}>{view.line}</Text>
        </View>
      </View>
      <View style={styles.actions}>
        <Pressable
          style={[styles.button, { backgroundColor: theme.fill }]}
          onPress={() => {
            haptic("tap");
            setHidden(true);
          }}
          accessibilityRole="button"
          accessibilityHint="Hides this card"
        >
          <Text style={[styles.buttonText, { color: theme.foreground }]}>Not now</Text>
        </Pressable>
        <Pressable
          style={[styles.button, { backgroundColor: theme.primary }]}
          onPress={() => {
            haptic("tap");
            open(view.url);
          }}
          accessibilityRole="link"
          accessibilityLabel={view.title}
          accessibilityHint={`Opens Nexus, where you sign in to ${view.label}`}
        >
          <Text style={[styles.buttonText, { color: theme.onPrimary }]}>Connect</Text>
        </Pressable>
      </View>
    </View>
  );
}

// The phone board's card (A6·e, A2·A): 280 pt wide, 12 by 14 pt padding, 18 pt round, a hairline; a 34 pt tile 10 pt
// round, 10 pt from the words (15 pt over 13.5/1.35); two 38 pt buttons, 12 pt round, sharing the width, 10 pt under.
const styles = StyleSheet.create({
  card: { width: 280, maxWidth: "100%", paddingVertical: 12, paddingHorizontal: 14, gap: 10, borderRadius: 18, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth },
  head: { flexDirection: "row", gap: 10, alignItems: "flex-start" },
  tile: { width: 34, height: 34, borderRadius: 10, borderCurve: "continuous", alignItems: "center", justifyContent: "center" },
  initial: { fontSize: 15, lineHeight: 20, fontFamily: font.medium },
  words: { flex: 1, gap: 2 },
  title: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
  line: { fontSize: 13.5, lineHeight: 18, fontFamily: font.regular },
  actions: { flexDirection: "row", gap: 8 },
  button: { flex: 1, height: 38, borderRadius: 12, borderCurve: "continuous", alignItems: "center", justifyContent: "center" },
  buttonText: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
});
