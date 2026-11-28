import type { InlineImage } from "@nexus/kit/synced-timeline";
import * as Clipboard from "expo-clipboard";
import { memo, useState } from "react";
import { Image, Keyboard, Pressable, ScrollView, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { costText } from "@/code-sessions";
import { type Step, type TimelineItem, type TranscriptView, type TurnOutput, groupTranscriptSteps, turnEnding } from "@/code-timeline";
import { Icon } from "@/icon";
import { haptic } from "@/haptics";
import { Markdown } from "@/markdown";
import { pressedFill, ripple } from "@/press";
import { StatusMark } from "@/status-mark";
import { Sheet, SheetTitle } from "@/sheet";
import { useToast } from "@/toast";
import { outputFacts } from "./turn-output";
import { font, mono, text, useTheme, type Theme } from "@/theme";

/**
 * One item of a synced session's timeline, as brand v5's P57·C draws a watched session: your message in its bubble on
 * the right, each turn's line (the model and its cost, then how it ended), the agent's answer as Markdown, and a run
 * of work as one card of rows. A row with a body (a command's output, an edit's lines) opens in place.
 */
export const TimelineItemView = memo(function TimelineItemView({ item, images, model, view = "normal", outputActions = false }: {
  item: TimelineItem; images: Map<string, InlineImage>; model: (id: string) => string; view?: TranscriptView; outputActions?: boolean;
}) {
  const theme = useTheme();
  switch (item.kind) {
    case "user":
      return (
        <View style={styles.userWrap}>
          {item.images.length ? (
            <View style={styles.images}>
              {item.images.map((id) => {
                const image = images.get(id);
                return image ? (
                  <Image
                    key={id}
                    source={{ uri: `data:${image.mediaType};base64,${image.data}` }}
                    style={[styles.image, { backgroundColor: theme.fill }]}
                    accessibilityLabel={image.name ?? "Image"}
                  />
                ) : null;
              })}
            </View>
          ) : null}
          {item.text.trim() ? (
            <View style={[styles.bubble, { backgroundColor: theme.userBubble }]}>
              <Text style={[styles.bubbleText, { color: theme.foreground }]} selectable>
                {item.text}
              </Text>
            </View>
          ) : null}
        </View>
      );
    case "turn": {
      // The model, then the cost in mono, then how the turn ended: "Sonnet 5.5 · $0.18 · Done in 41s".
      const parts = [
        item.model ? { text: model(item.model) } : null,
        item.status === "running" ? { text: "Working" } : null,
        item.costUsd ? { text: costText(String(item.costUsd)) ?? "", money: true } : null,
        { text: turnEnding(item) ?? "" },
      ].filter((part): part is { text: string; money?: boolean } => !!part?.text);
      return (
        <View style={styles.turn} accessible accessibilityLabel={parts.map((part) => part.text).join(", ")}>
          {item.status === "running" ? <StatusMark status="running" size={10} /> : null}
          <Text style={[styles.turnText, { color: theme.tertiary }]} numberOfLines={1}>
            {parts.map((part, i) => (
              <Text key={i}>
                {i ? " · " : ""}
                <Text style={part.money ? styles.money : undefined}>{part.text}</Text>
              </Text>
            ))}
          </Text>
        </View>
      );
    }
    case "text":
      return <Markdown source={item.text} />;
    case "output":
      return <TurnOutputCard output={item} actions={outputActions} />;
    case "work":
      return (
        <View style={[styles.card, { backgroundColor: theme.card }]}>
          {groupTranscriptSteps(item.steps, view).map((group) => group.kind === "routine" && group.steps.length > 1
            ? <RoutineSteps key={`${group.key}:${view}`} steps={group.steps} view={view} />
            : <StepRow key={`${group.key}:${view}`} step={group.kind === "step" ? group.step : group.steps[0]!} view={view} />)}
        </View>
      );
    case "question":
    case "approval":
    case "notice":
      return <Text style={[styles.note, { color: theme.muted }]}>{item.text}</Text>;
    case "error":
      return (
        <View style={styles.errorRow}>
          <Icon name="close" size={13} color={theme.danger} />
          <Text style={[styles.note, styles.errorText, { color: theme.danger }]} selectable>
            {item.detail ? `${item.text}\n${item.detail}` : item.text}
          </Text>
        </View>
      );
  }
});

function TurnOutputCard({ output, actions }: { output: TurnOutput; actions: boolean }) {
  const theme = useTheme();
  const toast = useToast();
  const { height } = useWindowDimensions();
  const [view, setView] = useState<"edits" | "tests">();
  const facts = outputFacts(output.edits.length, output.tests);
  const status = turnEnding(output);
  const title = view === "edits" ? "Recorded edits" : "Test command output";
  const open = (next: "edits" | "tests") => {
    Keyboard.dismiss();
    haptic("tap");
    setView(next);
  };
  const copy = async () => {
    haptic("tap");
    try {
      const summary = ["This turn", status, ...facts.map(({ label }) => label)].filter(Boolean).join("\n");
      if (await Clipboard.setStringAsync(summary)) toast({ text: "Turn summary copied.", icon: "check", haptic: "success" });
    } catch {
      toast({ text: "Couldn’t copy the summary.", haptic: "error" });
    }
  };
  return <View style={[styles.card, { backgroundColor: theme.card }]}>
    <View style={styles.outputTitle}>
      <Icon name={output.status === "completed" ? "check" : "close"} size={16} color={output.status === "error" ? theme.danger : theme.muted} />
      <Text style={[styles.outputHeading, { color: theme.foreground }]}>This turn</Text>
      {status ? <Text style={[styles.outputStatus, { color: theme.muted }]}>{status}</Text> : null}
    </View>
    {facts.map(({ kind, label }) => <View key={kind} style={styles.outputFact}>
      <Icon name={kind === "edits" ? "file" : "command"} size={16} color={theme.tertiary} />
      <Text style={[styles.outputLabel, { color: theme.foreground }]}>{label}</Text>
    </View>)}
    {actions ? <>
      <View style={[styles.outputActions, { borderTopColor: theme.separator }]}>
        {output.edits.length ? <OutputAction label="Review edits" onPress={() => open("edits")} /> : null}
        {output.tests.length ? <OutputAction label="View test output" onPress={() => open("tests")} /> : null}
        <OutputAction label="Copy summary" onPress={() => void copy()} />
      </View>
      <Sheet visible={!!view} onClose={() => setView(undefined)}>
        <SheetTitle>{title}</SheetTitle>
        <ScrollView style={{ maxHeight: height * 0.65 }} keyboardShouldPersistTaps="handled">
          {(view === "edits" ? output.edits : view === "tests" ? output.tests : []).map((step) =>
            <StepRow key={step.key} step={step} view="verbose" />)}
        </ScrollView>
      </Sheet>
    </> : null}
  </View>;
}

function OutputAction({ label, onPress }: { label: string; onPress: () => void }) {
  const theme = useTheme();
  return <Pressable accessibilityRole="button" onPress={onPress} style={({ pressed }) =>
    [styles.outputAction, pressed && pressedFill(theme)]}>
    <Text style={[styles.outputLabel, { color: theme.primary }]}>{label}</Text>
  </Pressable>;
}

function RoutineSteps({ steps, view }: { steps: Step[]; view: TranscriptView }) {
  const theme = useTheme();
  const [open, setOpen] = useState(false);
  return <>
    <Pressable onPress={() => { haptic("selection"); setOpen((was) => !was); }} android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.stepRow, pressed && pressedFill(theme)]}
      accessibilityRole="button" accessibilityState={{ expanded: open }} accessibilityHint={open ? "Hides steps" : "Shows steps"}>
      <View style={styles.stepHead}>
        <StepMark state="done" theme={theme} />
        <Text style={[styles.step, { color: theme.muted }]} numberOfLines={1}>{`${steps.length} completed steps`}</Text>
        <Icon name={open ? "chevron" : "forward"} size={11} color={theme.tertiary} />
      </View>
    </Pressable>
    {open ? steps.map((step) => <StepRow key={step.key} step={step} view={view} />) : null}
  </>;
}

