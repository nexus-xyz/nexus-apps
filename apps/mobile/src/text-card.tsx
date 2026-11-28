import type { AgentText } from "@nexus/kit";
import { useEffect, useState } from "react";
import { ActivityIndicator, AppState, Linking, Pressable, StyleSheet, Text, View } from "react-native";
import { referralOffer } from "@/account";
import { useAuth, useCore } from "@/auth";
import { afterMessages, textCardTitle, textToSend, type ContactMatch, type PhoneChoice } from "@/contact-text";
import { contactsAccess, findContacts, type ContactsAccess } from "@/contacts";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { composeText } from "@/text-message";
import { font, text, type Theme } from "@/theme";

/** Where the card is: finding the contact, then the approval, then how it ended. */
type Step =
  | { kind: "checking" }
  | { kind: "ask" }
  | { kind: "denied" }
  | { kind: "searching" }
  | { kind: "none" }
  | { kind: "choose"; matches: ContactMatch[] }
  | { kind: "ready"; name: string; phone: PhoneChoice; others: number; matches: ContactMatch[] }
  | { kind: "ended"; line: string };

/**
 * A text Nexus prepared: a normal permission prompt for a send, in the thread. It finds the person
 * Nexus was asked to text in this phone's contacts (they never leave the phone), then shows To, the exact text, and
 * Edit, Cancel and Send. Send opens the person's own Messages app with both filled in; nothing goes until they tap Send
 * there. An invitation ends with the person's referral link. Contacts access is asked for here, in Nexus's words first,
 * the first time a card needs it. A card waiting on access checks again when the person comes back from Settings.
 */
