import { Keyboard, Pressable, StyleSheet, View } from "react-native";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { font, text, useTheme } from "@/theme";

/** Settings' search field, at the top of the page as iOS Settings has it: a rounded field on the card's colour. */
export function SettingsSearch({ query, onChange }: { query: string; onChange: (query: string) => void }) {
  const theme = useTheme();
  return (
    <View style={[styles.search, { backgroundColor: theme.card }]}>
      <Icon name="search" size={16} color={theme.muted} />
      <TextInput
        accessibilityLabel="Search settings"
        autoCapitalize="none"
        autoCorrect={false}
        placeholder="Search settings"
        placeholderTextColor={theme.muted}
        returnKeyType="search"
        onSubmitEditing={() => Keyboard.dismiss()}
        style={[styles.input, { color: theme.foreground }]}
        value={query}
        onChangeText={onChange}
      />
      {query ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel="Clear settings search"
          onPress={() => {
            haptic("tap");
            onChange("");
          }}
          style={styles.clear}
        >
          <Icon name="close" size={14} color={theme.muted} />
        </Pressable>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  search: { alignItems: "center", flexDirection: "row", gap: 10, minHeight: 44, paddingLeft: 12, borderRadius: 12, borderCurve: "continuous" },
  input: { ...text.body, fontFamily: font.regular, flex: 1, minWidth: 0, paddingVertical: 10, paddingRight: 12 },
  clear: { alignItems: "center", justifyContent: "center", width: 44, minHeight: 44 },
});
