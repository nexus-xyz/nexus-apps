import { Pressable, ScrollView, StyleSheet, Text } from "react-native";
import type { DocOutline } from "@/doc-contents";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Sheet, SheetTitle } from "@/sheet";
import { font, text, useTheme } from "@/theme";

export function DocContentsSheet({ outline, visible, onPick, onClose }: {
  outline?: DocOutline;
  visible: boolean;
  onPick: (index: number) => void;
  onClose: () => void;
}) {
  const theme = useTheme();
  return (
    <Sheet visible={visible} onClose={onClose} solid>
      <SheetTitle>Contents</SheetTitle>
      {outline?.headings.length ? (
        <ScrollView style={styles.list}>
          {outline.headings.map(({ index, title, level }) => (
            <Pressable key={index} accessibilityRole="button" accessibilityState={{ selected: outline.active === index }}
              style={({ pressed }) => [styles.heading, { paddingLeft: 16 + Math.min(level - 1, 3) * 12 },
                pressed && { backgroundColor: theme.fill }]}
              onPress={() => { haptic("selection"); onPick(index); onClose(); }}>
              <Text style={[styles.headingText, { color: theme.foreground }]}>{title}</Text>
              {outline.active === index && <Icon name="check" color={theme.foreground} size={16} />}
            </Pressable>
          ))}
        </ScrollView>
      ) : <Text style={[styles.note, { color: theme.muted }]}>Headings in this doc will appear here.</Text>}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  list: { maxHeight: 360 },
  heading: { minHeight: 44, paddingRight: 16, paddingVertical: 8, flexDirection: "row", alignItems: "center", gap: 8 },
  headingText: { ...text.body, fontFamily: font.regular, flex: 1 },
  note: { ...text.sm, fontFamily: font.regular, padding: 20 },
});