/** One step: its mark, the verb and what it acts on, the short fact on the right; a tap opens its body. */
function StepRow({ step, view }: { step: Step; view: TranscriptView }) {
  const theme = useTheme();
  const [open, setOpen] = useState(view === "verbose" || (view === "thinking" && step.kind === "thinking"));
  const active = step.state === "running" || step.state === "asking";
  const color = active ? theme.foreground : theme.muted;
  const extra = step.state === "asking" ? "Needs you" : step.extra;
  const head = (
    <View style={styles.stepHead}>
      <StepMark state={step.state} theme={theme} />
      <Text style={[styles.step, { color }]} numberOfLines={1}>
        {step.verb}
        {step.detail ? <Text style={step.code ? styles.stepCode : undefined}>{` ${step.detail}`}</Text> : null}
      </Text>
      {extra ? <Text style={[styles.extra, { color: theme.tertiary }]}>{extra}</Text> : null}
      {step.body ? <Icon name={open ? "chevron" : "forward"} size={11} color={theme.tertiary} /> : null}
    </View>
  );
  const spoken = [step.verb, step.detail, extra].filter(Boolean).join(", ");
  if (!step.body) {
    return (
      <View style={styles.stepRow} accessible accessibilityLabel={spoken}>
        {head}
      </View>
    );
  }
  return (
    <View>
      <Pressable
        onPress={() => { haptic("selection"); setOpen((was) => !was); }}
        android_ripple={ripple(theme)}
        style={({ pressed }) => [styles.stepRow, pressed && pressedFill(theme)]}
        accessibilityRole="button"
        accessibilityLabel={spoken}
        accessibilityState={{ expanded: open }}
      >
        {head}
      </Pressable>
      {open ? <StepBody body={step.body} theme={theme} preview={view === "verbose"} /> : null}
    </View>
  );
}

