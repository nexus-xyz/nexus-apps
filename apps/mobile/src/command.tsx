import * as Clipboard from "expo-clipboard";
import { useState } from "react";
import { Pressable, StyleSheet, Text } from "react-native";
import { font, mono, text, type Theme } from "@/theme";

/** Copy a text, and which one was copied, for a moment, so the control can say "Copied". */
export function useCopy(): [copied: string | null, copy: (text: string) => void] {
  const [copied, setCopied] = useState<string | null>(null);
  const copy = (value: string) =>
    void Clipboard.setStringAsync(value).then(() => {
      setCopied(value);
      setTimeout(() => setCopied((now) => (now === value ? null : now)), 1500);
    });
  return [copied, copy];
}

/** A command to run on a computer: tap to copy it (there is nothing to run it in on a phone). */
export function Command({ command, theme }: { command: string; theme: Theme }) {
  const [copied, copy] = useCopy();
  return (
    <Pressable
      style={({ pressed }) => [styles.command, { backgroundColor: theme.userBubble }, pressed && { opacity: 0.7 }]}
      onPress={() => copy(command)}
      accessibilityRole="button"
      accessibilityLabel={copied ? "Copied" : "Copy the command"}
      accessibilityHint={command}
    >
      <Text style={[styles.code, { color: theme.foreground }]} numberOfLines={3} selectable>
        {command}
      </Text>
      <Text style={[styles.copy, { color: copied ? theme.primary : theme.muted }]}>{copied ? "Copied" : "Copy"}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  command: { flexDirection: "row", alignItems: "center", gap: 10, borderRadius: 10, paddingHorizontal: 12, paddingVertical: 8 },
  code: { fontFamily: mono, fontSize: 13, lineHeight: 18, flex: 1 },
  copy: { ...text.xs, fontFamily: font.medium },
});
