import { newIdempotencyKey } from "@nexus/kit";
import { cloudEventsToOpened, cloudSessionTitle } from "@nexus/kit/cloud-session-helpers";
import { syncedTimeline } from "@nexus/kit/synced-timeline";
import { router, Stack, useLocalSearchParams } from "expo-router";
import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, FlatList, Keyboard, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useCore, useStored } from "@/auth";
import { cloudError, cloudStatus } from "@/cloud-sessions";
import { foldTimeline, TRANSCRIPT_VIEWS, type TranscriptView } from "@/code-timeline";
import { TimelineItemView } from "@/code-timeline-view";
import { clientIdentity } from "@/connection-intent";
import { haptic } from "@/haptics";
import { Blank, Field, Footnote, Loading, Row, Section } from "@/list";
import { nameOf, useModels } from "@/models";
import { RemoteChoice } from "@/remote-settings-ui";
import { ScreenKeyboardAvoidingView } from "@/screen-keyboard";
import { Sheet, SheetTitle } from "@/sheet";
import { font, text, useTheme } from "@/theme";
import { useCloudSession } from "@/use-cloud-session";

export default function CloudSession() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const core = useCore();
  return <CloudTranscript key={`${clientIdentity(core)}:${id}`} id={id} />;
}

function CloudTranscript({ id }: { id: string }) {
  const core = useCore();
  const insets = useSafeAreaInsets();
  const theme = useTheme();
  const models = useModels(core);
  const view = useCloudSession(core, id);
  const [showTranscriptViews, setShowTranscriptViews] = useState(false);
  const savedView = useStored<TranscriptView>("choice", `cloud-transcript-view:${id}`);
  const [transcriptView, setTranscriptView] = useState(() => {
    const saved = savedView.load();
    return TRANSCRIPT_VIEWS.find(({ id }) => id === saved) ?? TRANSCRIPT_VIEWS[0];
  });
  const session = view.session;
  const active = session?.state === "running" || session?.state === "starting";
  const [draft, setDraft] = useState("");
  const [actionError, setActionError] = useState("");
  const [busy, setBusy] = useState<"send" | "stop">();
  const pending = useRef(false);
  const mounted = useRef(false);
  const sendKey = useRef<{ text: string; key: string } | undefined>(undefined);
  const stopKey = useRef<string | undefined>(undefined);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const converted = useMemo(() => cloudEventsToOpened(view.page?.events ?? [], {
    threadId: session?.threadId ?? undefined, title: session?.title, repo: session?.repo,
  }), [view.page, session?.threadId, session?.title, session?.repo]);
  const timeline = useMemo(() => syncedTimeline(converted.opened), [converted]);
  const items = useMemo(() => foldTimeline(timeline.events, active), [timeline, active]);
  const images = useMemo(() => new Map(timeline.images.map((image) => [image.id, image])), [timeline]);
  const run = async (kind: "send" | "stop") => {
    if (pending.current || !active) return;
    const message = draft.trim();
    if (kind === "send" && !message) return;
    pending.current = true;
    setBusy(kind);
    setActionError("");
    Keyboard.dismiss();
    try {
      if (kind === "send") {
        if (sendKey.current?.text !== message) sendKey.current = { text: message, key: newIdempotencyKey() };
        haptic("send");
        await core.sendCloudSessionMessage(id, { text: message }, sendKey.current.key);
        sendKey.current = undefined;
        if (mounted.current) setDraft("");
      } else {
        stopKey.current ??= newIdempotencyKey();
        await core.stopCloudSession(id, stopKey.current);
        stopKey.current = undefined;
        if (mounted.current) haptic("success");
      }
      if (mounted.current) view.refresh();
    } catch (error) {
      if (mounted.current) { setActionError(cloudError(error)); haptic("error"); }
    } finally {
      pending.current = false;
      if (mounted.current) setBusy(undefined);
    }
  };
  if (!session) return <>
    <Stack.Screen options={{ title: "Cloud session" }} />
    {view.error ? <Blank title="Couldn’t open Cloud session" body={view.error} action={{ label: "Retry", onPress: view.refresh }} /> : <Loading alone />}
  </>;
  return <ScreenKeyboardAvoidingView behavior="padding" style={[styles.screen, { backgroundColor: theme.screen }]}>
    <Stack.Screen options={{ title: cloudSessionTitle(session) }} />
    <FlatList data={items} keyExtractor={(item) => item.key} keyboardShouldPersistTaps="handled"
      contentContainerStyle={styles.transcript} onRefresh={view.refresh} refreshing={false}
      ListHeaderComponent={<View>
        <Text style={[styles.status, { color: theme.muted }]} accessibilityLiveRegion="polite">{`Cloud · ${session.repo} · ${cloudStatus(session)}`}</Text>
        {session.error || view.error || view.historyError ? <Footnote tone="danger">{session.error || view.error || view.historyError}</Footnote> : null}
        <Row label="All Cloud sessions" onPress={() => router.dismissTo("/cloud-sessions")} chevron />
        <Row label="Transcript view" value={transcriptView.label} chevron onPress={() => {
          Keyboard.dismiss();
          haptic("tap");
          setShowTranscriptViews(true);
        }} />
        {view.more ? <Row label={view.loadingEarlier ? "Loading earlier events…" : "Show earlier events"}
          disabled={view.loadingEarlier} onPress={() => void view.loadEarlier()} /> : null}
        {converted.omitted || converted.unreadable ? <Footnote>{converted.omitted + converted.unreadable} events are unavailable.</Footnote> : null}
      </View>}
      ListEmptyComponent={<Footnote>{view.page?.enabled === false ? "Cloud history storage is off for this organization." : "Transcript events will appear here."}</Footnote>}
      renderItem={({ item }) => <TimelineItemView item={item} images={images} model={(model) => nameOf(models, model)} view={transcriptView.id} />}
    />
    {active ? <View style={[styles.composer, { paddingBottom: insets.bottom, borderTopColor: theme.separator }]}>
      <Section footer={actionError || undefined} footerTone="danger">
        <Field value={draft} onChangeText={setDraft} placeholder="Follow up in Cloud" accessibilityLabel="Cloud session follow-up"
          editable={!busy} multiline maxLength={100_000} />
        <Row label={busy === "send" ? "Sending…" : "Send message"} tone="primary" disabled={!!busy || !draft.trim()} onPress={() => void run("send")} />
        <Row label={busy === "stop" ? "Stopping…" : "Stop session"} tone="danger" disabled={!!busy} onPress={() => Alert.alert(
          "Stop this Cloud session?", "The agent will stop working. Its saved history stays available.", [
            { text: "Cancel", style: "cancel" }, { text: "Stop", style: "destructive", onPress: () => void run("stop") },
          ],
        )} />
      </Section>
    </View> : null}
    <Sheet visible={showTranscriptViews} onClose={() => setShowTranscriptViews(false)}>
      <SheetTitle>Transcript view</SheetTitle>
      {TRANSCRIPT_VIEWS.map((choice) => (
        <RemoteChoice key={choice.id} label={choice.label} detail={choice.detail} selected={choice.id === transcriptView.id}
          onPress={() => {
            haptic("selection");
            setTranscriptView(choice);
            savedView.save(choice.id);
            setShowTranscriptViews(false);
          }} />
      ))}
    </Sheet>
  </ScreenKeyboardAvoidingView>;
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  transcript: { gap: 10, padding: 16 },
  status: { ...text.body, fontFamily: font.medium, paddingVertical: 8 },
  composer: { paddingHorizontal: 12, borderTopWidth: StyleSheet.hairlineWidth },
});