/** Done: a check. Running: the turning ring. Waiting on you: a ring. Failed: a cross. Never ran: a faint ring. */
function StepMark({ state, theme }: { state: Step["state"]; theme: Theme }) {
  return (
    <View style={styles.mark} accessibilityElementsHidden importantForAccessibility="no-hide-descendants" pointerEvents="none">
      {state === "running" ? (
        <StatusMark status="running" size={10} />
      ) : state === "done" ? (
        <Icon name="check" size={11} color={theme.tertiary} />
      ) : state === "failed" ? (
        <Icon name="close" size={11} color={theme.danger} />
      ) : (
        <View style={[styles.ring, { borderColor: state === "asking" ? theme.foreground : theme.tertiary }]} />
      )}
    </View>
  );
}

function StepBody({ body, theme, preview }: { body: NonNullable<Step["body"]>; theme: Theme; preview: boolean }) {
  const [all, setAll] = useState(false);
  const lines = body.kind === "output" ? body.text.split("\n") : [];
  const collapsed = preview && !all && lines.length > 4;
  if (body.kind === "text") return <Text style={[styles.thought, { color: theme.muted }]}>{body.text}</Text>;
  return (
    // Code keeps its lines: it scrolls sideways in its own box, never the page.
    <>
    <ScrollView horizontal style={[styles.code, { backgroundColor: theme.grouped }]} contentContainerStyle={styles.codeInner} showsHorizontalScrollIndicator={false}>
      {body.kind === "output" ? (
        <Text style={[styles.codeText, { color: body.error ? theme.danger : theme.muted }]} selectable>
          {collapsed ? lines.slice(0, 4).join("\n") : body.text}
        </Text>
      ) : (
        <View>
          {body.rows.map((row, i) => (
            <Text key={i} style={[styles.codeText, { color: row.sign === "+" ? theme.foreground : theme.tertiary, backgroundColor: row.sign === "+" ? theme.fill : undefined }]}>
              {`${row.sign === "-" ? "−" : row.sign} ${row.text}`}
            </Text>
          ))}
        </View>
      )}
    </ScrollView>
    {body.kind === "output" && body.truncated && !collapsed ?
      <Text style={[styles.expand, styles.note, { color: theme.muted }]}>Tool output was clipped.</Text> : null}
    {collapsed ? <Pressable onPress={() => { haptic("tap"); setAll(true); }} accessibilityRole="button"
      style={({ pressed }) => [styles.expand, pressed && pressedFill(theme)]}>
      <Text style={[styles.note, { color: theme.foreground }]}>{body.kind === "output" && !body.truncated ? `Show all ${lines.length} lines` : "Show available output"}</Text>
    </Pressable> : null}
    </>
  );
}

