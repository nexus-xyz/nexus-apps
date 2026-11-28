import { DISCOVER_LINE, introHeadline, introReads, type AgentIntro } from "@nexus/kit";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { NexusGreeting } from "@/assistant-bubbles";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { font, type Theme } from "@/theme";
import { useAssistantName } from "@/use-assistant-name";

/**
 * The Assistant's first open on the phone (the Assistant canvas's "Empty state", phone board). The profile
 * header above it is the screen's. Nexus writes first: one line and what it can read. Then come tiles, each with its
 * icon and a chevron: two starters, Connect your accounts, and See what I can do. The words are the agent API's and the
 * kit's (`agent-intro.ts`), so the phone shows what the web and desktop show. A starter sends its line; nothing here is
 * a wizard.
 */
export function AgentFirstOpen({
  intro,
  firstName,
  org,
  theme,
  disabled,
  onStart,
  onAccounts,
}: {
  intro: AgentIntro;
  firstName?: string;
  org?: { name: string; personal: boolean };
  theme: Theme;
  disabled: boolean;
  /** Sends a line as the person's question: a starter, or `discover`. */
  onStart: (line: string) => void;
  /** Opens where the person connects their accounts. */
  onAccounts: () => void;
}) {
  const assistant = useAssistantName();
  const reads = introReads(intro, org, assistant);
  return (
    <NexusGreeting words={introHeadline(intro, firstName, assistant)} theme={theme}>
      {reads ? <Text style={[styles.reads, { color: theme.muted }]}>{reads}</Text> : null}
      <View style={styles.tiles}>
        {intro.starters.slice(0, PHONE_STARTERS).map((line) => (
          <Tile key={line} icon="comment" label={line} theme={theme} disabled={disabled} onPress={() => onStart(line)} />
        ))}
        {intro.accounts ? <Tile icon="connector" label="Connect your accounts" theme={theme} onPress={onAccounts} /> : null}
        {intro.discover ? <Tile icon="plan" label="See what I can do" theme={theme} disabled={disabled} onPress={() => onStart(DISCOVER_LINE)} /> : null}
      </View>
    </NexusGreeting>
  );
}

/** One starter tile: an icon, the words, and a chevron. The whole tile is the button. */
export function Tile({ icon, label, theme, disabled, onPress }: { icon: "comment" | "connector" | "plan"; label: string; theme: Theme; disabled?: boolean; onPress: () => void }) {
  return (
    <Pressable
      onPress={() => {
        haptic("tap");
        onPress();
      }}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={label}
      style={({ pressed }) => [styles.tile, { backgroundColor: pressed ? theme.fill : theme.card, borderColor: theme.border }]}
    >
      <Icon name={icon} size={16} color={theme.muted} />
      <Text style={[styles.tileText, { color: theme.foreground }]}>{label}</Text>
      <Icon name="forward" size={12} color={theme.tertiary} />
    </Pressable>
  );
}

/** The canvas's phone board draws two starters, then the connect row. */
const PHONE_STARTERS = 2;

// The canvas's phone tiles: full width, 12 pt round, a hairline, 10 by 12 pt padding, 6 pt apart; the icon 10 pt from
// the words, and a chevron at the end. At least 44 pt tall, as a touch target.
const styles = StyleSheet.create({
  reads: { fontSize: 14, lineHeight: 19, fontFamily: font.regular, marginTop: -14, marginBottom: 16, paddingHorizontal: 4 },
  tiles: { gap: 6, marginBottom: 20 },
  tile: {
    minHeight: 44,
    flexDirection: "row",
    alignItems: "center",
    gap: 10,
    paddingHorizontal: 12,
    paddingVertical: 10,
    borderRadius: 12,
    borderCurve: "continuous",
    borderWidth: StyleSheet.hairlineWidth,
  },
  tileText: { flex: 1, fontSize: 15, lineHeight: 20, fontFamily: font.regular },
});
