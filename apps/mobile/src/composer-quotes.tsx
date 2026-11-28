import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { Icon } from "@/icon";
import { font, text, useTheme } from "@/theme";

export type DraftQuote = { id: string; sourceId?: string; content: string };

export function ComposerQuotes({ quotes, onOpen, onRemove }: {
  quotes: DraftQuote[];
  onOpen: (quote: DraftQuote) => void;
  onRemove: (quote: DraftQuote) => void;
}) {
  const theme = useTheme();
  if (!quotes.length) return null;
  return (
    <ScrollView style={styles.list} keyboardShouldPersistTaps="handled" contentContainerStyle={styles.rows}>
      {quotes.map((quote) => (
        <View key={quote.id} style={[styles.row, { backgroundColor: theme.fill, borderColor: theme.chartMuted }]}>
          <Pressable style={styles.content} accessibilityRole="button" accessibilityLabel={`Quote: ${quote.content}`} onPress={() => onOpen(quote)}>
            <Text style={[styles.text, { color: theme.foreground }]} numberOfLines={2}>{quote.content}</Text>
          </Pressable>
          <Pressable style={styles.remove} accessibilityRole="button" accessibilityLabel="Remove quote" onPress={() => onRemove(quote)}>
            <Icon name="close" size={14} color={theme.muted} />
          </Pressable>
        </View>
      ))}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  list: { maxHeight: 132 },
  rows: { gap: 6, paddingHorizontal: 12, paddingTop: 8 },
  row: { flexDirection: "row", alignItems: "center", borderRadius: 10, borderLeftWidth: 3, paddingLeft: 10 },
  content: { flex: 1, minHeight: 44, justifyContent: "center", paddingVertical: 4 },
  text: { ...text.secondary, fontFamily: font.regular },
  remove: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
});
