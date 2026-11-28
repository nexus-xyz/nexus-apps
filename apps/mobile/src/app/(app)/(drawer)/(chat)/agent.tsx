import { AGENT_MAX_CHARS, firstNameOf, type AgentClient, type AgentConversationSummary, type AgentIntro, type AgentMessage, type AgentProblem, type AgentTurn } from "@nexus/kit";
import { router, useLocalSearchParams, useNavigation } from "expo-router";
import { DrawerActions } from "expo-router/react-navigation";
import { Fragment, useEffect, useRef, useState, type ComponentRef } from "react";
import { ActivityIndicator, AccessibilityInfo, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import Reanimated, { LayoutAnimationConfig } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { CUT, entriesOf, phoneProblem, UNREACHABLE, type Entry } from "@/agent-chat";
import { offersHistory } from "@/agent-conversations";
import { errandsOn } from "@/agent-errands";
import { AgentFirstOpen, Tile } from "@/agent-first-open";
import type { DecideWrite, EditWrite } from "@/approval-card";
import { agreeTo, providerName, providersFor } from "@/ai-consent";
import { markSeen, setStreaming } from "@/assistant-activity";
import { DayMarker, NexusDraft, NexusGreeting, NexusTurn, OfflineLine, PersonBubble, ProfileHeader } from "@/assistant-bubbles";
import { handedWords } from "@/assistant-handoff";
import { menuRows, type MenuAction } from "@/assistant-menu";
import { AssistantMenu } from "@/assistant-menu-view";
import type { Seen } from "@/assistant-row";
import { chime } from "@/assistant-sounds";
import { layoutThread, timesOf } from "@/assistant-texting";
import { greetingOf, nextQueued, QUEUED, receiptAfter, receiptAfterFailure, receiptLine, SENDING, type Receipt } from "@/assistant-thread";
import { useAuth, useCore, useStored } from "@/auth";
import { haloOf } from "@/composer-halo";
import { ComposerSend, composerControls } from "@/composer-controls";
import { LINE, linesOf, MAX_LINES } from "@/composer-text";
import { ConsentSheet, keptConsent } from "@/consent-sheet";
import { TextInput } from "@/controls";
import { Composer } from "@/conversation-composer";
import { ConversationsSheet, useHistory } from "@/conversations-sheet";
import { ScreenKeyboardAvoidingView } from "@/screen-keyboard";
import { errandCard, lastSuggestions } from "@/assistant-cards";
import { ErrandsStrip } from "@/errands-strip";
import { GlassButton } from "@/glass";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Blank } from "@/list";
import { AUTO } from "@/meta";
import { nameOf, useModels } from "@/models";
import { BriefTurn, useTodayBrief } from "@/brief-bubble";
import { PausedBar } from "@/paused-bar";
import { PressScale } from "@/press-scale";
import { CHROME_FONT_SCALE } from "@/text-size";
import { font, text, useTheme, type Theme } from "@/theme";
import { swapIn } from "@/thinking";
import { useErrandStates } from "@/thread-cards";
import { BAR_HEIGHT, SidebarButton, TopBar } from "@/top-bar";
import { useReachable } from "@/use-reachable";
import { spokenReply, UNANSWERED } from "@/voice";
import { useVoiceMode, voiceAvailable, VoiceModeView } from "@/voice-mode";
import { useAssistantName } from "@/use-assistant-name";
import { withExisting } from "@/share-inbox";
import { useSharedHandoff } from "@/use-share-inbox";

/** The input's padding, top and bottom: one 22 pt line then fills the 44 pt row beside Send. */
const INPUT_PAD = 22;
/** Within this of the end, the feed counts as read to the bottom and follows a new answer. */
const NEAR_END = 48;
/** The dock's height before it is measured: the composer's padding (6 + 4 + 6), its 44 pt row, and the line under it. */
const ONE_LINE_DOCK = 6 + 4 + 44 + 6 + 6 + 17;
/** Errands and Projects in the menu, in a build with errands. */
const ERRANDS = errandsOn(process.env.EXPO_PUBLIC_NEXUS_AGENT_ERRANDS);

