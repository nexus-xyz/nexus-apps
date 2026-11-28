import { AGENT_MAX_CHARS, errandName, safeUrl, type AgentClient, type AgentErrandAction, type AgentErrandView } from "@nexus/kit";
import { useNavigation } from "expo-router";
import { DrawerActions } from "expo-router/react-navigation";
import { Fragment, useEffect, useRef, useState, type ComponentRef } from "react";
import { ActivityIndicator, AccessibilityInfo, Alert, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { ScreenKeyboardAvoidingView } from "@/screen-keyboard";
import { ACTION_LABEL, actedText, deadlineText, errandEntries, errandStateText, inProject, replyPlaceholder, surfaceName, whereToAnswer } from "@/agent-errands";
import type { DecideWrite, EditWrite } from "@/approval-card";
import { DayMarker, NexusDraft, NexusTurn, PersonBubble } from "@/assistant-bubbles";
import { AssistantMark } from "@/assistant-mark";
import { layoutThread } from "@/assistant-texting";
import { useCore } from "@/auth";
import { haloOf } from "@/composer-halo";
import { ComposerSend, composerControls } from "@/composer-controls";
import { LINE, linesOf, MAX_LINES } from "@/composer-text";
import { TextInput } from "@/controls";
import { Composer } from "@/conversation-composer";
import { useErrandMenu } from "@/errand-menu";
import { Glass } from "@/glass";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Blank } from "@/list";
import { linkHost, open as openLink } from "@/markdown";
import { nameOf, useModels } from "@/models";
import { CHROME_FONT_SCALE } from "@/text-size";
import { font, text, useTheme, type Theme } from "@/theme";
import { BAR_HEIGHT, SidebarButton, TopBar } from "@/top-bar";

/** The input's padding, top and bottom: one 22 pt line then fills the 44 pt row beside Send. */
const INPUT_PAD = 22;
/** The dock's height before it is measured: the composer's padding (6 + 4 + 6), its 44 pt row, and the gap under it. */
const ONE_LINE_DOCK = 6 + 4 + 44 + 6 + 6;

type Read = { kind: "loading" } | { kind: "view"; view: AgentErrandView } | { kind: "problem"; text: string };

/**
 * One errand as its own thread (A12 "An errand", A2): the bar names it, "Dinner Friday · errand", over its
 * state; the thread is its goal, the steps Nexus took as quiet notes, and the replies with their cards, which approve
 * here as in the main chat; then what it needs, a draft's Confirm and Cancel, and how it ended. While it is open, the
 * composer replies about it (`POST /agent/errands/{id}/messages`). More in the bar opens its menu: Pin, Rename, Mark
 * done, Archive (`errand-menu.tsx`). The thread's words are the person's and the agent's own, drawn as the chat draws
 * them.
 */
