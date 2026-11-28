import { Pressable, StyleSheet, Text, View } from "react-native";
import { CONSENT, consentText, withdrawable } from "@/ai-consent";
import { SITE_PAGES } from "@/links";
import { open } from "@/markdown";
import { Sheet, SheetRow } from "@/sheet";
import { load, removeNow, save } from "@/store";
import { font, text, useTheme } from "@/theme";

/** The AI providers the person agreed to send chats to, kept on the device (`ai-consent.ts`). Settings withdraws them. */
export const keptConsent = withdrawable({
  load: () => load<string[]>(CONSENT),
  save: (providers) => save(CONSENT, providers),
  remove: () => removeNow(CONSENT),
});

/**
 * The question before the first send (App Review 5.1.2(i)): which providers get a chat message and its
 * photos, with the privacy policy. Open while `names` is set; closing the sheet declines.
 */
export function ConsentSheet({ names, onAnswer }: { names: string[] | null; onAnswer: (allow: boolean) => void }) {
  const theme = useTheme();
  const { title, message } = consentText(names ?? []);
  return (
    <Sheet visible={!!names} onClose={() => onAnswer(false)}>
      <View style={styles.body}>
        <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">
          {title}
        </Text>
        <Text style={[styles.text, { color: theme.muted }]}>{message}</Text>
        <Text style={[styles.text, { color: theme.muted }]}>Nexus asks again before it sends to a new provider. You can withdraw this in Settings.</Text>
        {/* A link in the text, not a row, so the sheet's only choices are Allow and Don't Allow. */}
        <Pressable style={styles.link} onPress={() => open(SITE_PAGES.privacyPolicy)} accessibilityRole="link" hitSlop={8}>
          <Text style={[styles.text, styles.underline, { color: theme.foreground }]}>Privacy policy</Text>
        </Pressable>
      </View>
      <SheetRow label="Allow" tone="primary" onPress={() => onAnswer(true)} />
      <SheetRow label="Don't Allow" tone="muted" onPress={() => onAnswer(false)} />
    </Sheet>
  );
}

const styles = StyleSheet.create({
  body: { gap: 8, paddingHorizontal: 20, paddingTop: 12, paddingBottom: 8 },
  title: { ...text.lg, fontFamily: font.medium },
  text: { ...text.sm, fontFamily: font.regular },
  link: { alignSelf: "flex-start" },
  underline: { textDecorationLine: "underline" },
});
