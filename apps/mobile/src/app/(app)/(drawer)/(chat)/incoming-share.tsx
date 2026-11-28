import { router, useLocalSearchParams } from "expo-router";
import { useEffect, useState } from "react";
import { ActivityIndicator, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useCore } from "@/auth";
import { GlassButton } from "@/glass";
import { Icon } from "@/icon";
import { clearIncomingShare, incomingChatShares } from "@/incoming-chat-share";
import { Footnote, Page, Row, Section } from "@/list";
import { BAR_HEIGHT, TopBar } from "@/top-bar";
import { font, text, useTheme } from "@/theme";

type Session = { id: string; title: string };
type State = { kind: "loading" } | { kind: "ready"; sessions: Session[] } | { kind: "error" };

export default function IncomingShare() {
  const theme = useTheme();
  const core = useCore();
  const insets = useSafeAreaInsets();
  const { share } = useLocalSearchParams<{ share?: string }>();
  const [state, setState] = useState<State>({ kind: "loading" });
  const [retry, setRetry] = useState(0);
  const count = incomingChatShares().length;

  useEffect(() => {
    if (!count) return;
    const controller = new AbortController();
    setState({ kind: "loading" });
    void core.chatSessions({ limit: 12 }, controller.signal).then(
      (result) => { if (!controller.signal.aborted) setState({ kind: "ready", sessions: result.sessions }); },
      () => { if (!controller.signal.aborted) setState({ kind: "error" }); },
    );
    return () => controller.abort();
  }, [core, count, retry, share]);

  const toChat = (id: string) => router.replace({ pathname: "/session/[id]", params: { id, runOn: "chat", receiveShare: share ?? "1" } });
  const close = () => {
    clearIncomingShare();
    if (router.canGoBack()) router.back();
    else router.replace("/");
  };
  return (
    <View style={[styles.screen, { backgroundColor: theme.screen, paddingTop: insets.top + BAR_HEIGHT }]}>
      <Page>
        {count ? <>
          <Section footer={`${count} ${count === 1 ? "attachment" : "attachments"} will be added to your draft.`}>
            <Row label="New chat" icon="compose" chevron onPress={() => toChat("new")} />
          </Section>
          <Section title="Recent chats">
            {state.kind === "loading" ? <ActivityIndicator color={theme.muted} style={styles.loading} accessibilityLabel="Loading chats" /> : null}
            {state.kind === "error" ? <Row label="Couldn’t load chats. Try again." onPress={() => setRetry((n) => n + 1)} /> : null}
            {state.kind === "ready" ? state.sessions.map((item) => <Row key={item.id} label={item.title || "Untitled chat"} chevron onPress={() => toChat(item.id)} />) : null}
          </Section>
          {state.kind === "ready" && !state.sessions.length ? <Footnote>No recent chats yet.</Footnote> : null}
        </> : <Footnote>Share an image or a supported data file from another app.</Footnote>}
      </Page>
      <TopBar
        left={<GlassButton label="Cancel" onPress={close}><Icon name="close" size={18} color={theme.foreground} /></GlassButton>}
        center={<Text accessibilityRole="header" style={[styles.title, { color: theme.foreground }]}>Attach to a chat</Text>}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  title: { ...text.body, fontFamily: font.medium },
  loading: { padding: 12 },
});