const styles = StyleSheet.create({
  userWrap: { alignItems: "flex-end", gap: 6 },
  images: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: 6 },
  image: { width: 96, height: 96, borderRadius: 12 },
  // P57·C: 16 on 22, 10 by 14, the 18 pt corner.
  bubble: { maxWidth: "85%", paddingHorizontal: 14, paddingVertical: 10, borderRadius: 18, borderCurve: "continuous" },
  bubbleText: { ...text.base, fontFamily: font.regular, lineHeight: 22 },
  turn: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 18 },
  turnText: { ...text.caption, fontFamily: font.regular, flexShrink: 1 },
  money: { fontFamily: mono, fontVariant: ["tabular-nums"] },
  card: { borderRadius: 16, borderCurve: "continuous", paddingVertical: 4, overflow: "hidden" },
  outputTitle: { flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 14, paddingTop: 12, paddingBottom: 8 },
  outputHeading: { ...text.body, fontFamily: font.medium, flex: 1 },
  outputStatus: { ...text.caption, flexShrink: 1 },
  outputFact: { flexDirection: "row", gap: 10, alignItems: "center", minHeight: 34, paddingHorizontal: 14 },
  outputLabel: { ...text.caption, flexShrink: 1 },
  outputActions: { borderTopWidth: StyleSheet.hairlineWidth, marginTop: 8, flexDirection: "row", flexWrap: "wrap", paddingHorizontal: 6 },
  outputAction: { minHeight: 44, minWidth: 44, justifyContent: "center", paddingHorizontal: 8, borderRadius: 8 },
  stepRow: { minHeight: 44, justifyContent: "center", paddingHorizontal: 14 },
  stepHead: { flexDirection: "row", alignItems: "center", gap: 10 },
  // StatusMark's own box: every mark of a card sits in one column.
  mark: { width: 18, alignItems: "center", justifyContent: "center" },
  ring: { width: 9, height: 9, borderRadius: 5, borderWidth: 1.5 },
  step: { ...text.secondary, fontFamily: font.regular, flex: 1 },
  stepCode: { fontFamily: mono, fontSize: 13 },
  extra: { ...text.caption, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
  thought: { ...text.caption, fontFamily: font.italic, paddingHorizontal: 14, paddingBottom: 12, paddingLeft: 36 },
  code: { marginHorizontal: 10, marginBottom: 10, borderRadius: 10, borderCurve: "continuous" },
  codeInner: { padding: 10 },
  codeText: { fontFamily: mono, fontSize: 12, lineHeight: 18 },
  expand: { minHeight: 44, paddingHorizontal: 14, justifyContent: "center" },
  note: { ...text.secondary, fontFamily: font.regular },
  errorRow: { flexDirection: "row", gap: 8, alignItems: "flex-start" },
  errorText: { flex: 1, marginTop: -3 },
});