/**
 * The Assistant (Agent Interfaces S2.5, A21): a chat with Nexus, the person's own agent,
 * with the same notes, saved model and opt-ins, as Nexus UI's (S2.4) and desktop's (S2.3). It asks the agent API with this sign-in's
 * token (`auth.tsx`). The answer streams as Markdown, with its sources, model and cost under it.
 *
 * It runs no write itself. A write the agent asks for is the approval card (A9, `approval-card.tsx`): the person
 * approves or declines it in the thread, and the agent API runs it. An answer that needed an account the person has
 * not connected ends with a Connect card (A6·e, `connect-card.tsx`). The conversation's id is kept on the device, so the screen reads its turns back while the agent still holds them (an
 * hour). A build without the agent API's URL says the agent is not available here.
 *
 * When the agent API keeps shared conversations, History lists the person's conversations from every surface, Slack
 * included (`conversations-sheet.tsx`). Opening one reads its turns, and the next ask continues it. Each ask names
 * the phone as its surface. A 404 hides History, and the screen works as above.
 *
 * The thread follows the Assistant canvas (`product/nexus/design/spec-product/assistant_canon.py`, ASSISTANT.md
 * section 2): it is a session transcript, and texting is how Nexus behaves, not how the thread looks. The bar names
 * Nexus over "Acme · Assistant", with one control on the right: the menu (`assistant-menu.ts`), which holds History,
 * New conversation, the Assistant's pages and its settings. The profile header opens the thread and stays as the start
 * of its history. Nexus writes plain, full-width text; your turns are the right-side bubble; a day marker starts each
 * day (`assistant-texting.ts`). There is no avatar and no Delivered or Read. A message that was not sent says so, with
 * Retry. A send that fails at the network marks the screen offline: it says so, keeps what you send, and sends it when
 * the agent API answers again (`assistant-thread.ts`, `use-reachable.ts`). Search hands words over in `ask`, and the
 * screen sends them once. The composer reads "Ask Nexus anything". A light haptic and a quiet sound play on send and
 * on receive, muted in Account › Assistant (`assistant-sounds.ts`). Send shows once there is text.
 *
 * With no conversation kept, the empty chat is the first open (A19), when the agent API has it: the same line and
 * starters as on the web and desktop, shown once per person. A starter sends its line. Without it, Nexus still writes
 * first (`greetingOf`), with See what I can do under it.
 *
 * It is a page of the drawer, beside Chat, not a sheet, and works as Chat does: the sidebar button and a
 * glass bar over the feed, the floating glass composer and its halo, Send and Stop in one circle, haptics on send and
 * on the answer, and a feed that follows the answer only while you read at its end.
 */
export default function YourAgent() {
  const { agent, org } = useAuth();
  const { ask } = useLocalSearchParams<{ ask?: string }>();
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  if (!agent) {
    return (
      <View style={[styles.screen, { paddingTop: insets.top + BAR_HEIGHT }]}>
        <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service. Use @nexus in Slack, or the Assistant in Nexus UI." />
        <AgentBar theme={theme} />
      </View>
    );
  }
  return <AgentChat key={org} agent={agent} handed={handedWords(ask)} />;
}

/** How a message came: typed in the box, said in voice mode, handed over from Search, a first open's starter, or a quick reply. */
type Way = "typed" | "spoken" | "handed" | "starter" | "reply";

/**
 * The Assistant's bar, as the canvas's phone header draws it: the sidebar button, then the Assistant's name (the
 * person's choice or Nexus) over "Acme · Assistant", and `right`, the menu's one control. The profile header
 * in the thread carries the mark, so the bar does not.
 */
