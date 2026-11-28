import type { ChatAttachmentRef } from "@nexus/kit";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { sizeLabel } from "@/chat-images";
import { Icon } from "@/icon";
import { CHROME_FONT_SCALE } from "@/text-size";
import { font, text, useTheme } from "@/theme";

/** A paste long enough to be a file (P65): its text, where it went in, and the file it is sent as. */
export type Pasted = { id: string; text: string; start: number; name: string; data: string; bytes: number; mediaType?: ChatAttachmentRef["mediaType"] };

/**
 * The long pastes in the composer (brand v5's P65·G): each a file chip, "pasted-text.md · 14 KB", with a way to take it
 * out, and Paste as text instead for the latest until the next keystroke.
 */
export function PastedFiles({ pastes, offer, onAsText, onRemove }: { pastes: Pasted[]; offer: boolean; onAsText: () => void; onRemove: (id: string) => void }) {
  const theme = useTheme();
  if (!pastes.length) return null;
  return (
    <View style={styles.row}>
      {pastes.map((p) => (
        <View key={p.id} style={[styles.chip, { backgroundColor: theme.fill }]}>
          <Icon name="file" size={13} color={theme.muted} />
          <Text style={[styles.name, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {p.name}
          </Text>
          <Text style={[styles.size, { color: theme.muted }]} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {sizeLabel(p.bytes)}
          </Text>
          <Pressable onPress={() => onRemove(p.id)} style={styles.remove} accessibilityRole="button" accessibilityLabel={`Remove ${p.name}`}>
            <Icon name="close" size={12} color={theme.muted} />
          </Pressable>
        </View>
      ))}
      {offer ? (
        <Pressable onPress={onAsText} hitSlop={{ top: 10, bottom: 10 }} accessibilityRole="button">
          <Text style={[styles.asText, { color: theme.muted }]} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            Paste as text instead
          </Text>
        </Pressable>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  remove: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  row: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 8, paddingHorizontal: 10, paddingTop: 8 },
  chip: { flexDirection: "row", alignItems: "center", gap: 6, maxWidth: 260, minHeight: 44, paddingLeft: 10, paddingRight: 2, borderRadius: 16 },
  name: { ...text.xs, fontFamily: font.regular, flexShrink: 1 },
  size: { ...text.xs, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
  asText: { ...text.xs, fontFamily: font.medium, textDecorationLine: "underline" },
});
