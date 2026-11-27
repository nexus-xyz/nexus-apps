import { originLabel } from "@nexus/kit/session-history";
import { syncedTimeline } from "@nexus/kit/synced-timeline";
import { router, useNavigation } from "expo-router";
import { DrawerActions } from "expo-router/react-navigation";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FlatList, Keyboard, type NativeScrollEvent, type NativeSyntheticEvent, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { ScreenKeyboardAvoidingView } from "@/screen-keyboard";
import { useCore, useStored } from "@/auth";
import { AnswerRatingFooter } from "@/answer-rating-footer";
import { answerRatingsOn, useAnswerRatings } from "@/use-answer-ratings";
import { CodeControls } from "@/code-controls";
import { useReader, useSyncedThread } from "@/code-reader";
import { agentName, costText, latestTitle, STATUS_WORDS, unreadCounts } from "@/code-sessions";
import { turnStartedAt } from "@/session-activity";
import { foldTimeline, TRANSCRIPT_VIEWS, type TimelineItem, type TranscriptView } from "@/code-timeline";
import { TimelineItemView } from "@/code-timeline-view";
import { Blank, Loading } from "@/list";
import { nameOf, useModels } from "@/models";
import { font, text, useTheme } from "@/theme";
import { NativeSessionHeader } from "@/native-session-header";
import { useComputerSession, useOptionalComputerState } from "@/computers";
import { SettingsChip } from "@/remote-settings-ui";
import { handoffPrompt } from "@/session-handoff";
import { SessionSettingsSheet, type SessionSettingsPage } from "@/session-settings-sheet";
import { composerControls } from "@/composer-controls";
import { ProviderMark } from "@/provider-mark";
import { Icon } from "@/icon";
import { haptic } from "@/haptics";
import { LAST_SESSION_PLACE, type SessionPlace } from "@/session-place";
import { terminalCommands } from "@/read-only-workbench";

/** How close to the end still counts as at the end: new events then scroll into view. */
const FOLLOW_SLACK = 80;

/**
 * One synced coding session, read on the phone (brand v5's P57·C, "Watching"). Its last 100 events, opened
 * here, then the live stream: each new event appears as the computer syncs it, and the timeline keeps to its end while
 * you are there. "Show earlier" reads back 100 at a time. The header names it, its repository and whether it is live.
 * Phone controls send signed, encrypted commands back to the computer when its owner enabled them.
 */