function AgentBar({ theme, right }: { theme: Theme; right?: React.ReactNode }) {
  const navigation = useNavigation();
  const { orgName } = useAuth();
  const assistant = useAssistantName();
  return (
    <TopBar
      left={<SidebarButton onPress={() => navigation.dispatch(DrawerActions.openDrawer())} />}
      center={
        <View style={styles.heading} accessible accessibilityRole="header" accessibilityLabel={orgName ? `${assistant}, ${orgName}` : assistant}>
          <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {assistant}
          </Text>
          <Text style={[styles.where, { color: theme.tertiary }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {orgName ? `${orgName} · Assistant` : "Assistant"}
          </Text>
        </View>
      }
      right={right}
      centerInset={64}
    />
  );
}

function AgentChat({ agent, handed }: { agent: AgentClient; handed?: string }) {
  const core = useCore();
  const { name: person, org, orgName } = useAuth();
  const assistant = useAssistantName();
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  // The bar and the dock float over the feed, which scrolls under both, as in Chat.
  const top = insets.top + BAR_HEIGHT;
  const [dock, setDock] = useState(() => ONE_LINE_DOCK + Math.max(insets.bottom, 12));
  const models = useModels(core);
  const kept = useStored<string>("cache", "agent-conversation");
  // What the screen showed, for the drawer's unread count (`assistant-row.ts`).
  const seen = useStored<Seen>("cache", "assistant-seen");
  const [conversation, setConversation] = useState<string | undefined>(() => kept.load());
  const [entries, setEntries] = useState<Entry[]>([]);
  const [loading, setLoading] = useState(conversation !== undefined);
  const [draftText, setDraftText] = useState("");
  // Words shared from another app go after what is typed, which stays; unlike Search's hand-off, they wait
  // for Send.
  useSharedHandoff("assistant", true, (shared) => {
    setDraftText((typed) => withExisting(typed, shared.text).slice(0, AGENT_MAX_CHARS));
    AccessibilityInfo.announceForAccessibility("What you shared is in your message. Nothing is sent until you tap Send.");
  });
  // The answer being written: its words, its model, and the step that runs now (A1·C b).
  const [draft, setDraft] = useState<{ text: string; model?: string; step?: string } | undefined>();
  const [problem, setProblem] = useState<AgentProblem | undefined>();
  const [status, setStatus] = useState<string | undefined>();
  const [question, setQuestion] = useState<{ names: string[]; answer: (allow: boolean) => void } | null>(null);
  const [focused, setFocused] = useState(false);
  const [lines, setLines] = useState(1);
  // Where each of your messages is, by its entry's id, and whether the last send failed at the network.
  const [receipts, setReceipts] = useState<Record<string, Receipt>>({});
  const [offline, setOffline] = useState(false);
  const setReceipt = (id: string, receipt: Receipt) => setReceipts((now) => ({ ...now, [id]: receipt }));
  // When each message was sent, for the thread's groups and centred times (`assistant-texting.ts`), and the messages
  // that arrived on this screen, which move in; those read back do not.
  const [times, setTimes] = useState<Record<string, number>>({});
  const stamp = (id: string) => setTimes((now) => ({ ...now, [id]: Date.now() }));
  const fresh = useRef(new Set<string>());
  // The bar's menu opens below its ⋯ and is drawn in the page (`assistant-menu-view.tsx`).
  const [menu, setMenu] = useState(false);
  const menuAnchor = useRef<ComponentRef<typeof View>>(null);
  const page = useRef<ComponentRef<typeof View>>(null);
  const answering = useRef<AbortController | null>(null);
  // The read of a conversation opened from History.
  const opening = useRef<AbortController | null>(null);
  const history = useHistory(agent);
  const [picking, setPicking] = useState(false);
  const alive = useRef(true);
  const pendingConsent = useRef<((allow: boolean) => void) | undefined>(undefined);
  const scroll = useRef<ComponentRef<typeof ScrollView>>(null);
  // Whether the reader is at the end of the feed: only then does a new answer pull the feed down.
  const atEnd = useRef(true);
  const counter = useRef(0);
  const nextId = () => `e${(counter.current += 1)}`;
  const busy = draft !== undefined;
  const name = (model: string) => nameOf(models, model);
  // An approval card's Approve and Decline (A9): the agent API, with the person's own sign-in.
  const decide: DecideWrite = (card, decision, options) => agent.decideWrite(card, decision, options);
  const edit: EditWrite = (card, changes) => agent.editWrite(card, changes);
  const empty = !loading && !entries.length && !busy;
  // The first open (A19): read once with no conversation kept, and gone once the person asks anything.
  const [intro, setIntro] = useState<AgentIntro | undefined>();
  const [introDone, setIntroDone] = useState(false);
  const firstOpen = empty && intro && !introDone ? intro : undefined;
  // Today's brief (A1): read again when the phone is back online; not while a conversation from History is open.
  const [readOther, setReadOther] = useState(false);
  const [backOnline, setBackOnline] = useState(0);
  const brief = useTodayBrief(agent, !readOther, backOnline);
  // Quick replies go under the thread's last answer only, while nothing is sent after it (A1·D).
  const lastEntry = entries.at(-1);
  const replies = lastEntry?.role === "agent" && !lastEntry.readBack && !busy && !loading ? lastSuggestions(lastEntry.messages) : [];
  // The errand cards show each errand as it is now: the list is read while the thread has one (A1·5).
  const hasErrandCard = entries.some((entry) => entry.role === "agent" && entry.messages.some((message) => errandCard(message) !== undefined));
  const errandStates = useErrandStates(agent, hasErrandCard, entries.length);
  const errandNow = (id: string) => errandStates.get(id);

  // The conversation kept from last time, read back while the agent still holds it.
  useEffect(() => {
    const id = kept.load();
    if (!id) return setLoading(false);
    const controller = new AbortController();
    agent.conversation(id, controller.signal).then(
      (read) => {
        if (controller.signal.aborted) return;
        if (read.kind === "turns") showTurns(read.turns);
        else if (read.kind === "gone") {
          kept.remove();
          setConversation(undefined);
          setStatus("That conversation has ended. Start a new one.");
        } else setProblem(phoneProblem(read.failure));
        setLoading(false);
      },
      () => {
        // Aborted, or the network: the screen still takes a new question.
        if (!controller.signal.aborted) setLoading(false);
      },
    );
    return () => controller.abort();
  }, [agent, kept]);

  // The first open, for a chat that keeps no conversation. Once it shows, the agent API keeps it seen on every app.
  useEffect(() => {
    if (kept.load()) return;
    const controller = new AbortController();
    agent.intro(controller.signal).then(
      (read) => {
        if (controller.signal.aborted || read.kind !== "intro" || !read.intro.starters.length) return;
        setIntro(read.intro);
        agent.introSeen().catch(() => undefined);
      },
      () => undefined,
    );
    return () => controller.abort();
  }, [agent, kept]);

  // Leaving the screen stops an answer still streaming.
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      answering.current?.abort();
      opening.current?.abort();
      pendingConsent.current?.(false);
    };
  }, []);

  /** Turns read back: their entries and their times. The agent had each of them, so none says Not sent. */
  const showTurns = (turns: AgentTurn[]) => {
    setEntries(entriesOf(turns));
    setTimes(timesOf(turns));
    setReceipts({});
  };

  // On screen, Nexus's replies are seen: the drawer's count starts again from here.
  const answers = entries.filter((entry) => entry.role === "agent").length;
  useEffect(() => {
    if (loading || !conversation) return;
    seen.save({ conversation, answers });
    markSeen(org);
  }, [loading, conversation, answers, seen, org]);

  const keep = (id: string) => {
    if (!id) return;
    setConversation(id);
    kept.save(id);
  };

  /** Nexus's answer goes to the provider of your saved model, so it asks first, as a chat does. */
  const consented = () => {
    const providers = providersFor(AUTO, models);
    return agreeTo(providers, keptConsent, async () => {
      const listed = providers.length ? await core.listProviders().then((r) => r.data, () => []) : [];
      if (!alive.current) return false;
      return new Promise<boolean>((answer) => {
        pendingConsent.current = answer;
        setQuestion({ names: providers.map((id) => providerName(id, listed)), answer });
      });
    });
  };

  /**
   * Asks the box's text, the words said in voice mode, the words handed over from Search, or a first open's starter,
   * and gives the reply's messages. Only the box's own text clears the box. Offline, the message waits under the
   * thread and goes out when the connection is back.
   */
  const send = async (words?: string, way: Way = "typed"): Promise<AgentMessage[] | undefined> => {
    const message = (words ?? draftText).trim();
    if (!message || busy || loading || answering.current) return;
    const controller = new AbortController();
    answering.current = controller;
    let allowed = false;
    try {
      allowed = await consented();
    } catch {
      allowed = false;
    } finally {
      if (answering.current === controller) answering.current = null;
    }
    if (!allowed || controller.signal.aborted || !alive.current) return;
    haptic("send");
    // Voice mode speaks and listens: a sound there would talk over it.
    if (way !== "spoken") chime("send");
    const id = nextId();
    setIntroDone(true);
    atEnd.current = true;
    fresh.current.add(id);
    stamp(id);
    setEntries((now) => [...now, { id, role: "user", text: message }]);
    if (way === "typed") setDraftText("");
    if (offline) {
      setReceipt(id, QUEUED);
      return;
    }
    return deliver(id, message, way);
  };

  /** Sends one of your messages, already in the thread, and follows its receipt. */
  const deliver = async (id: string, message: string, way: Way = "typed"): Promise<AgentMessage[] | undefined> => {
    if (answering.current) return;
    const controller = new AbortController();
    answering.current = controller;
    let receipt: Receipt = SENDING;
    // Any event of the ask means the agent has the message: a failure after it says what happened, not Not sent.
    const mark = () => {
      receipt = receiptAfter(receipt);
      setReceipt(id, receipt);
    };
    // Nexus's message arrives once: with its first words while it streams, or whole when the reply lands.
    let arrived = false;
    const arrive = () => {
      if (arrived) return;
      arrived = true;
      haptic("receive");
      if (way !== "spoken") chime("receive");
    };
    setReceipt(id, SENDING);
    setProblem(undefined);
    setStatus(undefined);
    setDraft({ text: "" });
    setStreaming(org, true);
    try {
      const asked = await agent.ask(message, {
        conversation,
        surface: "mobile",
        signal: controller.signal,
        onEvent: (event) => {
          if (controller.signal.aborted || !alive.current) return;
          if (event.type === "started") {
            keep(event.conversation);
            mark();
          } else if (event.type === "model") {
            mark();
            setDraft((now) => ({ ...now, text: now?.text ?? "", model: event.model }));
          } else if (event.type === "step") {
            // The tool that runs now, as one line (A1·C b): it shows until the words start.
            setDraft((now) => ({ ...now, text: now?.text ?? "", step: event.line }));
          } else if (event.type === "text") {
            mark();
            arrive();
            setDraft((now) => ({ ...now, text: (now?.text ?? "") + event.delta }));
          }
        },
      });
      if (!alive.current) return;
      if (controller.signal.aborted) {
        setReceipt(id, receiptAfterFailure(receipt, false));
        setStatus("Stopped.");
        return;
      }
      if (asked.kind === "reply") {
        mark();
        setOffline(false);
        keep(asked.reply.conversation);
        const messages: AgentMessage[] = asked.reply.messages;
        if (messages.length) {
          const reply = nextId();
          // A reply that streamed is already on screen in its bubble; one that came whole pops in now.
          if (!arrived) fresh.current.add(reply);
          stamp(reply);
          setEntries((now) => [...now, { id: reply, role: "agent", messages }]);
        }
        arrive();
        // Voice mode says the answer, so the screen reader does not.
        if (way !== "spoken") AccessibilityInfo.announceForAccessibility(`${assistant} answered.`);
        return messages;
      }
      haptic("error");
      setReceipt(id, receiptAfterFailure(receipt, false));
      setProblem(asked.kind === "cut" ? CUT : phoneProblem(asked.failure));
    } catch {
      if (!alive.current) return;
      if (controller.signal.aborted) {
        setReceipt(id, receiptAfterFailure(receipt, false));
        setStatus("Stopped.");
      } else {
        // The network failed: a message the agent never had waits for the connection; one it had says what happened.
        haptic("error");
        setReceipt(id, receiptAfterFailure(receipt, true));
        if (receipt.kind !== "sending") setProblem(UNREACHABLE);
        setOffline(true);
      }
    } finally {
      if (answering.current === controller) answering.current = null;
      if (alive.current) setDraft(undefined);
      setStreaming(org, false);
    }
  };

  /** Retry under a message that was not sent: it goes again, in its place. */
  const retry = (id: string) => {
    const entry = entries.find((one) => one.id === id);
    if (entry?.role !== "user" || busy || loading || answering.current) return;
    atEnd.current = true;
    void deliver(id, entry.text);
  };

  // Back online: what waited goes out, one at a time, in the thread's order.
  useReachable(offline, (signal) => agent.conversations({ limit: 1, signal }), () => {
    setOffline(false);
    setBackOnline((count) => count + 1);
  });
  const queued = offline || busy || loading ? undefined : nextQueued(entries.map((entry) => entry.id), receipts);
  useEffect(() => {
    if (queued) retry(queued);
    // `retry` is this render's; the queued id is what changes.
  }, [queued]);

  // Words handed over from Search, sent once the kept conversation is on screen and no answer is on its way. The ref
  // holds the words last sent. It clears when the parameter clears, so the next hand-off to this mounted screen goes out.
  const handedOver = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (!handed) {
      handedOver.current = undefined;
      return;
    }
    if (loading || busy) return;
    router.setParams({ ask: undefined });
    if (handedOver.current === handed) return;
    handedOver.current = handed;
    void send(handed, "handed");
  }, [handed, loading, busy]);

  /** Opens a conversation from History: its turns on screen, and the next ask continues it. */
  const open = (row: AgentConversationSummary) => {
    setPicking(false);
    if (busy || loading || answering.current || row.id === conversation) return;
    opening.current?.abort();
    const controller = new AbortController();
    opening.current = controller;
    keep(row.id);
    setEntries([]);
    setTimes({});
    setReceipts({});
    setProblem(undefined);
    setStatus(undefined);
    setLoading(true);
    setReadOther(true);
    atEnd.current = true;
    agent.conversation(row.id, controller.signal).then(
      (read) => {
        if (controller.signal.aborted || !alive.current) return;
        if (read.kind === "turns") showTurns(read.turns);
        else if (read.kind === "gone") {
          kept.remove();
          setConversation(undefined);
          setStatus("That conversation has ended. Start a new one.");
          history.refresh();
        } else setProblem(phoneProblem(read.failure));
        setLoading(false);
      },
      () => {
        if (controller.signal.aborted || !alive.current) return;
        setProblem(UNREACHABLE);
        setLoading(false);
      },
    );
  };

  const startOver = () => {
    if (busy || loading || answering.current) return;
    haptic("tap");
    kept.remove();
    setConversation(undefined);
    setEntries([]);
    setTimes({});
    setReceipts({});
    setProblem(undefined);
    setStatus("New conversation.");
    setReadOther(false);
  };

  // Voice mode (A18), behind EXPO_PUBLIC_NEXUS_AGENT_VOICE: the words said go as a normal ask, and Nexus says the reply.
  const voice = useVoiceMode(async (words) => {
    const messages = await send(words, "spoken");
    return messages ? spokenReply(messages) : UNANSWERED;
  });
  // Type leaves voice mode with the composer focused.
  const [typing, setTyping] = useState(false);

  if (voice.open) {
    return (
      <View style={[styles.screen, { paddingTop: top }]}>
        <VoiceModeView
          view={voice}
          theme={theme}
          bottom={insets.bottom}
          onType={() => {
            voice.close();
            setTyping(true);
          }}
        />
        <ConsentSheet
          names={question?.names ?? null}
          onAnswer={(allow) => {
            question?.answer(allow);
            setQuestion(null);
          }}
        />
        <AgentBar theme={theme} />
      </View>
    );
  }

  // History and New stay in the menu while an answer runs, dimmed, so the thread never changes under the finger.
  const still = busy || loading;
  const rows = menuRows({ history: offersHistory(history.history), conversation: entries.length > 0, errands: ERRANDS });
  const act = (action: MenuAction) => {
    if (action === "new") return startOver();
    history.refresh();
    setPicking(true);
  };
  const sendable = draftText.trim().length > 0 && !loading;
  // The thread as a session transcript: a day marker on each new day, and each side's messages grouped.
  const places = layoutThread(
    entries.map((entry) => ({ id: entry.id, side: entry.role === "user" ? ("person" as const) : ("nexus" as const), at: times[entry.id] })),
    Date.now(),
  );

  return (
    // The page starts at the window's top, as Chat does, so the view lifts by exactly what the keyboard covers.
    <ScreenKeyboardAvoidingView style={styles.screen}>
      {/* The page above the keyboard. The padding the keyboard adds shrinks this view, which carries the dock up: a dock
          placed absolutely in the KeyboardAvoidingView itself would keep to its bottom edge, under the keyboard. */}
      <View ref={page} style={styles.screen} collapsable={false}>
        <ScrollView
          ref={scroll}
          style={styles.screen}
          contentContainerStyle={[styles.feed, entries.length || busy ? null : styles.grow, { paddingTop: top + 4, paddingBottom: dock + 16 }]}
          scrollIndicatorInsets={{ top, bottom: dock }}
          keyboardDismissMode="interactive"
          keyboardShouldPersistTaps="handled"
          scrollEventThrottle={100}
          onScroll={({ nativeEvent: e }) => {
            atEnd.current = e.contentOffset.y + e.layoutMeasurement.height >= e.contentSize.height - NEAR_END;
          }}
          // Streaming text grows the feed many times a second: it follows without animating, and only at the end.
          onContentSizeChange={() => {
            if (atEnd.current) scroll.current?.scrollToEnd({ animated: false });
          }}
          accessibilityLabel="Conversation"
        >
          {/* The profile header: the start of the thread's history, always first. */}
          <ProfileHeader org={orgName} theme={theme} />
          {/* Errands on top (A12), behind EXPO_PUBLIC_NEXUS_AGENT_ERRANDS: read again as each answer lands. */}
          <ErrandsStrip agent={agent} revision={entries.length} />
          {/* Today's brief (A1·A): the thread's first message, unless another conversation is open. */}
          {brief ? <BriefTurn brief={brief} agent={agent} theme={theme} /> : null}
          {loading ? <ActivityIndicator style={styles.loading} color={theme.muted} accessibilityLabel="Loading the conversation" /> : null}
          {firstOpen ? (
            <AgentFirstOpen
              intro={firstOpen}
              firstName={firstNameOf(person)}
              {...(orgName ? { org: { name: orgName, personal: orgName === "Personal" } } : {})}
              theme={theme}
              disabled={busy || loading}
              onStart={(line) => void send(line, "starter")}
              onAccounts={() => router.push("/integrations")}
            />
          ) : empty && !brief ? (
            // Nexus writes first, so the thread is never blank, and Discover is one tap away (A15·f).
            <NexusGreeting words={greetingOf(firstNameOf(person), assistant)} theme={theme}>
              <View style={styles.tiles}>
                <Tile icon="plan" label="See what I can do" theme={theme} onPress={() => router.push("/discover")} />
              </View>
            </NexusGreeting>
          ) : null}
          {entries.map((entry, i) => {
            const place = places[i];
            const day = place.day ? <DayMarker label={place.day} theme={theme} /> : null;
            const isFresh = fresh.current.has(entry.id);
            return entry.role === "user" ? (
              <Fragment key={entry.id}>
                {day}
                <PersonBubble
                  text={entry.text}
                  line={receiptLine(receipts[entry.id])}
                  onRetry={receipts[entry.id]?.kind === "failed" && !busy && !loading ? () => retry(entry.id) : undefined}
                  theme={theme}
                  last={place.last}
                  fresh={isFresh}
                />
              </Fragment>
            ) : (
              <Fragment key={entry.id}>
                {day}
                <NexusTurn
                  messages={entry.messages}
                  name={name}
                  line={!entry.readBack}
                  theme={theme}
                  decide={decide}
                  edit={edit}
                  fresh={isFresh}
                  errandNow={errandNow}
                  replies={entry === lastEntry ? replies : undefined}
                  onReply={(reply) => void send(reply, "reply")}
                />
              </Fragment>
            );
          })}
          {draft ? <NexusDraft draft={draft} name={name} theme={theme} /> : null}
          {offline ? <OfflineLine theme={theme} /> : null}
          {/* Paused (A11·b): read again as each answer lands; nothing while it runs. */}
          <PausedBar agent={agent} revision={entries.length} theme={theme} />
          {problem ? (
            <View style={[styles.problem, { backgroundColor: theme.card, borderColor: theme.border }]} accessibilityRole="alert">
              <Text style={[styles.problemTitle, { color: theme.danger }]}>{problem.title}</Text>
              <Text style={[styles.quiet, { color: theme.muted }]}>{problem.detail}</Text>
              {problem.signIn ? <Pill label="Open Account" theme={theme} onPress={() => router.push("/settings")} /> : null}
            </View>
          ) : null}
          {status ? <Text style={[styles.status, { color: theme.tertiary }]}>{status}</Text> : null}
        </ScrollView>
        <AgentBar
          theme={theme}
          right={
            <View ref={menuAnchor} collapsable={false}>
              <GlassButton label={`${assistant} menu`} hint="History, a new conversation and settings" onPress={() => setMenu((open) => !open)}>
                <Icon name="more" size={18} color={theme.foreground} />
              </GlassButton>
            </View>
          }
        />
        <View style={styles.dock} pointerEvents="box-none" onLayout={(e) => setDock(e.nativeEvent.layout.height)}>
          <Composer
            theme={theme}
            halo={haloOf({ busy, chat: conversation ?? null, sent: entries.length })}
            focused={focused}
            box={lines > 1}
          >
            <View style={styles.row}>
              <TextInput
                value={draftText}
                onChangeText={setDraftText}
                onFocus={() => setFocused(true)}
                onBlur={() => setFocused(false)}
                onContentSizeChange={(e) => setLines(linesOf(e.nativeEvent.contentSize.height - INPUT_PAD))}
                placeholder={`Ask ${assistant} anything`}
                placeholderTextColor={theme.muted}
                multiline
                maxLength={AGENT_MAX_CHARS}
                editable={!busy}
                autoFocus={typing}
                style={[styles.input, { color: theme.foreground }]}
                accessibilityLabel={`Message ${assistant}`}
              />
              {/* As in Messages: voice while the box is empty, and Send, an up arrow in a filled circle, once there is text. */}
              {voiceAvailable && !busy && !loading && !sendable ? (
                <Pressable style={composerControls.target} onPress={voice.start} accessibilityRole="button" accessibilityLabel={`Talk to ${assistant}`}>
                  <Icon name="voice" size={18} color={theme.foreground} />
                </Pressable>
              ) : null}
              {/* Send and Stop share one circle; the icon swaps when an answer starts and ends, not when the screen opens. */}
              <LayoutAnimationConfig skipEntering>
                {!busy && !sendable ? null : busy ? (
                  <ComposerSend
                    theme={theme}
                    stopping
                    onPress={() => {
                      haptic("tap");
                      answering.current?.abort();
                    }}
                  >
                    <Reanimated.View entering={swapIn}>
                      <Icon name="stop" size={13} color={theme.onPrimary} />
                    </Reanimated.View>
                  </ComposerSend>
                ) : (
                  <Reanimated.View entering={swapIn}>
                    <ComposerSend theme={theme} onPress={() => void send()}>
                      <Icon name="send" size={16} color={theme.onPrimary} />
                    </ComposerSend>
                  </Reanimated.View>
                )}
              </LayoutAnimationConfig>
            </View>
          </Composer>
        </View>
        {/* Last, so it draws over the bar and the dock. */}
        <AssistantMenu anchor={menuAnchor} container={page} visible={menu} rows={rows} busy={still} onClose={() => setMenu(false)} onAction={act} />
      </View>
      <ConversationsSheet
        visible={picking}
        onClose={() => setPicking(false)}
        history={history.history}
        reading={history.reading}
        current={conversation}
        onOpen={open}
        onMore={history.more}
      />
      <ConsentSheet
        names={question?.names ?? null}
        onAnswer={(allow) => {
          question?.answer(allow);
          setQuestion(null);
        }}
      />
    </ScreenKeyboardAvoidingView>
  );
}