export function ErrandThread({ agent, id }: { agent: AgentClient; id: string }) {
  const core = useCore();
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const models = useModels(core);
  const name = (model: string) => nameOf(models, model);
  const top = insets.top + BAR_HEIGHT;
  const [dock, setDock] = useState(() => ONE_LINE_DOCK + Math.max(insets.bottom, 12));
  const [read, setRead] = useState<Read>({ kind: "loading" });
  const [draftText, setDraftText] = useState("");
  const [sending, setSending] = useState<string | undefined>();
  const [working, setWorking] = useState(false);
  const [note, setNote] = useState<string | undefined>();
  const [focused, setFocused] = useState(false);
  const [lines, setLines] = useState(1);
  const scroll = useRef<ComponentRef<typeof ScrollView>>(null);
  const alive = useRef(true);
  const view = read.kind === "view" ? read.view : undefined;
  const menu = useErrandMenu(agent, (next) => setRead({ kind: "view", view: next }));
  const decide: DecideWrite = (card, decision, options) => agent.decideWrite(card, decision, options);
  const edit: EditWrite = (card, changes) => agent.editWrite(card, changes);

  useEffect(() => {
    alive.current = true;
    const controller = new AbortController();
    setRead({ kind: "loading" });
    agent.errand(id, controller.signal).then(
      (result) => {
        if (controller.signal.aborted) return;
        if (result.kind === "errand") setRead({ kind: "view", view: result.view });
        else setRead({ kind: "problem", text: result.kind === "gone" ? "That errand is gone. It was deleted, or it is not yours." : result.failure.detail || "Your errand could not be read. Try again in a moment." });
      },
      () => !controller.signal.aborted && setRead({ kind: "problem", text: "Nexus could not reach the Assistant. Try again in a moment." }),
    );
    return () => {
      alive.current = false;
      controller.abort();
    };
  }, [agent, id]);

  const reply = () => {
    const message = draftText.trim();
    if (!message || sending !== undefined || !view) return;
    haptic("send");
    setDraftText("");
    setSending(message);
    setNote(undefined);
    agent.errandReply(view.errand.id, message).then(
      async (result) => {
        if (!alive.current) return;
        if (result.kind === "failed") {
          haptic("error");
          setDraftText(message);
          setNote(result.failure.detail || "Your reply was not sent. Try again in a moment.");
          return setSending(undefined);
        }
        haptic("receive");
        // The reply carries the errand as it is now; an API that sends no errand is read again.
        const next = result.reply.errand ?? (await agent.errand(view.errand.id).then((again) => (again.kind === "errand" ? again.view : undefined), () => undefined));
        if (!alive.current) return;
        if (next) setRead({ kind: "view", view: next });
        setSending(undefined);
        AccessibilityInfo.announceForAccessibility("Nexus answered.");
      },
      () => {
        if (!alive.current) return;
        haptic("error");
        setDraftText(message);
        setNote("Nexus could not reach the Assistant. Try again in a moment.");
        setSending(undefined);
      },
    );
  };

  const act = (action: AgentErrandAction) => {
    if (!view || working) return;
    if (action === "cancel") {
      const hint = view.errand.need === "confirm" ? "The draft is deleted." : "Nexus stops working on it.";
      return Alert.alert("Cancel this errand?", hint, [
        { text: "Keep it", style: "cancel" },
        { text: ACTION_LABEL.cancel, style: "destructive", onPress: () => send(view.errand.id, action) },
      ]);
    }
    send(view.errand.id, action);
  };

  const send = (errand: string, action: AgentErrandAction) => {
    setWorking(true);
    agent.errandAction(errand, action).then(
      (result) => {
        if (!alive.current) return;
        setWorking(false);
        if (result.kind === "failed") return setNote(result.failure.detail || "That was not done. Try again in a moment.");
        setNote(actedText(result.acted));
        if (result.acted.state !== "discarded") setRead({ kind: "view", view: result.acted.errand });
      },
      () => {
        if (!alive.current) return;
        setWorking(false);
        setNote("That was not done. Try again in a moment.");
      },
    );
  };

  const bar = <ErrandBar theme={theme} view={view} onMore={view ? () => menu.open(view.errand) : undefined} />;
  if (read.kind === "problem") {
    return (
      <View style={[styles.screen, { paddingTop: top }]}>
        <Blank title="This errand did not open" body={read.text} />
        {bar}
      </View>
    );
  }

  const open = !!view && view.actions.length > 0;
  const draft = !!view && view.actions.includes("confirm");
  const entries = view ? errandEntries(view) : [];
  const places = layoutThread(
    entries.map((entry) => ({ id: entry.key, side: entry.kind === "you" ? ("person" as const) : ("nexus" as const), at: entry.at || undefined })),
    Date.now(),
  );
  const where = view && whereToAnswer(view);
  const meta = view ? [`Started in ${surfaceName(view.surface)}`, view.errand.deadline ? deadlineText(view.errand.deadline) : undefined, view.budgetUsd ? `Budget $${view.budgetUsd}` : undefined].filter(Boolean).join(" · ") : "";
  const sendable = draftText.trim().length > 0;
  const evidence = view?.outcome?.evidence ?? [];

  return (
    <ScreenKeyboardAvoidingView style={styles.screen}>
      <View style={styles.screen}>
        <ScrollView
          ref={scroll}
          style={styles.screen}
          contentContainerStyle={[styles.feed, { paddingTop: top + 4, paddingBottom: (open && !draft ? dock : insets.bottom) + 16 }]}
          scrollIndicatorInsets={{ top, bottom: open && !draft ? dock : 0 }}
          keyboardDismissMode="interactive"
          keyboardShouldPersistTaps="handled"
          onContentSizeChange={() => scroll.current?.scrollToEnd({ animated: false })}
          accessibilityLabel={view ? `The thread of ${errandName(view.errand)}` : "Errand"}
        >
          {!view ? <ActivityIndicator style={styles.loading} color={theme.muted} accessibilityLabel="Loading the errand" /> : null}
          {view ? <Text style={[styles.meta, { color: theme.tertiary }]}>{meta}</Text> : null}
          {view?.constraints.map((constraint, i) => (
            <Text key={i} style={[styles.meta, { color: theme.tertiary }]}>
              {constraint}
            </Text>
          ))}
          {entries.map((entry, i) => {
            const time = places[i]?.day ? <DayMarker label={places[i].day!} theme={theme} /> : null;
            if (entry.kind === "you") {
              return (
                <Fragment key={entry.key}>
                  {time}
                  <PersonBubble text={entry.text} theme={theme} last={places[i]?.last} />
                </Fragment>
              );
            }
            if (entry.kind === "step") {
              const urls = entry.urls.map((url) => safeUrl(url)).filter((url): url is string => Boolean(url));
              return (
                <Fragment key={entry.key}>
                  {time}
                  <View style={styles.step} accessible accessibilityLabel={`Step: ${entry.text}`}>
                    <Text style={[styles.stepText, { color: theme.muted }]}>{entry.text}</Text>
                    {urls.map((url) => (
                      <Pressable key={url} onPress={() => openLink(url)} accessibilityRole="link" hitSlop={6}>
                        <Text style={[styles.link, { color: theme.tertiary }]}>{linkHost(url) ?? url}</Text>
                      </Pressable>
                    ))}
                  </View>
                </Fragment>
              );
            }
            return (
              <Fragment key={entry.key}>
                {time}
                <NexusTurn messages={entry.messages} name={name} line={false} theme={theme} decide={decide} edit={edit} />
              </Fragment>
            );
          })}
          {sending !== undefined ? (
            <>
              <PersonBubble text={sending} theme={theme} fresh />
              <NexusDraft draft={{ text: "" }} name={name} theme={theme} />
            </>
          ) : null}
          {view?.errand.need === "approval" && view.errand.detail ? (
            <Text style={[styles.need, { color: theme.foreground, backgroundColor: theme.fill }]}>Asks first: {view.errand.detail}</Text>
          ) : null}
          {draft ? <Text style={[styles.need, { color: theme.foreground, backgroundColor: theme.fill }]}>This errand is a draft. Confirm it, and Nexus starts on it.</Text> : null}
          {where ? <Text style={[styles.line, { color: theme.muted }]}>{where}</Text> : null}
          {evidence.map((item, i) => {
            const url = item.kind === "link" ? safeUrl(item.url) : undefined;
            return url ? (
              <Pressable key={i} onPress={() => openLink(url)} accessibilityRole="link" hitSlop={6}>
                <Text style={[styles.link, { color: theme.muted }]}>{linkHost(url) ?? url}</Text>
              </Pressable>
            ) : (
              <Text key={i} style={[styles.line, { color: theme.muted }]}>
                {item.kind === "citation" ? item.text : item.url}
              </Text>
            );
          })}
          {draft || view?.actions.includes("cancel") ? (
            <View style={styles.actions}>
              {draft ? <ActionPill label={ACTION_LABEL.confirm} primary theme={theme} disabled={working} onPress={() => act("confirm")} /> : null}
              {view?.actions.includes("cancel") ? <ActionPill label={ACTION_LABEL.cancel} danger theme={theme} disabled={working} onPress={() => act("cancel")} /> : null}
            </View>
          ) : null}
          {view && !open ? <Text style={[styles.line, styles.closed, { color: theme.tertiary }]}>This errand is closed. Ask Nexus in the main chat to start a new one.</Text> : null}
          {note ? (
            <Text style={[styles.line, styles.closed, { color: theme.muted }]} accessibilityRole="alert">
              {note}
            </Text>
          ) : null}
        </ScrollView>
        {bar}
        {view && open && !draft ? (
          <View style={styles.dock} pointerEvents="box-none" onLayout={(e) => setDock(e.nativeEvent.layout.height)}>
            <Composer theme={theme} halo={haloOf({ busy: sending !== undefined, chat: view.errand.id, sent: view.thread.length })} focused={focused} box={lines > 1}>
              <View style={styles.row}>
                <TextInput
                  value={draftText}
                  onChangeText={setDraftText}
                  onFocus={() => setFocused(true)}
                  onBlur={() => setFocused(false)}
                  onContentSizeChange={(e) => setLines(linesOf(e.nativeEvent.contentSize.height - INPUT_PAD))}
                  placeholder={replyPlaceholder(view.errand)}
                  placeholderTextColor={theme.muted}
                  multiline
                  maxLength={AGENT_MAX_CHARS}
                  editable={sending === undefined}
                  style={[styles.input, { color: theme.foreground }]}
                  accessibilityLabel={replyPlaceholder(view.errand)}
                />
                {sendable && sending === undefined ? (
                  <ComposerSend theme={theme} onPress={reply}>
                    <Icon name="send" size={16} color={theme.onPrimary} />
                  </ComposerSend>
                ) : null}
              </View>
            </Composer>
          </View>
        ) : null}
      </View>
      {menu.sheet}
    </ScreenKeyboardAvoidingView>
  );
}

