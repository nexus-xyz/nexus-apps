import type { SkillSummary } from "@nexus/kit";
import { useEffect, useState } from "react";
import { Platform, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useArtifacts, useStored } from "@/auth";
import { matchSlash } from "@/composer-text";
import { Glass } from "@/glass";
import { Icon } from "@/icon";
import { pressedFill, ripple } from "@/press";
import { CHROME_FONT_SCALE } from "@/text-size";
import { font, text, useTheme } from "@/theme";

/** A row of the menu: a skill of yours, or one of the app's own commands. */
export type SlashEntry = { kind: "skill" | "command"; name: string; description: string };

export const COMMANDS: SlashEntry[] = [
  { kind: "command", name: "model", description: "Change this session’s model" },
  { kind: "command", name: "context", description: "See this session’s token usage" },
];

/** Your skills, as they were last time until the Gateway answers; read once per screen. */
export function useSkills(): SlashEntry[] {
  const artifacts = useArtifacts();
  const cache = useStored<SkillSummary[]>("cache", "skills");
  const [skills, setSkills] = useState<SkillSummary[]>(() => cache.load() ?? []);
  useEffect(() => {
    artifacts.skills().then(
      (list) => {
        setSkills(list.skills);
        cache.save(list.skills);
      },
      () => undefined,
    );
  }, [artifacts, cache]);
  return skills.map((s) => ({ kind: "skill", name: s.name, description: s.description }));
}

/**
 * Skills and commands (brand v5's P98·D): typing `/` at the start of a word opens this card right above the composer,
 * filtered as you type; a tap puts the skill in the text, or runs the command. Five rows show, the rest scroll.
 */
export function SlashMenu({ entries, query, onPick }: { entries: SlashEntry[]; query: string; onPick: (entry: SlashEntry) => void }) {
  const theme = useTheme();
  const shown = matchSlash(entries, query);
  if (!shown.length) return null;
  return (
    <View style={styles.room}>
      <Glass style={styles.card} accessibilityRole="menu">
        <Text style={[styles.title, { color: theme.tertiary }]} accessibilityRole="header" maxFontSizeMultiplier={CHROME_FONT_SCALE}>
          Skills and commands
        </Text>
        <ScrollView style={styles.list} keyboardShouldPersistTaps="always" bounces={false}>
          {shown.map((entry, n) => (
            <Pressable
              key={`${entry.kind}:${entry.name}`}
              android_ripple={ripple(theme)}
              style={({ pressed }) => [styles.row, n > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: theme.separator }, pressed && pressedFill(theme)]}
              onPress={() => onPick(entry)}
              accessibilityRole="menuitem"
              accessibilityLabel={`${entry.name}, ${entry.description}`}
            >
              <Icon name={entry.kind === "skill" ? "skill" : "command"} size={15} color={theme.muted} />
              <View style={styles.body}>
                <Text style={[styles.name, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
                  /{entry.name}
                </Text>
                <Text style={[styles.description, { color: theme.tertiary }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
                  {entry.description}
                </Text>
              </View>
            </Pressable>
          ))}
        </ScrollView>
      </Glass>
    </View>
  );
}

// The board's card (P98·D): 12 pt in from the screen, radius 20, its title 13 pt; rows 52 pt with a 15 pt glyph, the
// name 16 pt and what it does 13 pt under it. Five rows, then it scrolls.
const ROW = 52;
const styles = StyleSheet.create({
  room: { paddingHorizontal: 12, paddingBottom: 8 },
  card: { borderRadius: 20, borderCurve: "continuous", overflow: "hidden", paddingTop: 8 },
  title: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 16, paddingBottom: 4 },
  list: { maxHeight: ROW * 5 },
  row: { flexDirection: "row", alignItems: "center", gap: 12, minHeight: ROW, paddingHorizontal: 16, paddingVertical: 6, ...(Platform.OS === "android" ? { overflow: "hidden" as const } : null) },
  body: { flex: 1, gap: 1 },
  name: { ...text.base, fontFamily: font.regular },
  description: { ...text.caption, fontFamily: font.regular },
});
