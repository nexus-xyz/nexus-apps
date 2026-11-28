import type { CodeAction, SessionEvent } from "@nexus/kit";
import type { InlineImage } from "@nexus/kit/synced-timeline";
import { useEffect, useMemo, useRef, useState } from "react";
import { FlatList, KeyboardAvoidingView, Platform, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import type { ControlState, Delivery } from "./code-control";
import { CodeControlForm } from "./code-controls";
import { foldTimeline, type TimelineItem } from "./code-timeline";
import { TimelineItemView } from "./code-timeline-view";
import { font, text, useTheme } from "./theme";

const PREVIEW_COMPUTER = "Studio Mac";

type Scenario ="Approval" | "Edit" | "Plan" | "Question" | "Running" | "Offline";
const scenarios: Scenario[] = ["Approval", "Edit", "Plan", "Question", "Running", "Offline"];

const PLAN: SessionEvent = { kind: "tool", id: "preview-todos", name: "TodoWrite", input: { todos: [
  { content: "Find the failing case", activeForm: "Finding the failing case", status: "completed" },
  { content: "Fix the decoder", activeForm: "Fixing the decoder", status: "completed" },
  { content: "Run the decoder tests", activeForm: "Running the decoder tests", status: "in_progress" },
  { content: "Update the changelog", activeForm: "Updating the changelog", status: "pending" },
  { content: "Open a pull request", activeForm: "Opening a pull request", status: "pending" },
] } };

function sample(scenario: Scenario): SessionEvent[] {
  const start: SessionEvent[] = [{ kind: "user", text: "Fix the failing decoder test." }, { kind: "turnStart" }];
  if (scenario === "Edit") return [...start,
    { kind: "text", text: "The bound check is off by one. Here is the fix." },
    { kind: "tool", id: "preview-edit", name: "Edit", input: { file_path: "src/codec/decoder.ts", old_string: "  if (offset > buffer.length) {\n    throw new RangeError(\"offset\");\n  }", new_string: "  if (offset >= buffer.length) {\n    throw new RangeError(`offset ${offset} is past the end`);\n  }" } },
    { kind: "approval", requestId: "preview-edit-approval", toolUseId: "preview-edit", tool: "Edit", input: { file_path: "src/codec/decoder.ts", old_string: "  if (offset > buffer.length) {\n    throw new RangeError(\"offset\");\n  }", new_string: "  if (offset >= buffer.length) {\n    throw new RangeError(`offset ${offset} is past the end`);\n  }" } },
  ];
  if (scenario === "Plan") return [...start,
    { kind: "text", text: "I read the decoder and its tests. Here is what I would do." },
    { kind: "approval", requestId: "preview-plan", tool: "ExitPlanMode", input: { plan: "## Fix the decoder's bound check\n\n1. Change `>` to `>=` in `decoder.ts`.\n2. Name the offset in the error.\n3. Add a test for a read at the end of the buffer.\n4. Run the decoder tests." } },
  ];
  if (scenario === "Question") return [...start,
    { kind: "text", text: "I found the failing case. Which test coverage would you like?" },
    { kind: "approval", requestId: "preview-question", tool: "AskUserQuestion", input: { questions: [{ question: "Which tests should I run?", options: [{ label: "Decoder tests", description: "Run the focused suite." }, { label: "All tests", description: "Run the full project suite." }], multiSelect: false }] } },
  ];
  if (scenario === "Approval") return [...start, PLAN,
    { kind: "text", text: "The decoder change is ready. I need your approval to run its tests." },
    { kind: "tool", id: "preview-tool", name: "Bash", input: { command: "npm test -- decoder" } },
    { kind: "approval", requestId: "preview-approval", toolUseId: "preview-tool", tool: "Bash", input: { command: "npm test -- decoder" }, description: "Run the decoder tests in this project." },
  ];
  return [...start, PLAN, { kind: "text", text: "Reading the decoder and checking the failing case…" }, { kind: "tool", id: "preview-read", name: "Read", input: { file_path: "src/decoder.ts" } }];
}

/** A scenario's name from a link (`nexus://dev/code-control?scenario=Edit`), or Approval. */
export function previewScenario(name: string | undefined): Scenario {
  return scenarios.find((scenario) => scenario.toLowerCase() === name?.toLowerCase()) ?? "Approval";
}

/**
 * A development-only, local UI demonstration. No sign-in, network, file writes or real agent actions. `initial` opens
 * on that scenario, so a link can show each one.
 */
export function CodeControlPreview({ initial = "Approval" }: { initial?: Scenario }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const [scenario, setScenario] = useState<Scenario>(initial);
  const [events, setEvents] = useState<SessionEvent[]>(() => sample(initial));
  const [running, setRunning] = useState(true);
  const [delivery, setDelivery] = useState<Delivery>({ kind: "idle" });
  const [stop, setStop] = useState<Delivery>({ kind: "idle" });
  const [viewKey, setViewKey] = useState(0);
  // The preview's turn started 83 seconds ago, so "Working 1m 23s" shows at once.
  const [startedAt, setStartedAt] = useState(() => Date.now() - 83_000);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);
  const sequence = useRef(0);
  const list = useRef<FlatList<TimelineItem>>(null);
  const items = useMemo(() => foldTimeline(events, running), [events, running]);
  const images = useMemo(() => new Map<string, InlineImage>(), []);

  const clear = () => { timers.current.forEach(clearTimeout); timers.current = []; };
  useEffect(() => () => clear(), []);
  const choose = (next: Scenario) => {
    clear();
    setScenario(next);
    setEvents(sample(next));
    setRunning(true);
    setDelivery({ kind: "idle" });
    setStop({ kind: "idle" });
    setViewKey((key) => key + 1);
    setStartedAt(Date.now() - 83_000);
  };
  const submit = async (action: CodeAction, options: { waits?: boolean } = {}) => {
    const id = `preview-${++sequence.current}`;
    const set = action.kind === "stop" ? setStop : setDelivery;
    // A message for the sleeping computer waits until the person cancels it.
    if (options.waits) return set({ kind: "queued", id, action, waitsFor: PREVIEW_COMPUTER });
    set({ kind: "queued", id, action });
    timers.current.push(setTimeout(() => set({ kind: "claimed", id, action }), 1200));
    timers.current.push(setTimeout(() => {
      set({ kind: "completed", id, action });
      setRunning(false);
      const fresh: SessionEvent[] = action.kind === "send"
        ? [{ kind: "user", text: action.text }, { kind: "turnStart" }, { kind: "text", text: "Your message appeared in the session. In a connected session, the agent would continue from here." }]
        : action.kind === "stop"
          ? [{ kind: "notice", text: "The preview run stopped." }]
          : [{ kind: "approvalResolved", requestId: action.requestId, decision: action.decision, message: action.message }, { kind: "text", text: action.decision === "deny" ? "Your refusal reached the session." : "Your answer reached the session. This demonstration does not run commands." }];
      setEvents((held) => [...held, ...fresh, { kind: "turnEnd", status: action.kind === "stop" ? "interrupted" : "completed" }]);
    }, 3500));
  };
  const control: ControlState = scenario === "Offline" ? { kind: "offline", waits: { computer: PREVIEW_COMPUTER } } : { kind: "ready", control: { runnerDeviceId: "preview-computer", enabled: true, expiresAt: Date.now() + 60_000 } };
  const waiting = delivery.kind === "queued" && !!delivery.waitsFor;
  const cancel = async () => { if (delivery.kind === "queued") setDelivery({ ...delivery, kind: "cancelled" }); };

  return (
    <KeyboardAvoidingView style={[styles.screen, { backgroundColor: theme.screen, paddingTop: insets.top }]} behavior={Platform.OS === "ios" ? "padding" : undefined}>
      <View style={styles.header}>
        <Text style={[styles.title, { color: theme.foreground }]}>Phone control</Text>
        <Text style={[styles.note, { color: theme.muted }]}>Interactive demo · no real computer is connected</Text>
        <Text style={[styles.note, { color: theme.tertiary }]}>Try an approval, answer a question, send a message, or stop a run.</Text>
        <View style={styles.scenarios}>
          {scenarios.map((item) => <Pressable key={item} onPress={() => choose(item)} accessibilityRole="button" accessibilityState={{ selected: scenario === item }} style={[styles.choice, { backgroundColor: scenario === item ? theme.primary : theme.fill }]}>
            <Text style={[styles.choiceText, { color: scenario === item ? theme.onPrimary : theme.foreground }]}>{item}</Text>
          </Pressable>)}
        </View>
      </View>
      <FlatList ref={list} style={styles.list} contentContainerStyle={styles.timeline} data={items} keyExtractor={(item) => item.key}
        renderItem={({ item }) => <TimelineItemView item={item} images={images} model={(id) => id} />}
        onContentSizeChange={() => list.current?.scrollToEnd({ animated: true })} keyboardShouldPersistTaps="handled" />
      <CodeControlForm key={viewKey} events={events} running={running} unlocked bottom={insets.bottom} startedAt={startedAt} control={{ control, delivery, stop, viewKey, canStopChecking: () => false, stopChecking: () => {}, submit, retry: async () => {}, canCancel: (slot) => slot === "delivery" && waiting, cancel, recheck: async () => {}, attachWaits: () => {} }} />
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  header: { paddingHorizontal: 20, paddingTop: 16, paddingBottom: 12, gap: 8 },
  title: { ...text.body, fontFamily: font.medium, fontSize: 24, lineHeight: 30 },
  note: { ...text.caption, fontFamily: font.regular },
  scenarios: { flexDirection: "row", flexWrap: "wrap", gap: 8, marginTop: 6 },
  choice: { minHeight: 44, paddingHorizontal: 14, justifyContent: "center", borderRadius: 22 },
  choiceText: { ...text.sm, fontFamily: font.medium },
  list: { flex: 1 },
  timeline: { padding: 16, gap: 14 },
});