/**
 * The errand's bar (A2, A12): the sidebar button; the mark over "Dinner Friday · errand" and its state; and More,
 * which opens its menu.
 */
function ErrandBar({ theme, view, onMore }: { theme: Theme; view?: AgentErrandView; onMore?: () => void }) {
  const navigation = useNavigation();
  const title = view ? errandName(view.errand) : "Errand";
  // Its state, and the project it is filed in (A14): "Working on it · in Fundraise".
  const state = view ? [errandStateText(view), inProject(view.errand)].filter(Boolean).join(" · ") : undefined;
  return (
    <TopBar
      left={<SidebarButton onPress={() => navigation.dispatch(DrawerActions.openDrawer())} />}
      center={
        <View style={styles.heading} accessible accessibilityRole="header" accessibilityLabel={[title, "errand", state].filter(Boolean).join(", ")}>
          <AssistantMark size={28} />
          <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {title} <Text style={{ color: theme.tertiary }}>· errand</Text>
          </Text>
          {state ? (
            <Text style={[styles.state, { color: theme.tertiary }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
              {state}
            </Text>
          ) : null}
        </View>
      }
      right={
        onMore ? (
          <Glass interactive style={styles.more}>
            <Pressable
              style={({ pressed }) => [styles.moreButton, pressed && styles.pressed]}
              onPress={() => {
                haptic("tap");
                onMore();
              }}
              accessibilityRole="button"
              accessibilityLabel={`More for ${title}`}
              hitSlop={4}
            >
              <Icon name="more" size={18} color={theme.foreground} />
            </Pressable>
          </Glass>
        ) : undefined
      }
      centerInset={64}
    />
  );
}

/** A Confirm or Cancel errand pill: filled for Confirm, outlined for Cancel, 44 pt high. */
function ActionPill({ label, primary, danger, theme, disabled, onPress }: { label: string; primary?: boolean; danger?: boolean; theme: Theme; disabled: boolean; onPress: () => void }) {
  return (
    <Pressable
      disabled={disabled}
      onPress={onPress}
      style={[styles.action, primary ? { backgroundColor: theme.primary } : { borderColor: theme.border, borderWidth: StyleSheet.hairlineWidth }, disabled && styles.idle]}
      accessibilityRole="button"
      accessibilityState={{ disabled }}
    >
      <Text style={[styles.actionText, { color: primary ? theme.onPrimary : danger ? theme.danger : theme.foreground }]}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  feed: { paddingHorizontal: 16 },
  loading: { marginTop: 24 },
  heading: { alignItems: "center", gap: 2, maxWidth: 240 },
  title: { fontFamily: font.regular, fontSize: 12, lineHeight: 15 },
  state: { fontFamily: font.regular, fontSize: 11, lineHeight: 14 },
  more: { height: 44, borderRadius: 22 },
  moreButton: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  pressed: { opacity: 0.5 },
  meta: { ...text.caption, fontFamily: font.regular, textAlign: "center", marginBottom: 4 },
  step: { alignItems: "center", gap: 2, paddingVertical: 6, paddingHorizontal: 24 },
  stepText: { ...text.xs, fontFamily: font.regular, textAlign: "center" },
  link: { ...text.caption, fontFamily: font.regular, textDecorationLine: "underline" },
  need: { ...text.sm, fontFamily: font.regular, padding: 12, borderRadius: 12, borderCurve: "continuous", overflow: "hidden", marginTop: 8 },
  line: { ...text.sm, fontFamily: font.regular, marginTop: 6 },
  closed: { textAlign: "center", marginTop: 12 },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: 8, marginTop: 12 },
  action: { minHeight: 44, paddingHorizontal: 16, borderRadius: 22, alignItems: "center", justifyContent: "center" },
  actionText: { ...text.sm, fontFamily: font.medium },
  idle: { opacity: 0.4 },
  dock: { position: "absolute", left: 0, right: 0, bottom: 0 },
  row: { flexDirection: "row", alignItems: "flex-end" },
  input: { ...composerControls.input, flex: 1, paddingTop: INPUT_PAD / 2, paddingBottom: INPUT_PAD / 2, maxHeight: LINE * MAX_LINES + INPUT_PAD },
});
