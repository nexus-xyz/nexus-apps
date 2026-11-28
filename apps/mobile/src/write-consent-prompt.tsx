import type { WriteConsent } from "@nexus/kit";
import * as WebBrowser from "expo-web-browser";
import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { useCore } from "@/auth";
import { haptic } from "@/haptics";
import { font, type Theme } from "@/theme";
import { consentPage, WRITE_CONSENT_FAILED, writeConsentLine, writeConsentScopes, writeConsentTitle } from "@/write-consent";

/**
 * The write consent prompt, drawn as the Connect card (`connect-card.tsx`) and worded as Nexus UI's: a write
 * Gateway refused with 403 `write_consent_required` asks "<Connector> needs your permission to write", with Not now
 * and Continue. Continue makes the call Gateway named and opens the vendor's page in the in-app browser
 * (`write-consent.ts`). The write is not sent again. Not now hides the prompt on this screen.
 */
export function WriteConsentPrompt({ consent, theme }: { consent: WriteConsent; theme: Theme }) {
  const core = useCore();
  const [hidden, setHidden] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  if (hidden) return null;
  const title = writeConsentTitle(consent);
  const line = writeConsentLine(consent);
  const scopes = writeConsentScopes(consent);
  const allow = async () => {
    if (busy) return;
    haptic("tap");
    setBusy(true);
    setProblem(undefined);
    const url = await consentPage(core, consent);
    if (url) await WebBrowser.openBrowserAsync(url).catch(() => setProblem(WRITE_CONSENT_FAILED));
    else setProblem(WRITE_CONSENT_FAILED);
    setBusy(false);
  };
  return (
    <View style={[styles.card, { backgroundColor: theme.card, borderColor: theme.border }]} accessibilityRole="summary" accessibilityLabel={`${title}. ${line}`}>
      <View style={styles.words}>
        <Text style={[styles.title, { color: theme.foreground }]}>{title}</Text>
        <Text style={[styles.line, { color: theme.muted }]}>{line}</Text>
        {scopes ? <Text style={[styles.line, { color: theme.tertiary }]}>{scopes}</Text> : null}
      </View>
      {problem ? (
        <Text style={[styles.line, { color: theme.danger }]} accessibilityRole="alert">
          {problem}
        </Text>
      ) : null}
      <View style={styles.actions}>
        <Pressable
          style={[styles.button, { backgroundColor: theme.fill }]}
          onPress={() => {
            haptic("tap");
            setHidden(true);
          }}
          disabled={busy}
          accessibilityRole="button"
          accessibilityHint="Hides this prompt"
        >
          <Text style={[styles.buttonText, { color: theme.foreground }]}>Not now</Text>
        </Pressable>
        <Pressable
          style={[styles.button, { backgroundColor: theme.primary }, busy && styles.disabled]}
          onPress={() => void allow()}
          disabled={busy}
          accessibilityRole="link"
          accessibilityState={{ disabled: busy, busy }}
          accessibilityHint="Opens the service, where you allow writes"
        >
          <Text style={[styles.buttonText, { color: theme.onPrimary }]}>{busy ? "Opening…" : "Continue"}</Text>
        </Pressable>
      </View>
    </View>
  );
}

// As the Connect card: 280 pt wide, 12 by 14 pt padding, 18 pt round, a hairline; two 38 pt buttons sharing the width.
const styles = StyleSheet.create({
  card: { width: 280, maxWidth: "100%", paddingVertical: 12, paddingHorizontal: 14, gap: 10, borderRadius: 18, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth },
  words: { gap: 2 },
  title: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
  line: { fontSize: 13.5, lineHeight: 18, fontFamily: font.regular },
  actions: { flexDirection: "row", gap: 8 },
  button: { flex: 1, height: 38, borderRadius: 12, borderCurve: "continuous", alignItems: "center", justifyContent: "center" },
  disabled: { opacity: 0.5 },
  buttonText: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
});
