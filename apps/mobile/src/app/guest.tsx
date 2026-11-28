import { createCoreClient } from "@nexus/kit";
import * as Crypto from "expo-crypto";
import { router } from "expo-router";
import * as SecureStore from "expo-secure-store";
import { useEffect, useMemo, useRef, useState, type ComponentRef } from "react";
import { ActivityIndicator, Alert, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { KeyboardAvoidingView } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { consentText } from "@/ai-consent";
import { TextInput } from "@/controls";
import { END, endOf, GuestChatError, guestChat, noteOf, problemOf, type GuestRefusal, type GuestRound, type GuestStatus } from "@/guest";
import { useGuestChallenge } from "@/guest-challenge";
import { Icon } from "@/icon";
import { DEFAULT_GATEWAY } from "@/links";
import { Markdown } from "@/markdown";
import { NexusMark } from "@/nexus-mark";
import { ripple } from "@/press";
import { font, text, useTheme, type Theme } from "@/theme";

// The guest id, made once and kept on this device only, so the round count survives a restart (ADR 0019).
const ID_KEY = "nexus.guest.id";
const STORE = { keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY };

async function keptGuestId(): Promise<string> {
  const kept = await SecureStore.getItemAsync(ID_KEY, STORE).catch(() => null);
  if (kept) return kept;
  const made = Crypto.randomUUID();
  await SecureStore.setItemAsync(ID_KEY, made, STORE).catch(() => undefined);
  return made;
}

// Keyless: the guest routes need no sign-in. Writes carry an Idempotency-Key, which Gateway requires.
const core = createCoreClient({ baseUrl: DEFAULT_GATEWAY, idempotency: true });

/** App Review 5.1.2(i): a guest's messages go to the guest model's provider too, so the first send asks. */
const askConsent = () =>
  new Promise<boolean>((resolve) => {
    const { title, message } = consentText([], false);
    Alert.alert(title, message, [
      { text: "Don't Allow", style: "cancel", onPress: () => resolve(false) },
      { text: "Allow", onPress: () => resolve(true) },
    ]);
  });

/**
 * Guest Home, behind `EXPO_PUBLIC_NEXUS_GUEST=on`: a signed-out person chats a few rounds through
 * Gateway's guest routes, as on Nexus UI's guest Home, then the composer turns into a sign-in card. The chat lives in
 * this screen only.
 */
export default function GuestHome() {
  const theme = useTheme();
  const styles = useMemo(() => stylesFor(theme), [theme]);
  const insets = useSafeAreaInsets();
  const challenge = useGuestChallenge();
  const [chat, setChat] = useState<ReturnType<typeof guestChat> | null>(null);
  const [status, setStatus] = useState<GuestStatus | null | undefined>(undefined);
  const [thread, setThread] = useState<GuestRound[]>([]);
  const [draft, setDraft] = useState("");
  const [answer, setAnswer] = useState<string | null>(null);
  const [ended, setEnded] = useState<GuestRefusal | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const consented = useRef(false);
  const scroll = useRef<ComponentRef<typeof ScrollView>>(null);

  useEffect(() => {
    const abort = new AbortController();
    void keptGuestId().then(async (id) => {
      const next = guestChat(core, id);
      setChat(next);
      try {
        setStatus(await next.status(abort.signal));
      } catch (error) {
        if (abort.signal.aborted) return;
        if (error instanceof GuestChatError && END[error.reason]) setEnded(error.reason);
        else setStatus(null);
      }
    });
    return () => abort.abort();
  }, []);

  const end = endOf(ended, status, challenge.off);
  const busy = answer !== null;
  const { prepare } = challenge;
  // A token ready before the first send, and after each round for the next (ADR 0019 amendment).
  useEffect(() => {
    if (status && !end && !busy) prepare();
  }, [status, end, busy, prepare]);

  const send = async () => {
    const question = draft.trim();
    if (!question || busy || end || !chat || !status) return;
    if (!consented.current) {
      if (!(await askConsent())) return;
      consented.current = true;
    }
    const next: GuestRound[] = [...thread, { role: "user", content: question }];
    setThread(next);
    setDraft("");
    setProblem(null);
    setAnswer("");
    let written = "";
    try {
      const counted = await chat.send(next, await challenge.next(), (piece) => {
        written += piece;
        setAnswer(written);
      });
      setThread([...next, { role: "assistant", content: written }]);
      if (counted) setStatus({ ...status, ...counted });
    } catch (error) {
      const reason = error instanceof GuestChatError ? error.reason : "failed";
      if (END[reason]) setEnded(reason);
      else {
        setProblem(problemOf(reason));
        setThread(thread);
        setDraft(question);
      }
    } finally {
      setAnswer(null);
    }
  };

  return (
    <KeyboardAvoidingView behavior="padding" style={[styles.screen, { paddingTop: insets.top, paddingBottom: insets.bottom }]}>
      <View style={styles.bar}>
        <NexusMark size={24} />
        <Pressable accessibilityRole="button" hitSlop={8} onPress={() => router.push("/sign-in")}>
          <Text style={styles.link}>Sign in</Text>
        </Pressable>
      </View>
      <ScrollView ref={scroll} contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled" onContentSizeChange={() => scroll.current?.scrollToEnd()}>
        {thread.length === 0 ? (
          <View style={styles.head}>
            <NexusMark size={32} />
            <Text style={styles.greeting} accessibilityRole="header">
              What are you working on?
            </Text>
          </View>
        ) : (
          thread.map((round, n) =>
            round.role === "user" ? (
              <View key={n} style={styles.user}>
                <Text style={styles.userText}>{round.content}</Text>
              </View>
            ) : (
              <Markdown key={n} source={round.content} />
            ),
          )
        )}
        {busy && (answer ? <Markdown source={answer} /> : <Text style={styles.note}>Thinking…</Text>)}
      </ScrollView>
      {end ? (
        <View style={styles.card} accessibilityRole="summary">
          <Text style={styles.cardTitle} accessibilityRole="header">
            {END[end]!.title}
          </Text>
          <Text style={styles.note}>{END[end]!.body}</Text>
          <View style={styles.actions}>
            <Pressable style={[styles.button, styles.secondary]} android_ripple={ripple(theme)} accessibilityRole="button" onPress={() => router.push("/sign-in")}>
              <Text style={styles.buttonText}>Sign in</Text>
            </Pressable>
            <Pressable style={[styles.button, styles.primary]} android_ripple={ripple(theme)} accessibilityRole="button" onPress={() => router.push("/sign-in")}>
              <Text style={[styles.buttonText, { color: theme.onPrimary }]}>Create account</Text>
            </Pressable>
          </View>
        </View>
      ) : (
        <View style={styles.composer}>
          <TextInput
            style={styles.input}
            value={draft}
            onChangeText={setDraft}
            placeholder="Ask anything"
            placeholderTextColor={theme.muted}
            multiline
            maxLength={4000}
            editable={!busy}
            accessibilityLabel="Message"
          />
          {status === undefined ? (
            <ActivityIndicator color={theme.muted} />
          ) : (
            <Pressable
              style={[styles.send, (busy || !draft.trim()) && styles.inactive]}
              disabled={busy || !draft.trim()}
              accessibilityRole="button"
              accessibilityLabel="Send"
              onPress={() => void send()}
            >
              <Icon name="send" size={16} color={theme.onPrimary} />
            </Pressable>
          )}
        </View>
      )}
      {!end && status && (
        <Text style={[styles.note, styles.footnote]} accessibilityRole="text" accessibilityLiveRegion="polite">
          {problem ?? noteOf(thread.length, status)}
        </Text>
      )}
      {challenge.element}
    </KeyboardAvoidingView>
  );
}

function stylesFor(theme: Theme) {
  return StyleSheet.create({
    screen: { flex: 1, backgroundColor: theme.screen },
    bar: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: 20, paddingVertical: 12 },
    link: { ...text.body, fontFamily: font.medium, color: theme.foreground },
    content: { flexGrow: 1, gap: 16, paddingHorizontal: 20, paddingBottom: 16 },
    head: { flex: 1, alignItems: "center", justifyContent: "center", gap: 12 },
    greeting: { ...text.xxl, fontFamily: font.display, color: theme.foreground, textAlign: "center" },
    user: { alignSelf: "flex-end", maxWidth: "85%", borderRadius: 18, paddingHorizontal: 14, paddingVertical: 10, backgroundColor: theme.card },
    userText: { ...text.reading, fontFamily: font.regular, color: theme.foreground },
    composer: { flexDirection: "row", alignItems: "flex-end", gap: 8, marginHorizontal: 16, padding: 8, borderRadius: 24, borderWidth: StyleSheet.hairlineWidth, borderColor: theme.border, backgroundColor: theme.card },
    input: { ...text.body, flex: 1, maxHeight: 140, paddingHorizontal: 8, paddingVertical: 6, fontFamily: font.regular, color: theme.foreground },
    send: { width: 36, height: 36, borderRadius: 18, alignItems: "center", justifyContent: "center", backgroundColor: theme.primary },
    inactive: { opacity: 0.4 },
    note: { ...text.sm, fontFamily: font.regular, color: theme.muted },
    footnote: { textAlign: "center", paddingHorizontal: 24, paddingTop: 8 },
    card: { gap: 8, marginHorizontal: 16, padding: 20, borderRadius: 20, borderWidth: StyleSheet.hairlineWidth, borderColor: theme.border, backgroundColor: theme.card },
    cardTitle: { ...text.lg, fontFamily: font.medium, color: theme.foreground },
    actions: { flexDirection: "row", gap: 8, marginTop: 8 },
    button: { flex: 1, minHeight: 48, borderRadius: 24, alignItems: "center", justifyContent: "center", overflow: "hidden" },
    secondary: { borderWidth: StyleSheet.hairlineWidth, borderColor: theme.border },
    primary: { backgroundColor: theme.primary },
    buttonText: { ...text.body, fontFamily: font.medium, color: theme.foreground },
  });
}
