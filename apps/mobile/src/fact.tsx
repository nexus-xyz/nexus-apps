import type { Href } from "expo-router";
import { StyleSheet, Text, View } from "react-native";
import { Row } from "@/list";
import { font, mono, text, useTheme } from "@/theme";

/**
 * One fact of an item's page (brand v5's P105·D "An item"): its name small over its value, a row of the item's card.
 * Used by the Tools, Activity and Admin pages' items: an audit event, an integration, a repository.
 */
export function Fact({ label, value, monoValue, tone, href, onPress }: { label: string; value: string; monoValue?: boolean; tone?: "danger"; href?: Href; onPress?: () => void }) {
  const theme = useTheme();
  return (
    <Row
      label={
        <View style={styles.fact}>
          <Text style={[styles.name, { color: theme.tertiary }]}>{label}</Text>
          <Text style={[styles.value, monoValue && styles.mono, { color: tone === "danger" ? theme.danger : theme.foreground }]} selectable={!href && !onPress}>
            {value}
          </Text>
        </View>
      }
      href={href}
      onPress={onPress}
      accessibilityLabel={`${label}: ${value}`}
    />
  );
}

// The board's fact: the name 12 pt over the value at the body's size.
const styles = StyleSheet.create({
  fact: { flex: 1, gap: 1 },
  name: { fontSize: 12, lineHeight: 16, fontFamily: font.regular },
  value: { ...text.body, fontFamily: font.regular },
  mono: { fontFamily: mono, fontSize: 15, lineHeight: 21 },
});