/** The problem's one action, as Chat's Retry: an outlined pill that gives a little under the finger. */
function Pill({ label, theme, onPress }: { label: string; theme: Theme; onPress: () => void }) {
  return (
    <PressScale onPress={onPress} accessibilityRole="button" hostStyle={styles.pillHost} style={[styles.pill, { borderColor: theme.border }]}>
      <Text style={[styles.pillText, { color: theme.foreground }]}>{label}</Text>
    </PressScale>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  // The canvas's phone header: the name at 15 pt medium over its place at 11.5 pt.
  heading: { alignItems: "center", gap: 1 },
  title: { fontFamily: font.medium, fontSize: 15, lineHeight: 19 },
  where: { fontFamily: font.regular, fontSize: 11.5, lineHeight: 14 },
  dock: { position: "absolute", left: 0, right: 0, bottom: 0 },
  feed: { paddingHorizontal: 16 },
  grow: { flexGrow: 1 },
  loading: { marginTop: 24 },
  quiet: { ...text.sm, fontFamily: font.regular },
  problem: { gap: 4, marginBottom: 20, padding: 14, borderRadius: 16, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth },
  problemTitle: { ...text.base, fontFamily: font.medium },
  status: { ...text.xs, fontFamily: font.regular, textAlign: "center", marginBottom: 12 },
  tiles: { marginBottom: 20 },
  row: { flexDirection: "row", alignItems: "flex-end" },
  input: { ...composerControls.input, flex: 1, paddingTop: INPUT_PAD / 2, paddingBottom: INPUT_PAD / 2, maxHeight: LINE * MAX_LINES + INPUT_PAD },
  pillHost: { alignSelf: "flex-start", marginTop: 8 },
  pill: { minHeight: 36, justifyContent: "center", paddingHorizontal: 16, borderRadius: 999, borderWidth: 1 },
  pillText: { ...text.sm, fontFamily: font.medium },
});
