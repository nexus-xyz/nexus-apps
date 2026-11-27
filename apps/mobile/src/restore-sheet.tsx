import { StyleSheet, Text, View } from "react-native";
import { PressScale } from "@/press-scale";
import { Sheet } from "@/sheet";
import { font, text, useTheme } from "@/theme";

export type Restorable = { id: string; content: string; createdAt?: string };

const at = (iso?: string) => (iso ? new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }) : undefined);

/**
 * Restore to here's confirmation (brand v5's P52 Restore sheet), opened from a question's menu (`message-menu.tsx`):
 * the time it goes back to, the question it goes back before, what happens, then Cancel and Restore side by side.
 */
export function RestoreSheet({ question, onClose, onRestore }: { question: Restorable | null; onClose: () => void; onRestore: (question: Restorable) => void }) {
  const theme = useTheme();
  const excerpt = question ? (question.content.length > 60 ? `${question.content.slice(0, 60).trimEnd()}…` : question.content) : "";
  return (
    <Sheet visible={!!question} onClose={onClose}>
      {question ? (
        <View style={styles.confirm}>
          <Text style={[styles.title, { color: theme.foreground }]}>{at(question.createdAt) ? `Restore to ${at(question.createdAt)}?` : "Restore to here?"}</Text>
          <Text style={[styles.detail, { color: theme.tertiary }]} numberOfLines={2}>
            Before “{excerpt}”
          </Text>
          <Text style={[styles.body, { color: theme.muted }]}>
            The chat goes back to before this message, and the message goes back in the box to edit or send again. What came after stays saved, out of the
            model's context.
          </Text>
          <View style={styles.buttons}>
            <PressScale hostStyle={styles.host} style={[styles.button, { backgroundColor: theme.fill }]} onPress={onClose} accessibilityRole="button">
              <Text style={[styles.buttonText, { color: theme.foreground }]}>Cancel</Text>
            </PressScale>
            <PressScale
              hostStyle={styles.host}
              style={[styles.button, { backgroundColor: theme.primary }]}
              onPress={() => {
                onClose();
                onRestore(question);
              }}
              accessibilityRole="button"
            >
              <Text style={[styles.buttonText, { color: theme.onPrimary }]}>Restore</Text>
            </PressScale>
          </View>
        </View>
      ) : null}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  confirm: { paddingHorizontal: 20, paddingTop: 12, paddingBottom: 8, gap: 6 },
  title: { ...text.xl, fontFamily: font.medium },
  detail: { ...text.secondary, fontFamily: font.regular },
  body: { ...text.secondary, fontFamily: font.regular, marginTop: 6 },
  buttons: { flexDirection: "row", gap: 10, marginTop: 14 },
  // The boards' sheet buttons: two side by side, full height for a thumb.
  host: { flex: 1 },
  button: { minHeight: 50, borderRadius: 25, alignItems: "center", justifyContent: "center" },
  buttonText: { ...text.body, fontFamily: font.medium },
});