export function TextCard({ card, theme }: { card: AgentText; theme: Theme }) {
  const link = useReferralLink(card.invite === true);
  const [step, setStep] = useState<Step>({ kind: "checking" });
  const [draft, setDraft] = useState<string | undefined>();
  const [editing, setEditing] = useState(false);
  const prepared = textToSend(card, link);
  const body = draft ?? prepared.text;

  const search = async () => {
    setStep({ kind: "searching" });
    const matches = await findContacts(card.to).catch(() => []);
    const only = matches.length === 1 ? matches[0] : undefined;
    if (only?.phones.length === 1) setStep({ kind: "ready", name: only.name, phone: only.phones[0], others: 0, matches });
    else setStep(matches.length ? { kind: "choose", matches } : { kind: "none" });
  };
  const allow = async (ask: boolean) => {
    const access: ContactsAccess = await contactsAccess(ask);
    if (access === "granted") return search();
    setStep({ kind: access === "denied" ? "denied" : "ask" });
  };
  useEffect(() => {
    void allow(false);
    // Once per card: a later answer gets its own card.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // Back from Settings: while access is off, the card checks again each time the app comes to the front, with no prompt.
  // Only when denied: the system's own prompt also leaves and returns to the app, and that answer already arrives.
  const waiting = step.kind === "denied";
  useEffect(() => {
    if (!waiting) return;
    const listener = AppState.addEventListener("change", (state) => {
      if (state === "active") void allow(false);
    });
    return () => listener.remove();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [waiting]);

  const send = async (to: string[]) => {
    haptic("tap");
    const result = await composeText(body, to).catch(() => "cancelled" as const);
    const line = afterMessages(result);
    if (line) setStep({ kind: "ended", line });
  };

  const header = (
    <View style={styles.head}>
      <Text style={[styles.what, { color: theme.foreground }]}>{textCardTitle(card)}</Text>
      <Text style={[styles.small, { color: theme.muted }]}>Your contacts stay on this phone.</Text>
    </View>
  );
  const message = editing ? (
    <TextInput
      style={[styles.input, { backgroundColor: theme.fill, color: theme.foreground }]}
      value={body}
      onChangeText={setDraft}
      multiline
      accessibilityLabel="The text"
      autoFocus
    />
  ) : (
    <View style={[styles.exact, { backgroundColor: theme.fill }]}>
      <Text style={[styles.body, { color: theme.foreground }]} selectable>
        {body}
      </Text>
    </View>
  );

  return (
    <View style={[styles.card, { backgroundColor: theme.card, borderColor: theme.border }]} accessibilityRole="summary">
      {header}
      {step.kind === "checking" || step.kind === "searching" ? <ActivityIndicator color={theme.muted} /> : null}
      {step.kind === "ask" ? (
        <>
          <Text style={[styles.small, { color: theme.foreground }]}>To find {card.to}, Nexus looks in this phone&apos;s contacts. Nothing is uploaded.</Text>
          <Button label={`Find ${card.to}`} primary theme={theme} onPress={() => void allow(true)} />
        </>
      ) : null}
      {step.kind === "denied" ? (
        <>
          <Text style={[styles.small, { color: theme.foreground }]}>Nexus can&apos;t read your contacts. Allow it in Settings, or choose {card.to} in Messages.</Text>
          {message}
          <View style={styles.actions}>
            <Button label="Settings" theme={theme} onPress={() => void Linking.openSettings()} />
            <Button label="Open Messages" primary theme={theme} onPress={() => void send([])} />
          </View>
        </>
      ) : null}
      {step.kind === "none" ? (
        <>
          <Text style={[styles.small, { color: theme.foreground }]}>No contact named {card.to} has a phone number. Choose who gets it in Messages.</Text>
          {message}
          <Button label="Open Messages" primary theme={theme} onPress={() => void send([])} />
        </>
      ) : null}
      {step.kind === "choose" ? (
        <View style={styles.list} accessibilityRole="list">
          <Text style={[styles.small, { color: theme.muted }]}>Which {card.to}?</Text>
          {step.matches.flatMap((match) =>
            match.phones.map((phone) => (
              <Pressable
                key={`${match.id}:${phone.number}`}
                style={({ pressed }) => [styles.row, { backgroundColor: pressed ? theme.fill : "transparent", borderColor: theme.border }]}
                onPress={() => {
                  haptic("selection");
                  setStep({ kind: "ready", name: match.name, phone, others: step.matches.reduce((n, m) => n + m.phones.length, 0) - 1, matches: step.matches });
                }}
                accessibilityRole="button"
                accessibilityLabel={`${match.name}, ${phone.label} ${phone.number}`}
              >
                <Text style={[styles.body, { color: theme.foreground }]}>{match.name}</Text>
                <Text style={[styles.small, { color: theme.muted }]}>
                  {phone.label} · {phone.number}
                </Text>
              </Pressable>
            )),
          )}
        </View>
      ) : null}
      {step.kind === "ready" ? (
        <>
          <View style={styles.fact}>
            <Text style={[styles.factLabel, { color: theme.muted }]}>To</Text>
            <Text style={[styles.factValue, { color: theme.foreground }]}>
              {step.name} · {step.phone.label} {step.phone.number}
            </Text>
          </View>
          {step.others > 0 ? (
            <Pressable onPress={() => setStep({ kind: "choose", matches: step.matches })} accessibilityRole="button" hitSlop={8}>
              <Text style={[styles.small, styles.underline, { color: theme.primary }]}>Choose another number</Text>
            </Pressable>
          ) : null}
          {message}
          {prepared.linkMissing ? <Text style={[styles.small, { color: theme.muted }]}>Your referral link isn&apos;t available now, so the text goes without it.</Text> : null}
          <View style={styles.actions}>
            <Button label={editing ? "Done" : "Edit"} theme={theme} onPress={() => setEditing((on) => !on)} />
            <Button label="Cancel" theme={theme} onPress={() => setStep({ kind: "ended", line: "You cancelled this text." })} />
            <Button label="Send" primary theme={theme} onPress={() => void send([step.phone.number])} disabled={!body.trim()} />
          </View>
        </>
      ) : null}
      {step.kind === "ended" ? <Text style={[styles.small, { color: theme.muted }]}>{step.line}</Text> : null}
    </View>
  );
}

/** The person's referral link, read only for an invitation; undefined while it loads, or when there is none. */
function useReferralLink(wanted: boolean): string | undefined {
  const core = useCore();
  const { ui } = useAuth();
  const [link, setLink] = useState<string | undefined>();
  useEffect(() => {
    if (!wanted) return;
    let live = true;
    core.referrals().then(
      (list) => live && setLink(referralOffer(list, ui).link),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [core, ui, wanted]);
  return link;
}

function Button({ label, primary, disabled, theme, onPress }: { label: string; primary?: boolean; disabled?: boolean; theme: Theme; onPress: () => void }) {
  return (
    <Pressable
      style={[styles.button, { backgroundColor: primary ? theme.primary : theme.fill }, disabled && styles.disabled]}
      onPress={onPress}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityState={{ disabled: !!disabled }}
    >
      <Text style={[styles.buttonText, { color: primary ? theme.onPrimary : theme.foreground }]}>{label}</Text>
    </Pressable>
  );
}

// The approval card's measures (A9·b, A2·A): raised with a hairline, 14 by 16 pt, 18 pt round, 12 pt between parts; the
// exact text on the fill, 12 pt round; 38 pt buttons that share the width.
const styles = StyleSheet.create({
  card: { paddingVertical: 14, paddingHorizontal: 16, gap: 12, borderRadius: 18, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth },
  head: { gap: 2 },
  what: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
  fact: { flexDirection: "row", gap: 10 },
  factLabel: { fontSize: 13, lineHeight: 19, fontFamily: font.regular, width: 64 },
  factValue: { flex: 1, fontSize: 13, lineHeight: 19, fontFamily: font.regular },
  exact: { padding: 12, borderRadius: 12, borderCurve: "continuous" },
  input: { minHeight: 110, paddingHorizontal: 12, paddingVertical: 8, borderRadius: 12, borderCurve: "continuous", fontSize: 15, lineHeight: 20, textAlignVertical: "top" },
  list: { gap: 6 },
  row: { gap: 2, paddingVertical: 8, paddingHorizontal: 10, borderRadius: 12, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth },
  body: { ...text.base, fontFamily: font.regular },
  small: { ...text.sm, fontFamily: font.regular },
  underline: { textDecorationLine: "underline" },
  actions: { flexDirection: "row", alignItems: "center", gap: 8 },
  button: { flex: 1, height: 38, paddingHorizontal: 12, borderRadius: 12, borderCurve: "continuous", alignItems: "center", justifyContent: "center" },
  disabled: { opacity: 0.5 },
  buttonText: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
});
