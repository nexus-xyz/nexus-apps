import { CHAT_FILE_LIMITS, CoreError, type ChatSessionMessage, type ChatSessionSharing } from "@nexus/kit";
import { LAST_SESSION_PLACE, type SessionPlace } from "@/session-place";
import * as Clipboard from "expo-clipboard";
import { randomUUID } from "expo-crypto";
import { haptic } from "@/haptics";
import { router, useLocalSearchParams, useNavigation } from "expo-router";
import { DrawerActions, useNavigationState } from "expo-router/react-navigation";
import { useEffect, useMemo, useRef, useState, type ComponentRef } from "react";
import { AccessibilityInfo, Alert, FlatList, Keyboard, Platform, Pressable, ScrollView, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import Reanimated, { interpolate, LayoutAnimationConfig, useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";
import { Gesture, GestureDetector } from "react-native-gesture-handler";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { agreeTo, providerName, providersFor } from "@/ai-consent";
import { answerText } from "@/answer-text";
import { useArtifacts, useAuth, useCore, useStored } from "@/auth";
import { isDataFile, resend, type ResentFile } from "@/chat-files";
import { addSuggestionToDraft, nextStepModel } from "@/chat-next-step";
import { ChatNextStep } from "@/chat-next-step-card";
import { fromStored, isImage, keepSent, readImage, toInput, type ComposerImage } from "@/chat-images";
import { haloOf } from "@/composer-halo";
import { Composer } from "@/conversation-composer";
import { ComposerPicker, ComposerSend, composerControls, INPUT_PAD } from "@/composer-controls";
import { ConsentSheet, keptConsent } from "@/consent-sheet";
import { ScreenKeyboardAvoidingView } from "@/screen-keyboard";
import { useScreenFocused } from "@/screen-focus";
import { contextUse } from "@/context";
import { ContextRing } from "@/context-ring";
import { TextInput } from "@/controls";
import { dictating, MicButton, MicMeter, MicNote, useDictation } from "@/dictation";
import { attachFrom, ImageTray, useComposerImages, type ImageSource } from "@/images";
import { ComposerEditor } from "@/composer-editor";
import { ComposerQuotes, type DraftQuote } from "@/composer-quotes";
import { ComposerReferences, pickContextFile, readContextFile, type ContextFile } from "@/composer-references";
import { EXPAND_FROM, LINE, LONG_PASTE, MAX_LINES, insertSlash, insertedRun, linesOf, pastedFile, referenceToken, slashQuery } from "@/composer-text";
import { ModelSheet } from "@/model-sheet";
import { clearIncomingShare, incomingChatShares } from "@/incoming-chat-share";
import { formatQuotedPrompt, readQuotedPrompt } from "@/quoted-prompt";
import { UNANSWERED } from "@/voice";
import { useVoiceMode, voiceAvailable, VoiceModeView } from "@/voice-mode";
import { PastedFiles, type Pasted } from "@/pasted-files";
import { cannotSendQueued, queueFailure, restoreQueue, storeQueue, type Queued, type StoredQueue } from "@/chat-queue";
import { QueueCard } from "@/queue-card";
import { Sheet, SheetRow } from "@/sheet";
import { COMMANDS, SlashMenu, useSkills, type SlashEntry } from "@/slash-menu";
import { pressedFill, ripple } from "@/press";
import { absoluteLink, audienceLabel } from "@/share";
import { ShareSheet, type ShareActions } from "@/share-sheet";
import { Dropdown } from "@/dropdown";
import { Glass, GlassButton, useGlass } from "@/glass";
import { greeting } from "@/greeting";
import { Icon } from "@/icon";
import { AnswerFoot, AnswerProse, Question, failed, messageStyles } from "@/messages";
import { AnswerSheet, type AnswerMenuTarget } from "@/answer-sheet";
import { reportable } from "@/report";
import { AUTO } from "@/meta";
import { nameOf, useModelCatalog } from "@/models";
import { splitCurrent } from "@/current-models";
import { ProviderMark } from "@/provider-mark";
import { DURATION, timing } from "@/motion";
import { swapIn, Thinking } from "@/thinking";
import { NexusMark } from "@/nexus-mark";
import { explainRefusal, freeStartPays, payLineParts, payLineText, payNotice, storedDetails, usageLine, type FreeStartModels, type Plan } from "@/plan";
import { PlanLine, PlanNotice } from "@/plan-notice";
import type { MessageAction } from "@/message-menu";
import { RestoreSheet, type Restorable } from "@/restore-sheet";
import { useToast } from "@/toast";
import { HistorySuggestions } from "@/history-suggestions";
import { RunOn } from "@/run-on";
import { takeTransferredDraft } from "@/connection-intent";
import { withExisting } from "@/share-inbox";
import { useSharedHandoff } from "@/use-share-inbox";
import { teamPicker, type MyTeams } from "@/teams";
import { BAR_HEIGHT, EdgeFade, SidebarButton, TopBar } from "@/top-bar";
import { font, text, useTheme, type Theme } from "@/theme";
import { ACCESSIBILITY_FONT_SCALE, CHROME_FONT_SCALE, isAccessibilitySize } from "@/text-size";
import { usePlan } from "@/use-plan";

/**
 * `previews`: a question's images as attached, until Gateway has them (never kept on the device). `details`: a refusal's
 * `error.nexus` as it happened, such as a failed payment's reason; a stored answer keeps its own in `error.details`.
 */
type Shown = Pick<ChatSessionMessage, "id" | "role" | "content" | "status"> &
  Partial<
    Pick<ChatSessionMessage, "model" | "mode" | "costUsd" | "requestedModel" | "attachments" | "error" | "usage" | "skills" | "connectors" | "sources" | "page" | "createdAt">
  > & {
    stopped?: boolean;
    previews?: ComposerImage[];
    details?: Record<string, unknown>;
  };

/**
 * A failed or refused answer the person can act on (a used-up Nexus usage, a missing key), in plain words. `free`: the
 * refused model and the models the free start pays for.
 */
const explained = (m: Shown, plan: Plan, free?: FreeStartModels) =>
  failed(m) ? explainRefusal({ code: m.error?.code, message: m.error?.message ?? m.content, details: m.details ?? storedDetails(m.error) }, plan, free) : undefined;


/**
 * A one-line composer's height above the home indicator, from `styles`: the dock's top padding (6), the pill's padding
 * (4 + 6), the input (a 22 pt line, padded 10 + 6) and the toolbar (a 44 pt chip).
 */
const ONE_LINE_COMPOSER = 6 + 4 + LINE + INPUT_PAD + 44 + 6;

/**
 * One chat. `new` is the home's composer: the session is created on the first send, as in Nexus UI. No navigation bar:
 * the sidebar's button, the title ("New session" until the first send), and the chat's actions float in glass over the
 * messages, and the composer floats in glass at the foot; the messages scroll under both. The composer is brand v5's
 * (P72, P65): it grows to five lines and opens the editor from the third, a long paste becomes a file, `/` lists your
 * skills (P98), the model is a sheet (P32), and while an answer runs Send queues and a long press sends now (P51).
 */
export default function Session() {
  const core = useCore();
  const theme = useTheme();
  const screenFocused = useScreenFocused();
  const { models, catalog } = useModelCatalog(core);
  const params = useLocalSearchParams<{ id: string; receiveShare?: string }>();
  // The home (`index`, no id) and `session/new` are a new chat.
  const [id, setId] = useState(!params.id || params.id === "new" ? null : params.id);
  // What this chat showed last time: it opens with it, then refreshes; with the unsent message and where you
  // were reading. A new chat's are under `new` until its first send names it.
  const cache = useStored<{ title: string; model: string; team?: string; messages: Shown[] }>("cache", `chat:${id ?? "new"}`);
  const kept = useStored<string>("draft", id ?? "new");
  const keptQuotes = useStored<DraftQuote[]>("draft", `quotes:${id ?? "new"}`);
  const [quotes, setQuotes] = useState<DraftQuote[]>(() => keptQuotes.load() ?? []);
  const [openQuote, setOpenQuote] = useState<DraftQuote>();
  const place = useStored<number>("place", id ?? "new");
  const lastSession = useStored<SessionPlace>("place", LAST_SESSION_PLACE);
  const [shown] = useState(() => (id ? cache.load() : undefined));
  const restored = useRef(false);
  const navigation = useNavigation();
  // Subscribe to this stack: a pop must restore the sidebar button even when the chat's data hasn't changed.
  // `canGoBack()` also counts an open sidebar.
  const pushed = useNavigationState((state) => state.index > 0);
  const insets = useSafeAreaInsets();
  const [title, setTitle] = useState(shown?.title ?? "");
  const { ui, uiLink, name } = useAuth();
  const artifacts = useArtifacts();
  /** Who the chat is shared with, when it is (its artifact's link and audience). */
  const [sharing, setSharing] = useState<ChatSessionSharing | undefined>();
  const [sheet, setSheet] = useState<ShareActions | null>(null);
  // The answer whose More menu is open (Copy, Report).
  const [answerMenu, setAnswerMenu] = useState<AnswerMenuTarget | null>(null);
  const [model, setModel] = useState(shown?.model ?? AUTO);
  const [picking, setPicking] = useState<"models" | "larger">();
  const [showContext, setShowContext] = useState(false);
  const modelChip = useRef<ComponentRef<typeof View>>(null);
  /**
   * Where the spend goes (a team's slug): the chat's own team, or the one you pick. Sent with a new chat and with every
   * message. Unset until then, so none is sent and Gateway uses your default team, as Nexus UI does.
   */
  const [team, setTeam] = useState(shown?.team);
  const [choosingTeam, setChoosingTeam] = useState(false);
  const teamChip = useRef<ComponentRef<typeof View>>(null);
  // Your teams and your default, as they were last time, until Gateway answers.
  const myTeams = useStored<MyTeams>("cache", "my-teams");
  const [mine, setMine] = useState<MyTeams>(() => myTeams.load() ?? {});
  const teams = teamPicker(mine, team);
  // The team chip says its team's name only when the name fits beside the model's; else its icon alone, as in a chat,
  // so the model's name keeps its room. From the chips' room and their width with every name in full.
  const [room, setRoom] = useState(0);
  const [natural, setNatural] = useState(0);
  const teamNamed = !id && (!room || !natural || natural <= room);
  const attached = useComposerImages();
  const [attaching, setAttaching] = useState(false);
  const attachChip = useRef<ComponentRef<typeof View>>(null);
  const content = useRef<ComponentRef<typeof View>>(null);
  const editorInput = useRef<ComponentRef<typeof TextInput>>(null);
  // Long input (P65): how many lines the box holds, the editor open, and long pastes held as files.
  const [lines, setLines] = useState(1);
  const [editing, setEditing] = useState(false);
  const [editorAttach, setEditorAttach] = useState(false);
  const [pastes, setPastes] = useState<Pasted[]>([]);
  const pasteRows = useRef(pastes);
  const attachmentDraft = useRef(0);
  const updatePastes = (next: Pasted[]) => {
    pasteRows.current = next;
    setPastes(next);
  };
  // Paste as text instead is offered until the next keystroke.
  const [offer, setOffer] = useState(false);
  const [referencePending, setReferencePending] = useState(false);
  const [dismissedReference, setDismissedReference] = useState<string>();
  const referenceRequest = useRef<symbol | undefined>(undefined);
  // Where the caret is, for `/` (P98): what follows a slash at the caret filters your skills.
  const [caret, setCaret] = useState(Number.MAX_SAFE_INTEGER);
  const skills = useSkills();
  // Queue or interrupt (P51): what waits for the answer to end, kept with the session; how the last answer ended, since
  // only one that finished picks up the queue; and Send's long-press menu.
  const queueStore = useStored<StoredQueue | string[]>("draft", `queue:${id ?? "new"}`);
  const [storedQueue] = useState(() => queueStore.load());
  const [queue, setQueue] = useState<Queued[]>(() => restoreQueue(storedQueue, randomUUID));
  const queueItems = useRef(queue);
  const ended = useRef<"done" | "failed" | "stopped" | null>(Array.isArray(storedQueue) ? null : storedQueue?.held ?? null);
  const [sendingQueue, setSendingQueue] = useState<string>();
  const dispatchingQueue = useRef(false);
  const persistQueue = (items: Queued[]) => {
    if (!id) return;
    if (items.length) queueStore.save(storeQueue(items, ended.current));
    else queueStore.remove();
  };
  const updateQueue = (change: (items: Queued[]) => Queued[]) => {
    const next = change(queueItems.current);
    queueItems.current = next;
    persistQueue(next);
    setQueue(next);
  };
  const [queueEdit, setQueueEdit] = useState<string | null>(null);
  const [sendMenu, setSendMenu] = useState(false);
  const sendTarget = useRef<ComponentRef<typeof View>>(null);
  // The newest of each line first, older ones behind "Other models"; Auto and the default stay in front, and the list
  // opens already expanded when the model in use is an older one.
  const picker = useMemo(() => splitCurrent(models, (m) => m.id === AUTO || !!m.default_for?.includes("chat")), [models]);
  const [messages, setMessages] = useState<Shown[]>(shown?.messages ?? []);
  const [draft, setDraft] = useState(() => takeTransferredDraft(core) ?? kept.load() ?? "");
  const currentDraft = useRef(draft);
  currentDraft.current = draft;
  // Shared from another app, into a new chat only: its words go after the unsent draft, which stays, and
  // its photos into the tray. Nothing is sent until Send.
  useSharedHandoff("chat", !id, (shared) => {
    const next = withExisting(currentDraft.current, shared.text);
    currentDraft.current = next;
    setDraft(next);
    kept.save(next);
    void attached.add(shared.images, shared.skipped);
    AccessibilityInfo.announceForAccessibility("What you shared is in your message. Nothing is sent until you tap Send.");
  });
  // Where the caret is, so dictated words go there (M38).
  const selection = useRef({ start: Number.MAX_SAFE_INTEGER, end: Number.MAX_SAFE_INTEGER });
  const dictation = useDictation((value, caret) => {
    setDraft(value);
    kept.save(value);
    selection.current = { start: caret, end: caret };
  });
  const answering = useRef<AbortController | null>(null);
  const retrying = useRef(false);
  // The consent question is open, or its providers' names are loading: a second tap meanwhile sends nothing.
  const asking = useRef(false);
  const [question, setQuestion] = useState<{ names: string[]; answer: (allow: boolean) => void } | null>(null);
  const [busy, setBusy] = useState(false);
  const [searchingWeb, setSearchingWeb] = useState(false);
  // The message field has focus: a new chat's ice halo brightens its edge, as Nexus UI's chat home does.
  const [focused, setFocused] = useState(false);
  const composerGesture = useMemo(() => Gesture.Native().disallowInterruption(true).shouldActivateOnStart(true), []);
  // Free or pay as you go, and what is left of the free start: a used-up one says so above the composer.
  const { plan, usage, notice, freeStartPays: onFreeStart, freeStartMaxUsd, refresh: refreshPlan } = usePlan();
  // While the free start pays, the highest output price it pays for: the picker marks those models.
  const freeStart = onFreeStart ? freeStartMaxUsd : undefined;
  // Set when the refresh failed but the chat as it was is on screen, so it is not taken for the current one.
  const [stale, setStale] = useState<string | null>(null);
  const [refreshed, setRefreshed] = useState<{ core: typeof core; id: string }>();
  const list = useRef<FlatList<Shown>>(null);
  const retryQuoteScroll = useRef(false);
  // Follow the answer down only while you are at the bottom; scrolled up to read, the view stays put.
  const atBottom = useRef(true);
  // How many messages the list last scrolled for. A message added after the first content (a question you send)
  // scrolls into view, animated. The first content only jumps, so a chat opens at its end; so do streamed words.
  const scrolledFor = useRef(0);
  // The composer (and a new chat's suggestions) float over the list's foot; the list ends above them. Until the dock
  // measures itself, a one-line composer's height, so the list does not jump when it does (a larger text size still
  // moves it once).
  const [dock, setDock] = useState(() => ONE_LINE_COMPOSER + Math.max(insets.bottom, 12));
  const [listHeight, setListHeight] = useState<number>();
  const largeText = isAccessibilitySize(useWindowDimensions().fontScale);
  const suggestionsVisible = !id && !messages.length && !draft && !attached.images.length && !pastes.length && !largeText;
  /** This chat's stored images, read once each. */
  const images = useMemo<ImageSource | undefined>(() => (id ? { key: `session:${id}`, load: (attachment) => core.chatAttachment(id, attachment) } : undefined), [core, id]);
  const sendable = (!!draft.trim() || attached.images.length > 0 || pastes.length > 0 || quotes.length > 0) && !attached.preparing && !referencePending;
  const newChat = () => router.replace({ pathname: "/session/[id]", params: { id: "new" } });

  useEffect(() => {
    core.me().then((me) => {
      const next = { teams: me.teams, team: me.team };
      setMine(next);
      myTeams.save(next);
    }, () => undefined);
  }, [core, myTeams]);

  useEffect(() => {
    if (!id) return;
    let current = true;
    core
      .chatSession(id)
      .then((session) => {
        if (!current) return;
        setRefreshed({ core, id });
        setStale(null);
        lastSession.save({ id: session.id, source: "chat" });
        setTitle(session.title);
        setModel(session.model);
        setTeam((t) => session.team ?? t);
        setSharing(session.sharing);
        // A rewound message stays saved for its author, out of the model's context: the chat shows what the model reads.
        setMessages(session.messages.filter((m) => !m.rewound));
      })
      // Offline with the chat kept: it stays as it was, with a line saying so, rather than the error in its place.
      .catch((e: Error) => {
        if (!current) return;
        const saved = lastSession.load();
        if (e instanceof CoreError && (e.status === 404 || e.status === 403) && saved?.source === "chat" && saved.id === id) lastSession.remove();
        setStale(e.message);
        setMessages((m) => (m.length ? m : [{ id: "error", role: "assistant", content: e.message, status: "failed" }]));
      });
    return () => { current = false; };
    // Once, for the chat opened from the list: a new one already shows what was sent.
  }, []);

  // Kept once an answer has settled; never mid-answer (the "question" and "answer" placeholders) or an error in its place.
  useEffect(() => {
    if (id && !busy && messages.length && !messages.some((m) => m.id === "question" || m.id === "answer" || m.id === "error")) {
      cache.save({ title, model, team, messages: messages.map(({ previews: _, ...m }) => m) });
    }
  }, [id, busy, messages, title, model, team, cache]);

  // Leaving the chat stops an answer still streaming into it.
  useEffect(() => () => answering.current?.abort(), []);

  /**
   * App Review 5.1.2(i): before the first send, ask whether the AI providers may have the messages and their
   * photos (`ConsentSheet`). `ai-consent.ts` keeps an Allow; after a decline nothing is sent and the draft stays.
   */
  async function consented(using = model, signal?: AbortSignal): Promise<boolean> {
    if (asking.current || signal?.aborted) return false;
    asking.current = true;
    try {
      const providers = providersFor(using, models);
      return await agreeTo(providers, keptConsent, async () => {
        // The providers' own names (Baseten, DeepInfra): the app's marks do not carry every host.
        const listed = providers.length ? await core.listProviders().then((r) => r.data, () => []) : [];
        if (signal?.aborted) return false;
        return new Promise<boolean>((answer) => setQuestion({ names: providers.map((id) => providerName(id, listed)), answer }));
      });
    } finally {
      asking.current = false;
    }
  }

  /**
   * Data files use the same path for new and retried questions. Retry can choose Auto instead of the selected model.
   */
  async function send(
    content = draft.trim(), sent: ComposerImage[] = attached.images, fromDraft = true, files: ResentFile[] = [],
    using = model, queued?: Queued,
  ) {
    // The ref, not `busy`: it is set before the first await, so a second tap in the same frame cannot start a second answer.
    if ((!content && !sent.length && !files.length) || answering.current) return;
    const request = queued?.attempt ?? { model: using, mode: "chat" as const, team };
    if (queued) setSendingQueue(queued.id);
    if (fromDraft) {
      updateQuotes([]);
      setDraft("");
      kept.remove();
      clearAttachments();
      setOffer(false);
    }
    haptic("send");
    // VoiceOver and TalkBack hear what the screen shows: the answer starting, then ready, failed or stopped.
    AccessibilityInfo.announceForAccessibility("Thinking…");
    setBusy(true);
    const controller = new AbortController();
    answering.current = controller;
    atBottom.current = true;
    setMessages((m) => [
      ...m,
      // Its files keep Gateway's refs, so the chat shows them and a second Retry reads them again.
      { id: "question", role: "user", content, status: "complete", previews: sent.length ? sent : undefined, attachments: files.length ? files.map((f) => f.ref) : undefined },
      { id: "answer", role: "assistant", content: "", status: "complete", mode: request.mode, requestedModel: request.model },
    ]);
    const replaceAnswer = (change: (a: Shown) => Shown) => setMessages((m) => m.map((x) => (x.id === "answer" ? change(x) : x)));
    let accepted = false;
    const acceptQueued = () => {
      if (!queued || accepted) return;
      accepted = true;
      updateQueue((items) => items.filter((item) => item.id !== queued.id));
    };
    try {
      const sessionId = id ?? (await core.createChatSession({ model: using, mode: "chat", team })).id;
      setId(sessionId);
      if (!controller.signal.aborted) lastSession.save({ id: sessionId, source: "chat" });
      const done = await core.streamChatMessage(
        sessionId,
        { content, ...request, ...(sent.length ? { images: sent.map(toInput) } : {}), ...(files.length ? { files: files.map((f) => f.input) } : {}) },
        (chunk) => {
          if (chunk.nexus?.question) acceptQueued();
          const delta = chunk.choices[0]?.delta.content;
          const tool = chunk.nexus?.tool;
          if (tool || delta) setSearchingWeb(tool?.name === "web_search");
          if (delta) replaceAnswer((a) => ({ ...a, content: a.content + delta }));
        },
        controller.signal,
        queued ? `mobile-queue:${queued.id}` : undefined,
      );
      acceptQueued();
      setRefreshed({ core, id: sessionId });
      setTitle(done.session.title);
      keepSent(sessionId, done.question.attachments, sent);
      setMessages((m) => [...m.filter((x) => x.id !== "question" && x.id !== "answer"), done.question, done.message]);
      ended.current = "done";
      haptic("success");
      AccessibilityInfo.announceForAccessibility("Answer ready.");
      return done.message;
    } catch (e) {
      const stopped = controller.signal.aborted;
      ended.current = stopped ? "stopped" : "failed";
      if (!stopped) haptic("error");
      AccessibilityInfo.announceForAccessibility(stopped ? "Stopped." : "The answer failed.");
      // The refusal's code, as a stored answer carries it, so it is explained the same way.
      const error = e instanceof CoreError && e.code ? { code: e.code, message: e.message, status: e.status } : undefined;
      const details = e instanceof CoreError ? e.details : undefined;
      if (queued && !accepted) {
        setMessages((items) => items.filter((item) => item.id !== "question" && item.id !== "answer"));
        const delivery = queueFailure(error?.code);
        if (delivery === "accepted") {
          acceptQueued();
          ended.current = "stopped";
          const refreshed = id ? await core.chatSession(id).catch(() => undefined) : undefined;
          if (refreshed) {
            setMessages(refreshed.messages.filter((item) => !item.rewound));
            setTitle(refreshed.title);
          }
          toast({ text: "Gateway already received this message." });
        } else {
          updateQueue((items) => items.map((item) => item.id === queued.id ? { ...item, delivery } : item));
          if (delivery === "unconfirmed") toast({ text: "Check the transcript to see if your message was sent.", haptic: "warning" });
          else if (error?.message) toast({ text: error.message, haptic: false });
        }
        return;
      }
      replaceAnswer((a) => ({ ...a, id: `answer-${Date.now()}`, content: stopped ? a.content : (e as Error).message, status: stopped ? "complete" : "failed", stopped, error, details }));
      setMessages((m) => m.map((x) => (x.id === "question" ? { ...x, id: `question-${Date.now()}` } : x)));
      // A used-up Nexus usage: read it again, so the composer's notice shows.
      if (!stopped && error && explainRefusal({ ...error, details }, plan)?.usedUp) refreshPlan();
    } finally {
      answering.current = null;
      if (queued) setSendingQueue(undefined);
      setSearchingWeb(false);
      setBusy(false);
    }
  }

  /** The chat's share menu, as Nexus UI's: share it with an audience or stop, who has access, its link, its Markdown, Nexus UI. */
  const shareActions = (sessionId: string): ShareActions => ({
    title: title || "This chat",
    audience: sharing ? audienceLabel(sharing.audience) : "Only you",
    // A public link's own address, as Nexus UI copies it; the members' page for every other audience.
    link: sharing && ui ? absoluteLink(sharing.publicUrl ?? sharing.url, ui) : undefined,
    shareable: { kind: "chat", id: sessionId },
    // Shared or revoked in the sheet: the chat's newest share, read again.
    changed: () => void core.chatSession(sessionId).then((s) => setSharing(s.sharing), () => undefined),
    stopSharing: sharing
      ? async () => {
          await artifacts.unshareChatSession(sessionId);
          setSharing(undefined);
        }
      : undefined,
    markdown: () => artifacts.exportChatSession(sessionId),
    openUrl: uiLink ? uiLink(`/sessions/${encodeURIComponent(sessionId)}`) : undefined,
  });

  /** The question Restore to here asks about. */
  const [restoring, setRestoring] = useState<Restorable | null>(null);
  const toast = useToast();
  /**
   * Restore to here (P52): the chat goes back to before the question (Gateway's rewind), and the question goes back in
   * the box to edit or send again.
   */
  async function restore(question: Restorable) {
    if (!id || answering.current) return;
    try {
      const session = await core.rewindChatSession(id, question.id);
      setMessages(session.messages.filter((m) => !m.rewound));
      const restored = readQuotedPrompt(question.content);
      const next = restored.quotes.map((content) => ({ id: randomUUID(), content }));
      updateQuotes(next);
      setDraft(restored.prompt);
      if (restored.prompt) kept.save(restored.prompt);
      else kept.remove();
      haptic("success");
    } catch (e) {
      haptic("error");
      Alert.alert("Couldn't restore the chat", (e as Error).message);
    }
  }
  // A sent question with Gateway's id; never the one still being sent.
  const restorable = (m: Shown) => !!id && !busy && !m.id.startsWith("question");
  // What the menu opens (a sheet) waits for iOS's context menu to finish closing: iOS drops a modal presented meanwhile.
  const afterMenu = (open: () => void) => setTimeout(open, Platform.OS === "ios" ? 350 : 0);
  /** A question's long-press menu (P47, P52): Copy, Restore to here, Retry for the last one, and Share. */
  const questionActions = (m: Shown): MessageAction[] => {
    if (!m.content) return [];
    const last = !busy && m === [...messages].reverse().find((x) => x.role === "user");
    const actions: (MessageAction | false)[] = [
      { key: "copy", label: "Copy", icon: { ios: "doc.on.doc", android: "content_copy" }, onPress: () => void Clipboard.setStringAsync(m.content).then(() => toast({ text: "Copied", icon: "copy" })) },
      restorable(m) && {
        key: "restore",
        label: "Restore to here",
        icon: { ios: "arrow.uturn.backward", android: "undo" },
        onPress: () => afterMenu(() => setRestoring({ id: m.id, content: m.content, createdAt: m.createdAt })),
      },
      last && { key: "retry", label: "Retry", icon: { ios: "arrow.clockwise", android: "refresh" }, onPress: () => void retry() },
      !!id && { key: "share", label: "Share", icon: { ios: "square.and.arrow.up", android: "share" }, onPress: () => afterMenu(() => setSheet(shareActions(id))) },
    ];
    return actions.filter((a): a is MessageAction => !!a);
  };
  // How full the chat's context is, from its latest answer (P53).
  const context = contextUse(messages, (m) => models.find((x) => x.id === m)?.context_window);
  const hasLargerModel = !!context && models.some((item) => (item.context_window ?? 0) > context.window);

  /**
   * Retry: drop the failed or stopped answer and its question, and ask the question again, with its images and data
   * files. `using`: the model, Auto when the free start does not pay for the one chosen.
   */
  async function retry(using = model) {
    const question = [...messages].reverse().find((m) => m.role === "user");
    // Its images and files may be read first: a second tap meanwhile does nothing.
    if (!question || answering.current || retrying.current) return;
    retrying.current = true;
    try {
      // Asked before the question leaves the list: a decline keeps the chat as it is.
      if (!(await consented(using))) return;
      let again = question.previews ?? [];
      let files: ResentFile[] = [];
      const stored = question.attachments ?? [];
      if (id && stored.length) {
        // A question sent here holds its images as previews; its data files stay Gateway's refs, read each time.
        try {
          [again, files] = await Promise.all([
            again.length ? again : Promise.all(stored.filter(isImage).map((a) => readImage(`session:${id}:${a.id}`, () => core.chatAttachment(id, a.id)).then((image) => fromStored(a, image)))),
            Promise.all(stored.filter(isDataFile).map((a) => core.chatAttachment(id, a.id).then((file) => resend(a, file)))),
          ]);
        } catch (e) {
          return Alert.alert("Couldn't read the question's attachments", (e as Error).message);
        }
      }
      // Nothing to send again (images kept from an earlier visit, never stored): the chat stays as it is.
      if (!question.content && !again.length && !files.length) return;
      // By id, not by object: a refresh during the read replaces the objects.
      setMessages((m) => {
        const at = m.map((x) => x.id).lastIndexOf(question.id);
        return at < 0 ? m : m.slice(0, at);
      });
      send(question.content, again, false, files, using);
    } finally {
      retrying.current = false;
    }
  }

  /**
   * A change of the draft. A paste over 2,000 characters becomes a file instead (P65), with Paste as text instead until
   * the next keystroke. The box was controlled with the old text, so it is shown the paste for a frame and then the
   * text without it, which it would otherwise keep.
   */
  const changeDraft = (value: string) => {
    currentDraft.current = value;
    setDismissedReference(undefined);
    const run = insertedRun(draft, value);
    if (run && run.text.length > LONG_PASTE && !referenceRequest.current && pasteRows.current.length < CHAT_FILE_LIMITS.count) {
      const file = pastedFile(run.text);
      if (file.bytes <= CHAT_FILE_LIMITS.bytes) {
        const before = draft;
        updatePastes([...pasteRows.current, { id: randomUUID(), text: run.text, start: run.start, ...file }]);
        setOffer(true);
        setDraft(value);
        requestAnimationFrame(() => setDraft((current) => current === value ? before : current));
        return;
      }
    }
    setOffer(false);
    setDraft(value);
    if (value) kept.save(value);
    else kept.remove();
  };
  /** The latest long paste back as text, where it was pasted. */
  const pasteAsText = () => {
    const last = pastes[pastes.length - 1];
    if (!last) return;
    const next = draft.slice(0, last.start) + last.text + draft.slice(last.start);
    updatePastes(pasteRows.current.filter((item) => item.id !== last.id));
    setOffer(false);
    setDraft(next);
    kept.save(next);
  };
  /** The draft's long pastes as the data files a question takes. */
  const pastedFiles = (): ResentFile[] =>
    pastes.map((p) => ({ ref: { id: p.id, type: "file", mediaType: p.mediaType ?? "text/markdown", bytes: p.bytes, name: p.name }, input: { data: p.data, name: p.name } }));
  /** Send, as the button does: dictation stops and its words go too, after the AI providers' consent. */
  const sendDraft = () => {
    if (referenceRequest.current) return;
    const said = dictation.finish()?.trim();
    void consented().then((ok) => (ok ? send(formatQuotedPrompt(said ?? draft, quotes.map((quote) => quote.content)), attached.images, true, pastedFiles()) : undefined));
  };

  const reference = focused && !editing && dismissedReference !== draft ? referenceToken(draft, caret) : undefined;
  useEffect(() => {
    setReferencePending(false);
    return () => {
      referenceRequest.current = undefined;
      attachmentDraft.current += 1;
    };
  }, [core]);
  const addReference = (load: () => Promise<ContextFile | undefined>, token?: ReturnType<typeof referenceToken>) => {
    if (referenceRequest.current) return;
    if (pasteRows.current.length >= CHAT_FILE_LIMITS.count) {
      toast({ text: `A message can include ${CHAT_FILE_LIMITS.count} files.`, haptic: "warning" });
      return;
    }
    haptic("selection");
    const owner = Symbol();
    referenceRequest.current = owner;
    setReferencePending(true);
    setOffer(false);
    const started = draft;
    void load().then((file) => {
      if (referenceRequest.current !== owner || !file) return;
      if (!file.bytes || file.bytes > CHAT_FILE_LIMITS.bytes) throw new Error("Choose a nonempty file up to 5 MB.");
      if (pasteRows.current.length >= CHAT_FILE_LIMITS.count) throw new Error(`A message can include ${CHAT_FILE_LIMITS.count} files.`);
      updatePastes([...pasteRows.current, { ...file, id: randomUUID(), start: 0, text: "" }]);
      if (token && currentDraft.current === started) {
        const before = started.slice(0, token.start);
        const after = started.slice(token.end);
        const next = before + (before.endsWith(" ") && after.startsWith(" ") ? after.slice(1) : after);
        setDraft(next);
        if (next) kept.save(next);
        else kept.remove();
        setCaret(before.length);
        selection.current = { start: before.length, end: before.length };
      }
      haptic("success");
    }).catch((error: unknown) => {
      if (referenceRequest.current !== owner) return;
      toast({ text: error instanceof Error ? error.message : "Couldn’t attach context. Try again.", haptic: "error" });
    }).finally(() => {
      if (referenceRequest.current !== owner) return;
      referenceRequest.current = undefined;
      setReferencePending(false);
    });
  };

  const consumedShare = useRef<string | undefined>(undefined);
  useEffect(() => {
    const receipt = params.receiveShare;
    if (!receipt || consumedShare.current === receipt) return;
    const items = incomingChatShares();
    if (!items.length) return;
    consumedShare.current = receipt;
    clearIncomingShare();
    const images = items.filter((item) => item.kind === "image");
    if (images.length) void attached.add(images);
    const files = items.filter((item) => item.kind === "file");
    if (!files.length) return;
    const owner = Symbol();
    referenceRequest.current = owner;
    setReferencePending(true);
    void (async () => {
      const errors: string[] = [];
      for (const file of files) {
        if (referenceRequest.current !== owner) return;
        if (pasteRows.current.length >= CHAT_FILE_LIMITS.count) {
          errors.push(`A message can include ${CHAT_FILE_LIMITS.count} files.`);
          break;
        }
        try {
          const data = await readContextFile(file.uri, file.name);
          if (referenceRequest.current !== owner) return;
          updatePastes([...pasteRows.current, { ...data, id: randomUUID(), start: 0, text: "" }]);
        } catch (error: unknown) {
          errors.push(error instanceof Error ? error.message : "Couldn’t attach the shared file.");
        }
      }
      if (referenceRequest.current === owner && errors.length) toast({ text: [...new Set(errors)].join(" "), haptic: "warning" });
    })().finally(() => {
      if (referenceRequest.current !== owner) return;
      referenceRequest.current = undefined;
      setReferencePending(false);
    });
  }, [params.receiveShare]);

  // `/` at the caret (P98): the menu over the composer while it has focus.
  const slash = focused && !editing ? slashQuery(draft, caret) : null;
  const pickSlash = (entry: SlashEntry) => {
    const query = slash ?? "";
    if (entry.kind === "command") {
      // A command runs: its word leaves the text.
      const start = caret - query.length - 1;
      const next = draft.slice(0, start) + draft.slice(caret);
      setDraft(next);
      if (next) kept.save(next);
      else kept.remove();
      haptic("selection");
      if (entry.name === "model") setPicking("models");
      if (entry.name === "context") setShowContext(true);
      return;
    }
    const picked = insertSlash(draft, caret, entry.name);
    setDraft(picked.text);
    kept.save(picked.text);
    setCaret(picked.caret);
    selection.current = { start: picked.caret, end: picked.caret };
  };

  useEffect(() => {
    persistQueue(queueItems.current);
  }, [busy, id, queueStore]);

  useEffect(() => {
    const first = queue[0];
    if (busy || !id || !first || cannotSendQueued(first) || queueEdit === first.id || answering.current || dispatchingQueue.current) return;
    if (!first.now && (ended.current !== "done" || first.delivery === "retry")) return;
    const attempt = first.attempt ?? { model, mode: "chat" as const, team };
    dispatchingQueue.current = true;
    void consented(attempt.model).then((allowed) => {
      dispatchingQueue.current = false;
      const next = queueItems.current[0];
      if (!next || next.id !== first.id) {
        if (next) setQueue([...queueItems.current]);
        return;
      }
      if (!allowed) {
        ended.current = "stopped";
        updateQueue((items) => items.map((item) => item.id === next.id ? { ...item, now: false } : item));
        return;
      }
      if (answering.current) return;
      const request = { ...next, attempt, now: false };
      updateQueue((items) => items.map((item) => item.id === next.id ? request : item));
      void send(next.content, next.images ?? [], false, next.files ?? [], attempt.model, request);
    }).catch(() => {
      dispatchingQueue.current = false;
      ended.current = "stopped";
      updateQueue((items) => items.map((item) => item.id === first.id ? { ...item, now: false } : item));
      toast({ text: "Couldn’t check access to the model. Try again.", haptic: "error" });
    });
  }, [busy, id, queue, queueEdit]);
  /** The draft into the queue: last (Queue after this step), or first, stopping the answer (Send now). */
  const queueDraft = (now = false) => {
    if (referenceRequest.current) return;
    const content = formatQuotedPrompt(draft, quotes.map((quote) => quote.content));
    if (!content && !attached.images.length && !pastes.length) return;
    haptic("send");
    const item: Queued = { id: randomUUID(), content, images: attached.images, files: pastedFiles(), now };
    updateQueue((q) => (now ? [item, ...q.map((entry) => ({ ...entry, now: false }))] : [...q, item]));
    updateQuotes([]);
    setDraft("");
    kept.remove();
    clearAttachments();
    setOffer(false);
    if (now) answering.current?.abort();
  };
  const sendQueuedNow = (queuedId: string) => {
    const queued = queueItems.current.find((item) => item.id === queuedId);
    if (!queued || cannotSendQueued(queued) || queued.id === sendingQueue) return;
    haptic("tap");
    updateQueue((q) => {
      const item = q.find((entry) => entry.id === queuedId);
      return item ? [{ ...item, now: true }, ...q.filter((entry) => entry.id !== queuedId).map((entry) => ({ ...entry, now: false }))] : q;
    });
    answering.current?.abort();
  };
  const removeQueued = (ids: string[]) => {
    const removed = new Set(ids.filter((item) => item !== sendingQueue));
    const previous = queue.filter((item) => removed.has(item.id));
    if (!previous.length) return;
    updateQueue((q) => q.filter((item) => !removed.has(item.id)));
    toast({
      text: previous.length === 1 ? "Removed queued message" : "Cleared queued messages",
      action: { label: "Undo", onPress: () => updateQueue((q) => {
        const current = new Set(q.map((item) => item.id));
        const restored = previous.filter((item) => !current.has(item.id)).map((item) => ({ ...item, now: false }));
        const next = queue.slice(queue.indexOf(previous[0]!) + 1).find((item) => current.has(item.id));
        const position = next ? q.findIndex((item) => item.id === next.id) : q.length;
        return [...q.slice(0, position), ...restored, ...q.slice(position)];
      }) },
    });
  };
  const moveQueued = (queuedId: string, direction: -1 | 1) => {
    haptic("selection");
    updateQueue((q) => {
      const index = q.findIndex((item) => item.id === queuedId);
      const target = index + direction;
      const item = q[index];
      const neighbor = q[target];
      if (!item || !neighbor || item.now || neighbor.now || item.id === sendingQueue || neighbor.id === sendingQueue) return q;
      return q.map((entry, at) => at === index ? neighbor : at === target ? item : entry);
    });
  };
  const saveQueued = (queuedId: string, value: string) => {
    updateQueue((q) => q.map((item) => item.id === queuedId && !item.attempt ? { ...item, content: value } : item));
    haptic("success");
  };
  const sendQueuedText = (queuedId: string) => {
    updateQueue((items) => items.map((item) => item.id === queuedId && !item.attempt ? { ...item, missingAttachments: false } : item));
    sendQueuedNow(queuedId);
  };
  const nextQueued = queue[0];
  const heldQueue = busy ? undefined : ended.current === "stopped" ? "Stopped"
    : ended.current === "failed" || nextQueued?.delivery === "retry" ? "The turn failed"
    : ended.current === null || (nextQueued && cannotSendQueued(nextQueued)) ? "Ready to send" : undefined;

  const clearAttachments = () => {
    attachmentDraft.current += 1;
    attached.clear();
    updatePastes([]);
  };
  const undoAttachment = (message: string, restore: () => boolean) => {
    const generation = attachmentDraft.current;
    toast({
      text: message,
      action: { label: "Undo", onPress: () => {
        if (attachmentDraft.current === generation && restore()) haptic("selection");
      } },
    });
  };
  const removeImage = (imageId: string) => {
    const removed = attached.remove(imageId);
    if (removed) undoAttachment("Image removed", () => attached.restore(removed));
  };
  const removeFile = (fileId: string) => {
    const index = pasteRows.current.findIndex((item) => item.id === fileId);
    const file = pasteRows.current[index];
    if (!file) return;
    if (index === pasteRows.current.length - 1) setOffer(false);
    updatePastes(pasteRows.current.filter((item) => item.id !== fileId));
    undoAttachment("File removed", () => {
      const current = pasteRows.current;
      if (current.length >= CHAT_FILE_LIMITS.count) {
        toast({ text: `A message can include ${CHAT_FILE_LIMITS.count} files.`, haptic: "warning" });
        return false;
      }
      updatePastes([...current.slice(0, index), file, ...current.slice(index)]);
      return true;
    });
  };

  const updateQuotes = (next: DraftQuote[]) => {
    setQuotes(next);
    if (next.length) keptQuotes.save(next);
    else keptQuotes.remove();
  };
  const addQuote = (sourceId: string, content: string, ask: boolean) => {
    updateQuotes([...quotes, { id: randomUUID(), sourceId, content }]);
    haptic("success");
    if (ask) afterMenu(() => editorInput.current?.focus());
  };
  const removeQuote = (quote: DraftQuote) => {
    haptic("tap");
    updateQuotes(quotes.filter((item) => item.id !== quote.id));
    setOpenQuote(undefined);
  };
  const showQuoteSource = (quote: DraftQuote) => {
    const index = messages.findIndex((message) => message.id === quote.sourceId);
    if (index < 0) return;
    setOpenQuote(undefined);
    haptic("tap");
    atBottom.current = false;
    retryQuoteScroll.current = true;
    list.current?.scrollToIndex({ index, animated: true, viewPosition: 0.35 });
  };

  const top = insets.top + BAR_HEIGHT;
  const last = messages[messages.length - 1];
  const nextModel = id === refreshed?.id && core === refreshed?.core && screenFocused && !busy && !stale
    ? nextStepModel(last)
    : undefined;
  // Not twice: an explained used-up refusal at the end already says what the notice would.
  const refused = notice?.kind === "refused" && !(last && explained(last, plan)?.usedUp) ? notice : undefined;
  // The models the free start pays for, newest of each line in the picker's order, for a refusal to name.
  const covered = useMemo(
    () => (freeStartMaxUsd === undefined ? [] : picker.current.filter((m) => m.id !== AUTO && freeStartPays(m, freeStartMaxUsd)).map((m) => m.name)),
    [picker, freeStartMaxUsd],
  );
  /** The model an answer asked for, and the models the free start pays for. */
  const freeModels = (m: Shown): FreeStartModels => {
    const asked = m.requestedModel ?? m.model;
    return { model: asked && asked !== AUTO ? nameOf(models, asked) : undefined, covered };
  };
  const freeLine = plan.kind === "free" && usage ? `Free · ${usageLine(usage)}` : undefined;
  const modelChipBody = (
    <>
      <ProviderMark model={model} size={14} />
      <Text style={[styles.chipText, { color: theme.muted }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
        {nameOf(models, model)}
      </Text>
      <Icon name="chevron" size={12} color={theme.muted} />
    </>
  );
  const teamChipBody = (named: boolean) => (
    <>
      <Icon name="members" size={14} color={theme.muted} />
      {named && teams ? (
        <Text style={[styles.chipText, styles.teamText, { color: theme.muted }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
          {teams.name}
        </Text>
      ) : null}
      <Icon name="chevron" size={12} color={theme.muted} />
    </>
  );

  const onSelection = (e: { nativeEvent: { selection: { start: number; end: number } } }) => {
    selection.current = e.nativeEvent.selection;
    setCaret(e.nativeEvent.selection.end);
  };
  const modelSheet = (
    <ModelSheet visible={!!picking} largerThan={picking === "larger" ? context?.window : undefined} models={catalog} latest={picker.current} selected={model} onPick={setModel} onClose={() => setPicking(undefined)} appliesNextTurn={!!id} freeStartMaxUsd={freeStart} />
  );
  /**
   * The primary button, as Nexus UI's .composer-send: Black on light, White on dark, faint until there is something to
   * send. While an answer runs it is Stop with nothing to send, else it queues what you wrote (P51); in the composer a
   * long press offers Queue, Send now and Stop. The icon swaps when an answer starts and ends, not when the screen opens.
   */
  const sendButton = (withMenu: boolean) => (
    <LayoutAnimationConfig skipEntering>
      {busy && !sendable ? (
        <ComposerSend
          theme={theme}
          stopping
          onPress={() => {
            answering.current?.abort();
            haptic("tap");
          }}
          label="Stop"
        >
          <Reanimated.View entering={swapIn}><Icon name="stop" size={13} color={theme.onPrimary} /></Reanimated.View>
        </ComposerSend>
      ) : (
        <ComposerSend
          theme={theme}
          ref={withMenu ? sendTarget : undefined}
          onPress={() => {
            if (busy) return queueDraft();
            if (!withMenu) setEditing(false);
            sendDraft();
          }}
          onLongPress={
            busy && withMenu
              ? () => {
                  haptic("longPress");
                  setSendMenu(true);
                }
              : undefined
          }
          disabled={!sendable}
          label={busy ? "Queue" : "Send"}
          hint={busy && withMenu ? "Long press to send now or stop" : undefined}
        >
          <Reanimated.View entering={swapIn}><Icon name="send" size={16} color={theme.onPrimary} /></Reanimated.View>
        </ComposerSend>
      )}
    </LayoutAnimationConfig>
  );

  const voice = useVoiceMode(async (words) => {
    if (!await consented()) return { problem: "Allow the model provider in order to send." };
    const reply = await send(words, [], false);
    const spoken = reply && !failed(reply) ? answerText(reply.content).trim() : "";
    return spoken ? { say: spoken, needsYou: false } : UNANSWERED;
  });
  const [resumeTyping, setResumeTyping] = useState(false);
  useEffect(() => {
    if (!resumeTyping || voice.open) return;
    setResumeTyping(false);
    editorInput.current?.focus();
  }, [resumeTyping, voice.open]);
  const canStartVoice = voiceAvailable && !busy && !dictating(dictation) && dictation.state !== "finishing";
  const startVoice = () => {
    if (!canStartVoice || answering.current || referenceRequest.current) return;
    haptic("tap");
    Keyboard.dismiss();
    voice.start();
  };

  if (voice.open) return (
    <View style={[styles.screen, { paddingTop: insets.top, backgroundColor: theme.screen }]}>
      <VoiceModeView view={voice} theme={theme} bottom={insets.bottom} onType={() => {
        voice.close();
        setResumeTyping(true);
      }} />
      <ConsentSheet
        names={question?.names ?? null}
        onAnswer={(allow) => {
          question?.answer(allow);
          setQuestion(null);
        }}
      />
    </View>
  );

  return (
    <ScreenKeyboardAvoidingView style={styles.screen}>
      {/* The content above the keyboard; the dropdowns are drawn in it, over the list and the composer. */}
      <View ref={content} style={styles.screen}>
        <FlatList
          ref={list}
          data={messages}
          keyExtractor={(m) => m.id}
          contentContainerStyle={[messages.length ? styles.feed : styles.grow, { paddingTop: top + 4, paddingBottom: dock + 8 }]}
          onLayout={(e) => setListHeight(e.nativeEvent.layout.height)}
          scrollIndicatorInsets={{ top, bottom: dock }}
          keyboardDismissMode="interactive"
          onScroll={({ nativeEvent: e }) => {
            atBottom.current = e.contentOffset.y + e.layoutMeasurement.height >= e.contentSize.height - 48;
          }}
          scrollEventThrottle={100}
          onScrollToIndexFailed={({ index, averageItemLength }) => {
            list.current?.scrollToOffset({ offset: averageItemLength * index, animated: false });
            if (!retryQuoteScroll.current) return;
            retryQuoteScroll.current = false;
            setTimeout(() => list.current?.scrollToIndex({ index, animated: true, viewPosition: 0.35 }), 220);
          }}
          // Where you stopped reading, kept unless it was the bottom (the chat then opens at its end, as before).
          onScrollEndDrag={({ nativeEvent: e }) => (atBottom.current ? place.remove() : place.save(Math.round(e.contentOffset.y)))}
          onMomentumScrollEnd={({ nativeEvent: e }) => (atBottom.current ? place.remove() : place.save(Math.round(e.contentOffset.y)))}
          onContentSizeChange={() => {
            if (!restored.current && messages.length) {
              restored.current = true;
              const y = place.load();
              if (y !== undefined) {
                atBottom.current = false;
                return list.current?.scrollToOffset({ offset: y, animated: false });
              }
            }
            const added = scrolledFor.current > 0 && messages.length > scrolledFor.current;
            scrolledFor.current = messages.length;
            if (atBottom.current) list.current?.scrollToEnd({ animated: added });
          }}
          ListEmptyComponent={id ? undefined : <Greeting theme={theme} name={name} planLine={usage?.usedUp ? undefined : freeLine} room={listHeight === undefined ? undefined : listHeight - top - 4 - dock - 8} />}
          keyboardShouldPersistTaps="handled"
          ListFooterComponent={id && last && nextModel ? (
            <ChatNextStep key={`${id}:${last.id}`} core={core} sessionId={id} consent={(signal) => consented(nextModel, signal)}
              onUse={(suggestion) => {
                changeDraft(addSuggestionToDraft(currentDraft.current, suggestion));
                AccessibilityInfo.announceForAccessibility("Suggestion added to your draft.");
              }} />
          ) : undefined}
          ListHeaderComponent={
            stale && !messages.some((m) => m.id === "error") ? (
              <Text style={[styles.staleText, { color: theme.muted }]}>{`Couldn't refresh (${stale}). This is the chat as it was.`}</Text>
            ) : undefined
          }
          renderItem={({ item }) => {
            if (item.role === "user") {
              return (
                <Question
                  content={item.content}
                  theme={theme}
                  attachments={item.attachments}
                  previews={item.previews}
                  images={images}
                  actions={questionActions(item)}
                />
              );
            }
            const refusal = explained(item, plan, freeModels(item));
            const searching = item.id === "answer" && searchingWeb;
            return (
              <View style={messageStyles.answer}>
                {refusal ? (
                  <PlanNotice title={refusal.title} body={refusal.body} open={refusal.open} ask={refusal.ask} theme={theme} />
                ) : item.content ? (
                  <AnswerProse message={item} theme={theme} />
                ) : item.stopped || searching ? null : (
                  <Thinking theme={theme} label="Thinking" />
                )}
                {searching ? <Thinking theme={theme} label="Searching the web…" /> : null}
                <AnswerFoot
                  message={item}
                  models={models}
                  theme={theme}
                  onMore={reportable(item) ? () => setAnswerMenu({ id: item.id, content: item.content, model: item.model }) : undefined}
                />
                {item.stopped ? <Text style={[messageStyles.meta, messageStyles.metaRow, { color: theme.muted }]}>Stopped</Text> : null}
                {(failed(item) || item.stopped) && item === messages[messages.length - 1] && !busy ? (
                  <View style={styles.retryRow}>
                    {/* The free start does not pay for the model asked: Auto picks one it pays for, and stays chosen. */}
                    {refusal?.cheaperModel && model !== AUTO ? (
                      <Pressable
                        accessibilityRole="button"
                        style={[styles.retry, { borderColor: theme.border }]}
                        onPress={() => {
                          setModel(AUTO);
                          void retry(AUTO);
                        }}
                        accessibilityLabel="Ask again with Auto"
                        hitSlop={4}
                      >
                        <Text style={[styles.retryText, { color: theme.foreground }]}>Ask again with Auto</Text>
                      </Pressable>
                    ) : null}
                    <Pressable accessibilityRole="button" style={[styles.retry, { borderColor: theme.border }]} onPress={() => void retry()} accessibilityLabel="Ask again" hitSlop={4}>
                      <Text style={[styles.retryText, { color: theme.foreground }]}>Retry</Text>
                    </Pressable>
                  </View>
                ) : null}
              </View>
            );
          }}
        />
        <TopBar
          // The sidebar's button where there is nothing to go back to (a chat opened from the sidebar takes the screen).
          left={
            pushed ? (
              <GlassButton label="Back" onPress={() => navigation.goBack()}>
                <Icon name="back" size={19} color={theme.foreground} />
              </GlassButton>
            ) : (
              <SidebarButton onPress={() => navigation.dispatch(DrawerActions.openDrawer())} />
            )
          }
          center={
            id ? (
              // The title, and under it what it runs on (P18: where it runs sits under the title).
              <View style={styles.heading} accessible accessibilityRole="header" accessibilityLabel={`${title}, ${nameOf(models, model)}`}>
                <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
                  {title}
                </Text>
                <Text style={[styles.subtitle, { color: theme.tertiary }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
                  {`Chat · ${nameOf(models, model)}`}
                </Text>
              </View>
            ) : (
              <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header" maxFontSizeMultiplier={CHROME_FONT_SCALE}>
                {Platform.OS === "ios" ? "New" : "New session"}
              </Text>
            )
          }
          right={id ? <ChatActions theme={theme} onShare={() => setSheet(shareActions(id))} onNew={newChat} /> : null}
          centerInset={id ? 112 : 64}
          centered={!id}
        />
        <View style={styles.dock} pointerEvents="box-none" onLayout={(e) => setDock(e.nativeEvent.layout.height)}>
          {/* What is there when the screen opens is shown, not played in. */}
          <LayoutAnimationConfig skipEntering>
            {suggestionsVisible ? <HistorySuggestions /> : null}
          </LayoutAnimationConfig>
          {refused ? (
            <View style={styles.notice}>
              <PlanNotice {...payNotice(refused, plan)} theme={theme} />
            </View>
          ) : notice?.kind === "line" && (id || messages.length) ? (
            <PlanLine label={payLineText(notice)} parts={payLineParts(notice)} act theme={theme} />
          ) : null}
          <QueueCard queue={queue} held={heldQueue} sending={sendingQueue} onSendTextOnly={sendQueuedText} onSendNow={sendQueuedNow} onRemove={(queuedId) => removeQueued([queuedId])}
            onClear={() => removeQueued(queue.map((item) => item.id))} onMove={moveQueued} onEdit={setQueueEdit} onSave={saveQueued} />
          {reference ? <ComposerReferences query={reference.query} pending={referencePending} onPick={(read) => addReference(read, reference)}
            onClose={() => setDismissedReference(draft)} /> : null}
          {slash !== null ? <SlashMenu entries={[...skills, ...COMMANDS]} query={slash} onPick={pickSlash} /> : null}
          <Composer theme={theme} halo={haloOf({ busy, chat: id, sent: messages.length })} focused={focused} box={lines > 1 || pastes.length > 0}>
            {!id ? <RunOn canRunOnComputer={!attached.images.length && !pastes.length} draft={draft} onLeaveChat={() => kept.remove()} /> : null}
            <ImageTray images={attached.images} problem={attached.problem} preparing={attached.preparing} onRemove={removeImage} theme={theme} />
            <ComposerQuotes quotes={quotes} onOpen={(quote) => { haptic("tap"); setOpenQuote(quote); }} onRemove={removeQuote} />
            {referencePending && !reference ? (
              <Text style={[styles.preparingFiles, { color: theme.muted }]} accessibilityLiveRegion="polite">Preparing files…</Text>
            ) : null}
            <PastedFiles pastes={pastes} offer={offer} onAsText={pasteAsText} onRemove={removeFile} />
            <View>
              <GestureDetector gesture={composerGesture}>
                <TextInput
                  ref={editorInput}
                  style={[styles.input, lines >= EXPAND_FROM && styles.inputBeside, { color: theme.foreground }]}
                  value={draft}
                  onFocus={() => setFocused(true)}
                  onBlur={() => setFocused(false)}
                  onSelectionChange={onSelection}
                  onContentSizeChange={(e) => setLines(linesOf(e.nativeEvent.contentSize.height - INPUT_PAD))}
                  onChangeText={changeDraft}
                  // What works: skills after a slash. @ for context comes when the phone has it.
                  placeholder="Build anything · @ context · / skills"
                  placeholderTextColor={theme.muted}
                  multiline
                  scrollEnabled
                  textAlignVertical="top"
                  accessibilityLabel="Message"
                />
              </GestureDetector>
              {/* From the third line, the editor (P65·A): 16 pt in muted, at the box's top right, in a 44 pt target. */}
              {lines >= EXPAND_FROM ? (
                <Pressable
                  style={styles.expand}
                  onPress={() => {
                    haptic("tap");
                    setEditing(true);
                  }}
                  accessibilityRole="button"
                  accessibilityLabel="Expand editor"
                >
                  <Icon name="expand" size={16} color={theme.muted} />
                </Pressable>
              ) : null}
            </View>
            <MicNote view={dictation} theme={theme} />
            <View style={styles.toolbar}>
              {dictating(dictation) ? (
                <MicMeter view={dictation} theme={theme} onPress={() => dictation.toggle(draft, selection.current)} />
              ) : (
                <View style={styles.chips} onLayout={(e) => setRoom(e.nativeEvent.layout.width)}>
                  <Pressable
                    accessibilityRole="button"
                    ref={attachChip}
                    style={[styles.attach, attached.preparing && styles.sendIdle]}
                    onPress={() => setAttaching(true)}
                    disabled={attached.preparing}
                    accessibilityLabel="Attach images"
                  >
                    <Icon name="plus" size={18} color={theme.muted} />
                  </Pressable>
                  <ComposerPicker ref={modelChip} onPress={() => setPicking("models")} label={`Model: ${nameOf(models, model)}`}>
                    {modelChipBody}
                  </ComposerPicker>
                  <ContextRing use={context} model={nameOf(models, context?.model ?? model)} theme={theme}
                    visible={showContext} onOpen={() => setShowContext(true)} onClose={() => setShowContext(false)}
                    onLarger={hasLargerModel ? () => { setShowContext(false); setPicking("larger"); } : undefined} />
                  {teams ? (
                    <ComposerPicker
                      ref={teamChip}
                      style={styles.chip}
                      onPress={() => setChoosingTeam(true)}
                      label={`Team: ${teams.name}`}
                      hint="Chooses where the spend goes"
                    >
                      {/* Beside the mode chip there is no room for its name: the icon alone, so the model keeps its name. */}
                      {teamChipBody(teamNamed)}
                    </ComposerPicker>
                  ) : null}
                  {/* The chips with every name in full, unseen, to know whether the team's name fits. */}
                  {!id && teams ? (
                    <View style={styles.measure} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
                      <View style={styles.natural} onLayout={(e) => setNatural(e.nativeEvent.layout.width)}>
                        <View style={styles.attach} />
                        <View style={styles.chip}>{modelChipBody}</View>
                        <View style={styles.chip}>{teamChipBody(true)}</View>
                      </View>
                    </View>
                  ) : null}
                </View>
              )}
              {dictating(dictation) ? null : <MicButton view={dictation} theme={theme} onPress={() => dictation.toggle(draft, selection.current)} onHold={canStartVoice ? startVoice : undefined} />}
              {sendButton(true)}
            </View>
          </Composer>
        </View>
        <RestoreSheet question={restoring} onClose={() => setRestoring(null)} onRestore={(q) => void restore(q)} />
        <ShareSheet actions={sheet} onClose={() => setSheet(null)} onError={(message) => Alert.alert("Couldn't do that", message)} />
        <AnswerSheet
          answer={answerMenu}
          onClose={() => setAnswerMenu(null)}
          onQuote={addQuote}
          onCopied={() => toast({ text: "Copied", icon: "copy" })}
          onReported={() => toast({ text: "Reported. Thanks for telling us.", icon: "flag" })}
        />
        <Sheet visible={!!openQuote} onClose={() => setOpenQuote(undefined)}>
          {openQuote ? <>
            <Text style={[styles.quoteTitle, { color: theme.foreground }]} accessibilityRole="header">Quote</Text>
            <ScrollView style={styles.quotePreview}>
              <Text selectable style={[styles.body, { color: theme.foreground }]}>{openQuote.content}</Text>
            </ScrollView>
            {messages.some((message) => message.id === openQuote.sourceId) ? (
              <SheetRow label="Show in transcript" onPress={() => showQuoteSource(openQuote)} />
            ) : null}
            <SheetRow label="Remove quote" tone="danger" onPress={() => removeQuote(openQuote)} />
            <SheetRow label="Done" onPress={() => setOpenQuote(undefined)} />
          </> : null}
        </Sheet>
        <ConsentSheet
          names={question?.names ?? null}
          onAnswer={(allow) => {
            question?.answer(allow);
            setQuestion(null);
          }}
        />
        <Dropdown
          anchor={attachChip}
          container={content}
          visible={attaching}
          sections={[{ options: [
            ...attachFrom(theme),
            { value: "files", label: "Files", leading: <Icon name="file" size={18} color={theme.foreground} /> },
            ...(canStartVoice ? [
              { value: "voice" as const, label: "Voice chat", leading: <Icon name="mic" size={18} color={theme.foreground} /> },
            ] : []),
          ] }]}
          onPick={(from) => {
            if (from === "files") addReference(pickContextFile);
            else if (from === "voice") startVoice();
            else void attached.attach(from);
          }}
          onClose={() => setAttaching(false)}
          width={220}
        />
        {editing ? null : modelSheet}
        <Dropdown
          anchor={sendTarget}
          container={content}
          visible={sendMenu}
          sections={[
            {
              options: [
                { value: "queue", label: "Queue after this step" },
                { value: "now", label: "Send now", detail: "Interrupts the answer" },
                { value: "stop", label: "Stop" },
              ],
            },
          ]}
          onPick={(choice) => (choice === "stop" ? answering.current?.abort() : queueDraft(choice === "now"))}
          onClose={() => setSendMenu(false)}
          width={260}
        />
        <ComposerEditor
          visible={editing}
          title={id ? title || "Untitled" : "New session"}
          value={draft}
          onChangeText={changeDraft}
          onSelectionChange={onSelection}
          attachedBytes={pastes.reduce((n, p) => n + p.bytes, 0) + attached.images.reduce((n, i) => n + i.bytes, 0)}
          onCollapse={() => setEditing(false)}
          bar={
            <>
              <Pressable accessibilityRole="button" style={styles.attach} onPress={() => setEditorAttach(true)} disabled={attached.preparing} accessibilityLabel="Attach images">
                <Icon name="plus" size={18} color={theme.muted} />
              </Pressable>
              <Pressable accessibilityRole="button" style={[styles.chip, styles.editorChip]} onPress={() => setPicking("models")} accessibilityLabel={`Model: ${nameOf(models, model)}`} hitSlop={8}>
                {modelChipBody}
              </Pressable>
              <View style={styles.spacer} />
              {dictating(dictation) ? (
                <MicMeter view={dictation} theme={theme} onPress={() => dictation.toggle(draft, selection.current)} />
              ) : (
                <MicButton view={dictation} theme={theme} onPress={() => dictation.toggle(draft, selection.current)} />
              )}
              {sendButton(false)}
            </>
          }
          overlay={
            <>
              {editing ? modelSheet : null}
              <Sheet visible={editorAttach} onClose={() => setEditorAttach(false)}>
                <SheetRow label="Files" onPress={() => { setEditorAttach(false); addReference(pickContextFile); }} />
                {attachFrom(theme).map((option) => (
                  <SheetRow
                    key={option.value}
                    label={option.label}
                    onPress={() => {
                      setEditorAttach(false);
                      // The picker waits for the sheet to close: iOS drops a screen presented while another is closing.
                      setTimeout(() => void attached.attach(option.value), Platform.OS === "ios" ? 350 : 0);
                    }}
                  />
                ))}
              </Sheet>
            </>
          }
        />
        {teams ? (
          <Dropdown
            anchor={teamChip}
            container={content}
            visible={choosingTeam}
            sections={[{ title: "Where the spend goes", options: teams.options }]}
            selected={teams.selected}
            onPick={setTeam}
            onClose={() => setChoosingTeam(false)}
          />
        ) : null}
      </View>
    </ScreenKeyboardAvoidingView>
  );
}

/** The chat's actions in one glass capsule, as iOS 26 groups a bar's buttons: Share, then New chat. */
function ChatActions({ theme, onShare, onNew }: { theme: Theme; onShare: () => void; onNew: () => void }) {
  const glass = useGlass();
  const action = ({ pressed }: { pressed: boolean }) => [styles.action, pressed && !glass && styles.actionPressed];
  return (
    <Glass interactive style={styles.actions}>
      <Pressable accessibilityRole="button" style={action} onPress={onShare} accessibilityLabel="Share" hitSlop={4}>
        <Icon name="share" size={18} color={theme.foreground} />
      </Pressable>
      <Pressable accessibilityRole="button" style={action} onPress={onNew} accessibilityLabel="New chat" hitSlop={4}>
        <Icon name="compose" size={18} color={theme.foreground} />
      </Pressable>
    </Glass>
  );
}

/**
 * The empty session's Nexus mark and greeting. On Free, a line
 * says so and what is left, and opens Billing.
 */
function Greeting({ theme, name, planLine, room }: { theme: Theme; name?: string; planLine?: string; room?: number }) {
  // `room` is what the list shows between the top bar and the dock. With the keyboard up it can be less than the
  // greeting's height, and the dock (suggestions and composer) would then sit over it: the greeting fades out instead.
  const [needs, setNeeds] = useState<number>();
  const fits = room === undefined || needs === undefined || needs <= room;
  const shown = useSharedValue(1);
  useEffect(() => {
    shown.value = withTiming(fits ? 1 : 0, timing(fits ? DURATION.fast : DURATION.exit));
  }, [fits, shown]);
  const fade = useAnimatedStyle(() => ({ opacity: shown.value }));
  return (
    <View style={styles.greeting}>
      <Reanimated.View
        style={[styles.greetingContent, fade]}
        onLayout={(e) => setNeeds(e.nativeEvent.layout.height)}
        pointerEvents={fits ? "box-none" : "none"}
        accessibilityElementsHidden={!fits}
        importantForAccessibility={fits ? "auto" : "no-hide-descendants"}
      >
        <NexusMark size={40} />
        {/* Push-out keeps a last word from standing alone on its line, as the Mac panel's `text-wrap: balance`. Already a
            28 pt headline, it grows only to the first accessibility size, so the whole line fits above the composer. */}
        <Text style={[styles.greetingTitle, { color: theme.foreground }]} accessibilityRole="header" lineBreakStrategyIOS="push-out" maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
          {greeting(name)}
        </Text>
        {planLine ? <PlanLine label={planLine} theme={theme} /> : null}
      </Reanimated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  grow: { flexGrow: 1 },
  feed: { paddingHorizontal: 16 },
  body: { ...text.base, fontFamily: font.regular },
  quoteTitle: { ...text.body, fontFamily: font.medium, paddingHorizontal: 20, paddingTop: 12 },
  quotePreview: { maxHeight: 200, marginHorizontal: 20, marginVertical: 12 },
  preparingFiles: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 12, paddingTop: 8 },
  retryRow: { flexDirection: "row", flexWrap: "wrap", gap: 8 },
  retry: { alignSelf: "flex-start", marginTop: 10, minHeight: 36, paddingHorizontal: 16, borderRadius: 999, borderWidth: 1, justifyContent: "center" },
  retryText: { ...text.sm, fontFamily: font.medium },
  staleText: { ...text.sm, fontFamily: font.regular, textAlign: "center", marginBottom: 12 },
  greeting: { flex: 1, justifyContent: "center", paddingHorizontal: 32 },
  greetingContent: { alignItems: "center", gap: 18 },
  greetingTitle: { ...text.xxl, fontFamily: font.display, textAlign: "center" },
  heading: { alignItems: "center" },
  title: { ...text.secondary, fontFamily: font.medium },
  subtitle: { fontFamily: font.regular, fontSize: 12, lineHeight: 15 },
  actions: { flexDirection: "row", alignItems: "center", height: 44, borderRadius: 22 },
  // 44 pt across, the group's old 4 pt padding folded in: the same 88 pt group, each a full target.
  action: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  actionPressed: { opacity: 0.5 },
  dock: { position: "absolute", left: 0, right: 0, bottom: 0 },
  suggestions: { paddingHorizontal: 16, paddingBottom: 6 },
  notice: { paddingHorizontal: 12, paddingBottom: 4 },
  suggestionsTitle: { ...text.xs, fontFamily: font.medium, paddingHorizontal: 8, marginBottom: 2 },
  suggestion: { flexDirection: "row", alignItems: "center", gap: 12, minHeight: 48, paddingHorizontal: 8, borderRadius: 14, borderCurve: "continuous" },
  suggestionMark: { width: 30, height: 30, borderRadius: 15, alignItems: "center", justifyContent: "center" },
  suggestionText: { ...text.base, fontFamily: font.regular, flex: 1 },
  // From two lines the pill is a box (P65·H).
  spacer: { flex: 1 },
  editorChip: { paddingLeft: 6 },
  // P65·H: 16/22 text; it grows a line at a time to five, then scrolls.
  input: composerControls.input,
  // Room at the right for ⤢.
  inputBeside: { paddingRight: 40 },
  expand: { position: "absolute", top: 0, right: 0, width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  toolbar: composerControls.toolbar,
  // All the room up to the send button, so its width is the room the chips have.
  chips: composerControls.choices,
  measure: { position: "absolute", left: 0, width: 2000, opacity: 0, flexDirection: "row" },
  natural: { flexDirection: "row", alignItems: "center", gap: 10 },
  chip: composerControls.picker,
  // 44 pt across for VoiceOver, TalkBack and touch alike (hitSlop widens only touch); the margin keeps the plus where it
  // was.
  attach: { width: 44, height: 44, alignItems: "center", justifyContent: "center", marginHorizontal: -6 },
  // Shrinks with its chip, so a long model name ends in "…" rather than running under the chips beside it.
  chipText: composerControls.pickerText,
  // A long team name is cut first, so the model's keeps its room.
  teamText: { maxWidth: 120 },
  // The 34 pt circle in a 44 pt target, the same footprint as the circle and its 5 pt margin.
  sendIdle: { opacity: 0.25 },
});