export function NativeConversation({ threadId }: { threadId: string }) {
  const core = useCore();
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const navigation = useNavigation();
  const [details, setDetails] = useState<SessionSettingsPage | null>(null);
  const savedView = useStored<TranscriptView>("choice", `transcript-view:${threadId}`);
  const [transcriptView, setTranscriptView] = useState<TranscriptView>(() => TRANSCRIPT_VIEWS.find(({ id }) => id === savedView.load())?.id ?? "normal");
  const pickTranscriptView = (next: TranscriptView) => {
    haptic("selection");
    setTranscriptView(next);
    savedView.save(next);
    setDetails(null);
  };
  const models = useModels(core);
  const reader = useReader(core);
  const ready = reader.state.kind === "ready" ? reader.state : undefined;
  const view = useSyncedThread(core, ready, threadId);
  const running = view.thread?.status === "running";
  const last = useStored<SessionPlace>("place", LAST_SESSION_PLACE);
  const loaded = view.thread?.threadId;
  useEffect(() => {
    if (view.deleted || view.missing) {
      const saved = last.load();
      if (saved?.source === "synced" && saved.id === threadId) last.remove();
    } else if (loaded === threadId) {
      last.save({ id: threadId, source: "synced" });
    }
  }, [last, loaded, threadId, view.deleted, view.missing]);
  const [viewer, setViewer] = useState<{ core: typeof core; personId: string }>();
  useEffect(() => {
    const request = new AbortController();
    void core.me(request.signal).then(({ user }) => {
      if (!request.signal.aborted && user) setViewer({ core, personId: user.id });
    }, () => undefined);
    return () => request.abort();
  }, [core]);
  const ownThread = viewer?.core === core && viewer.personId === view.thread?.personId;


  const timeline = useMemo(() => syncedTimeline(view.events), [view.events]);
  const items = useMemo(() => foldTimeline(timeline.events, running, timeline.turnEnds), [timeline, running]);
  const firstAnswer = items.find((item) => item.kind === "text" && item.answerSeq !== undefined);
  const ratings = useAnswerRatings(core, threadId, view.thread?.personId, firstAnswer?.kind === "text" ? firstAnswer.answerSeq : undefined);
  const images = useMemo(() => new Map(timeline.images.map((image) => [image.id, image])), [timeline.images]);
  const terminal = useMemo(() => terminalCommands(timeline.events), [timeline.events]);
  const title = latestTitle(view.events);
  const unread = unreadCounts(view.events);
  const computerId = title?.origin?.kind === "computer" ? title.origin.computerId : "";
  const remote = useComputerSession(computerId, threadId);
  const settingsState = useOptionalComputerState(remote.session);
  useEffect(() => { remote.session?.observeModel(view.thread?.model); }, [remote.session, view.thread?.model]);
  const described = useRef<object | undefined>(undefined);
  useEffect(() => {
    if (!details || details === "changes" || details === "terminal") { described.current = undefined; return; }
    if (details === "transcript") return;
    if (!remote.session || !settingsState.loaded || described.current === remote.session) return;
    described.current = remote.session;
    if (!settingsState.pending) void remote.session.describe();
  }, [details, remote.session, settingsState.loaded]);
  const openHandoffSession = useCallback((id: string) => {
    setDetails(null);
    router.push({ pathname: "/session/[id]", params: { id, source: "synced" } });
  }, []);
  useEffect(() => {
    const created = settingsState.created;
    if (!created?.ready) return;
    remote.session?.opened();
    openHandoffSession(created.threadId);
  }, [settingsState.created, remote.session, openHandoffSession]);
  const handoff = useMemo(() => ready && view.thread && !view.deleted && !view.missing && computerId && settingsState.settings
    ? { prompt: details === "handoff" ? handoffPrompt(items) : "", onOpen: openHandoffSession } : undefined,
  [ready, view.thread, view.deleted, view.missing, computerId, settingsState.settings, details, items, openHandoffSession]);
  const model = useCallback((id: string) => nameOf(models, id), [models]);

  // Keep to the end while the reader is there; leave them where they are once they scroll up.
  const list = useRef<FlatList<TimelineItem>>(null);
  const atEnd = useRef(true);
  const onScroll = (event: NativeSyntheticEvent<NativeScrollEvent>) => {
    const { contentOffset, contentSize, layoutMeasurement } = event.nativeEvent;
    atEnd.current = contentOffset.y + layoutMeasurement.height >= contentSize.height - FOLLOW_SLACK;
  };
  const first = useRef(true);
  const onContentSizeChange = () => {
    if (!atEnd.current) return;
    list.current?.scrollToEnd({ animated: !first.current });
    if (items.length) first.current = false;
  };

  const state = !view.thread ? null : running && view.live === "live" ? "Live" : running && view.live === "reconnecting" ? "Reconnecting…" : view.thread.status ? STATUS_WORDS[view.thread.status] : null;
  const origin = originLabel(title?.origin ?? { kind: "unknown" });
  const agentLine = [agentName(view.thread?.agent ?? null), view.thread?.model ? model(view.thread.model) : null].filter(Boolean).join(" · ") || null;
  const header = <NativeSessionHeader title={title?.title || "Session"} origin={origin} agent={agentLine} status={state} cost={costText(view.thread?.totals.costUsd)}
    onSidebar={() => navigation.dispatch(DrawerActions.openDrawer())} onDetails={() => setDetails("summary")} />;
  const startedAt = turnStartedAt(view.events);
  const settings = <SessionSettingsSheet page={details} onPage={setDetails} onClose={() => setDetails(null)} session={remote.session} state={settingsState} error={remote.error}
    title={title?.title || "Session"} origin={origin} project={title?.projectName} agent={agentName(view.thread?.agent ?? null)}
    transcriptView={transcriptView} onTranscriptView={pickTranscriptView}
    workbench={!!computerId && view.keyHeld === true} terminal={terminal} running={running} handoff={handoff} />;
  const selectedModel = settingsState.settings?.models.find((m) => m.id === settingsState.settings?.model)?.name ?? settingsState.settings?.model ?? view.thread?.model ?? "Model";
  const payer = settingsState.settings?.payer;
  const payerText = payer?.kind === "plan" ? "Plan" : payer?.kind === "apiKey" ? "API key" : payer?.kind === "gateway" ? "Nexus" : "Who pays";
  const openSettings = (page: SessionSettingsPage) => { Keyboard.dismiss(); setDetails(page); };
  const composerSettings = <>
    <Pressable onPress={() => openSettings("summary")} style={composerControls.target} accessibilityRole="button" accessibilityLabel="Session settings"><Icon name="more" size={18} color={theme.muted} /></Pressable>
    <SettingsChip label="Model" value={selectedModel} leading={<ProviderMark model={settingsState.settings?.model ?? view.thread?.model ?? "nexus"} size={14} />} onPress={() => openSettings("model")} />
    <SettingsChip label="Who pays" value={payer?.label ?? "Who pays"} displayValue={payerText} leading={<Icon name="members" size={14} color={theme.muted} />} onPress={() => openSettings("payer")} />
  </>;
  const gate = (body: React.ReactNode) => <View style={styles.screen}>{header}<View style={styles.screen}>{body}</View>{settings}</View>;

  const s = reader.state;
  const connect = { label: "Connect phone", onPress: () => router.push("/connect") };
  if (s.kind === "error") return gate(<Blank title="Couldn't load your devices" body={s.message} tone="danger" action={{ label: "Retry", onPress: reader.refresh }} />);
  if (s.kind !== "loading" && s.kind !== "ready") return gate(<Blank title="This phone can't read your sessions yet" body={PAIRING_WORDS[s.kind]} action={connect} />);
  if (view.deleted) return gate(<Blank title="This session was deleted" body="Nothing of it is kept on this phone." />);
  if (view.missing) return gate(<Blank title="This session isn't here" body="It was deleted, or it belongs to another organization." />);
  if (view.error && !view.thread) return gate(<Blank title="Couldn't load this session" body={view.error} tone="danger" action={{ label: "Retry", onPress: view.retry }} />);
  if (!view.thread) return gate(<Loading alone rows={5} />);

  const notes = [
    unread.noKey ? `${unread.noKey} ${unread.noKey === 1 ? "event" : "events"} this phone holds no key for: they were synced before it was paired.` : null,
    unread.local ? `${unread.local} ${unread.local === 1 ? "event stays" : "events stay"} on the computer, by your organization's policy.` : null,
    unread.unreadable ? `${unread.unreadable} ${unread.unreadable === 1 ? "event" : "events"} didn't open on this phone.` : null,
  ].filter((note): note is string => !!note);

  return (
    <ScreenKeyboardAvoidingView style={styles.screen} keyboardVerticalOffset={0}>
      {header}
      <FlatList
        ref={list}
        style={{ backgroundColor: theme.screen }}
        contentContainerStyle={[styles.content, { paddingBottom: 24 }]}
        data={items}
        keyExtractor={(item) => item.key}
        renderItem={({ item }) => {
          const timeline = <TimelineItemView item={item} images={images} model={model} view={transcriptView} outputActions={ownThread} />;
          if (!answerRatingsOn || item.kind !== "text" || item.answerSeq === undefined) return timeline;
          const seq = item.answerSeq;
          return <View>
            {timeline}
            <AnswerRatingFooter answer={item.text}
              audience={ratings.snapshot?.audience}
              saved={ratings.snapshot?.ratings.find((rating) => rating.answerSeq === seq)}
              pending={ratings.snapshot?.pending.find((rating) => rating.answerSeq === seq)?.value.rating}
              failed={ratings.failedAnswer === seq}
              onRating={(value) => ratings.put(seq, value)} />
          </View>;
        }}
        onScroll={onScroll}
        scrollEventThrottle={64}
        onContentSizeChange={onContentSizeChange}
        maintainVisibleContentPosition={{ minIndexForVisible: 0 }}
        ListHeaderComponent={
          <View style={styles.head}>
            {view.more ? (
              <Pressable onPress={view.loadOlder} disabled={view.loadingOlder} style={[styles.earlier, { backgroundColor: theme.fill }]} accessibilityRole="button" hitSlop={6}>
                <Text style={[styles.earlierText, { color: theme.foreground }]}>{view.loadingOlder ? "Loading…" : "Show earlier"}</Text>
              </Pressable>
            ) : null}
            {notes.map((note) => (
              <Text key={note} style={[styles.note, { color: theme.tertiary }]}>
                {note}
              </Text>
            ))}
          </View>
        }
        ListEmptyComponent={<Text style={[styles.note, { color: theme.tertiary }]}>Nothing to show yet. It appears here as the session runs.</Text>}
        keyboardShouldPersistTaps="handled"
      />
      {ready ? <CodeControls key={threadId} core={core} threadId={threadId} reader={ready.reader} events={timeline.events} running={running}
        unlocked={view.keyHeld === true} checkingKey={view.keyHeld === undefined} bottom={insets.bottom} settings={composerSettings} startedAt={startedAt} /> : null}
      {settings}
    </ScreenKeyboardAvoidingView>
  );
}

/** What an unpaired, pending, expired or revoked phone does next. */
const PAIRING_WORDS = {
  unpaired: "Connect this phone to a computer that runs Nexus.",
  pending: "Approve this phone on your computer, or connect it again.",
  expired: "This phone's approval expired. Connect it again.",
  revoked: "This phone was removed from your devices. Connect it again.",
} as const;

const styles = StyleSheet.create({
  screen: { flex: 1 },
  content: { paddingHorizontal: 16, paddingTop: 16, gap: 14 },
  head: { gap: 8, alignItems: "center" },
  earlier: { minHeight: 44, paddingHorizontal: 16, borderRadius: 18, justifyContent: "center", marginBottom: 6 },
  earlierText: { ...text.sm, fontFamily: font.medium },
  note: { ...text.caption, fontFamily: font.regular, textAlign: "center" },
});
