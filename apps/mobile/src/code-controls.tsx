import type { CodeAction, CoreClient, SessionEvent } from "@nexus/kit";
import { pendingCodeApprovals } from "@nexus/kit/remote-control";
import { useEffect, useMemo, useRef, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { approvalPreview, type PreviewBody } from "./approval-preview";
import { useAuth } from "./auth";
import { deliveryPending, deliveryText, slotOf, type ControlState, type Delivery, type Slot } from "./code-control";
import type { CodeDraftStore } from "./code-draft";
import { answerReceipt, answerValues, codeAnswers, codeQuestions, type QuestionChoice } from "./code-questions";
import type { ReaderState } from "./code-sessions";
import { Composer } from "./conversation-composer";
import { ComposerSend, composerControls, INPUT_PAD } from "./composer-controls";
import { linesOf } from "./composer-text";
import { TextInput } from "./controls";
import { haptic } from "./haptics";
import { Icon } from "./icon";
import { Markdown } from "./markdown";
import { SessionMark } from "./nav-row";
import { activityText, planProgress } from "./session-activity";
import { font, mono, text, useTheme, type Theme } from "./theme";
import { useCodeControl } from "./use-code-control";
import { useCodeDraft, useCodeStores } from "./use-code-draft";
import { useNow } from "./use-now";

type Approval = Extract<SessionEvent, { kind: "approval" }>;
interface Props {
  core: CoreClient;
  threadId: string;
  reader: Extract<ReaderState, { kind: "ready" }>["reader"];
  events: readonly SessionEvent[];
  running: boolean;
  /** This phone holds the session's current key. */
  unlocked: boolean;
  /** This phone is still checking for that key: the status says so instead of "needs the key". */
  checkingKey?: boolean;
  bottom: number;
  settings?: React.ReactNode;
  /** When the current turn started (ms since 1970), for "Working 1m 20s"; null or absent says "Working". */
  startedAt?: number | null;
}

/**
 * Commands leave this phone encrypted. A queued receipt keeps the draft until the computer confirms the action. The
 * draft is also saved encrypted per session, so it survives navigation and an app restart.
 */
export function CodeControls(props: Props) {
  const control = useCodeControl(props.core, props.threadId, props.reader);
  const { gateway } = useAuth();
  const stores = useCodeStores(props.core, gateway, props.threadId, props.reader);
  const { attachWaits } = control;
  // A message that waits for a sleeping computer comes back after an app restart, tracked by its id.
  useEffect(() => attachWaits(stores.wait), [attachWaits, stores.wait]);
  return <CodeControlForm key={control.viewKey} {...props} control={control} draftStore={stores.draft} />;
}

type Control = ReturnType<typeof useCodeControl>;

export function CodeControlForm({ events, running, unlocked, checkingKey = false, settings, startedAt = null, control, draftStore }: Pick<Props, "events" | "running" | "unlocked" | "checkingKey" | "bottom" | "settings" | "startedAt"> & { control: Control; draftStore?: CodeDraftStore }) {
  const theme = useTheme();
  const delivered = control.delivery.kind === "completed" && control.delivery.action.kind === "send" ? control.delivery.action.text : undefined;
  const [draft, setDraft] = useCodeDraft(draftStore, unsentText(control.delivery), delivered);
  const [focused, setFocused] = useState(false);
  const [lines, setLines] = useState(1);
  const [answered, setAnswered] = useState<ReadonlySet<string>>(new Set());
  const completed = useRef<string | undefined>(undefined);
  const stopped = useRef<string | undefined>(undefined);
  const pending = deliveryPending(control.delivery);
  const stopPending = deliveryPending(control.stop);
  const ready = unlocked && control.control.kind === "ready";
  const approvals = useMemo(() => pendingCodeApprovals(events), [events]);
  const open = approvals.filter((approval) => !answered.has(approval.requestId));
  const plan = useMemo(() => planProgress(events), [events]);
  const now = useNow(running && !open.length);
  const activity = activityText({ running, waiting: open.length > 0, since: startedAt, now });
  const receipt = deliveryText(control.delivery);

  useEffect(() => {
    const delivery = control.delivery;
    if (delivery.kind === "failed" || delivery.kind === "rejected" || delivery.kind === "expired") {
      haptic("error");
      return;
    }
    if (delivery.kind !== "completed" || completed.current === delivery.id) return;
    completed.current = delivery.id;
    haptic("success");
    if (delivery.action.kind === "send") {
      const sent = delivery.action.text;
      setDraft((current) => current.trim() === sent ? "" : current);
    } else if (delivery.action.kind === "approve") {
      const requestId = delivery.action.requestId;
      setAnswered((current) => new Set([...current, requestId]));
    }
  }, [control.delivery, setDraft]);

  useEffect(() => {
    const stop = control.stop;
    if (stop.kind === "failed" || stop.kind === "rejected" || stop.kind === "expired") {
      haptic("error");
      return;
    }
    if (stop.kind !== "completed" || stopped.current === stop.id) return;
    stopped.current = stop.id;
    haptic("success");
  }, [control.stop]);

  // The computer sleeps while Remote control is on. A message can wait for it; Stop and answers cannot.
  const asleep = unlocked && control.control.kind === "offline" ? control.control.waits?.computer : undefined;
  // Stop has its own slot: it stays sendable while a message or an answer waits for the computer.
  const submit = (action: CodeAction) => {
    const waits = !ready && !!asleep && action.kind === "send";
    if (!(ready || waits) || (slotOf(action) === "stop" ? stopPending : pending)) return;
    haptic("send");
    void control.submit(action, waits ? { waits } : undefined);
  };
  const problem = failedDelivery(control.delivery);
  // A message that waits, or one just cancelled, says so even while the computer sleeps.
  const ownReceipt = "waitsFor" in control.delivery && !!control.delivery.waitsFor;
  const status = !unlocked ? checkingKey ? "Opening the session’s key…" : "This phone needs the session’s current key." : control.control.kind !== "ready" && !ownReceipt ? controlText(control.control) : control.delivery.kind === "completed" ? null : receipt;
  const stopStatus = ready && control.stop.kind !== "completed" ? deliveryText(control.stop) : null;
  const stopping = running && !draft.trim() && !asleep;
  // One waiting message at a time: while one waits, its receipt and Cancel show, not a second "Send when … is back".
  const sendLater = !ready && !!asleep && !stopping && !ownReceipt;

  return (
    <View style={styles.dock}>
      {unlocked && open.length ? (
        <ScrollView style={styles.approvals} keyboardShouldPersistTaps="handled" contentContainerStyle={styles.approvalList}>
          {open.map((approval) => (
            <ApprovalCard key={approval.requestId} approval={approval} disabled={!ready || pending} onSubmit={submit} />
          ))}
        </ScrollView>
      ) : null}
      {activity || plan ? (
        <View style={styles.activity} testID="code-activity">
          {activity ? (
            <View style={styles.activityWords} accessible accessibilityLabel={plan?.current ? `${activity}, ${plan.current}` : activity} accessibilityLiveRegion="polite">
              <SessionMark state={open.length ? "needsYou" : "working"} />
              <Text style={[styles.activityText, { color: theme.muted }]} numberOfLines={1}>
                {activity}
                {plan?.current && !open.length ? <Text style={{ color: theme.tertiary }}>{` · ${plan.current}`}</Text> : null}
              </Text>
            </View>
          ) : <View style={styles.activityWords} />}
          {plan ? (
            <View style={[styles.planChip, { backgroundColor: theme.fill }]} accessible accessibilityLabel={`Plan, ${plan.done} of ${plan.total} done`}>
              <Icon name="audit" size={12} color={theme.muted} />
              <Text style={[styles.planText, { color: theme.foreground }]}>{`${plan.done} of ${plan.total} done`}</Text>
            </View>
          ) : null}
        </View>
      ) : null}
      {stopStatus ? <View style={styles.status}>
        <Text numberOfLines={2} style={[styles.note, { color: failedDelivery(control.stop) ? theme.danger : theme.muted }]} accessibilityLiveRegion="polite" testID="code-control-stop-receipt">
          {stopStatus}
        </Text>
      </View> : null}
      {status ? <View style={styles.status}>
        <Text numberOfLines={2} style={[styles.note, { color: problem ? theme.danger : theme.muted }]} accessibilityLiveRegion="polite" testID="code-control-receipt">
          {status}
        </Text>
      </View> : null}
      {(["stop", "delivery"] as const).map((slot) => (
        <SlotActions key={slot} slot={slot} delivery={control[slot]} canStopChecking={control.canStopChecking(slot)} canCancel={control.canCancel(slot)}
          onRetry={() => { haptic("tap"); void control.retry(slot); }} onStopChecking={() => { haptic("tap"); control.stopChecking(slot); }}
          onCancel={() => { haptic("tap"); void control.cancel(slot); }} onRecheck={() => { haptic("tap"); void control.recheck(slot); }} />
      ))}
      <Composer theme={theme} halo={pending || stopPending || running ? "working" : "waiting"} focused={focused} box={lines > 1}>
        <TextInput value={draft} onChangeText={setDraft} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
          onContentSizeChange={(event) => setLines(linesOf(event.nativeEvent.contentSize.height - INPUT_PAD))}
          editable={unlocked} placeholder="Message" placeholderTextColor={theme.muted} multiline scrollEnabled textAlignVertical="top" maxLength={16_000}
          style={[composerControls.input, { color: theme.foreground }]} accessibilityLabel="Message this session" testID="code-control-draft" />
        <View style={styles.toolbar}>
          <View style={styles.settings}>{settings}</View>
          {running && draft.trim() ? <Pressable onPress={() => submit({ kind: "stop" })} disabled={!ready || stopPending} style={[composerControls.target, (!ready || stopPending) && styles.dim]} accessibilityRole="button" accessibilityLabel="Stop this session" accessibilityState={{ disabled: !ready || stopPending }} testID="code-control-stop"><View style={[composerControls.send, { borderColor: theme.border, borderWidth: StyleSheet.hairlineWidth }]}><Icon name="stop" size={13} color={theme.foreground} /></View></Pressable> : null}
          {sendLater ? (
            <Pressable onPress={() => submit({ kind: "send", text: draft.trim() })} disabled={pending || !draft.trim()}
              style={[styles.later, { backgroundColor: theme.primary }, (pending || !draft.trim()) && styles.dim]}
              accessibilityRole="button" accessibilityState={{ disabled: pending || !draft.trim() }}
              accessibilityHint="Runs once when the computer is back, within 24 hours" testID="code-control-send-later">
              <Text numberOfLines={1} style={[styles.buttonText, { color: theme.onPrimary }]}>{`Send when ${asleep} is back`}</Text>
            </Pressable>
          ) : (
            <ComposerSend theme={theme} onPress={() => submit(stopping ? { kind: "stop" } : { kind: "send", text: draft.trim() })}
              disabled={!ready || (stopping ? stopPending : pending || !draft.trim())} loading={stopping ? stopPending : pending} stopping={stopping}
              label={stopping ? "Stop this session" : "Send to computer"} testID={stopping ? "code-control-stop" : "code-control-send"} />
          )}
        </View>
      </Composer>
    </View>
  );
}

function controlText(control: ControlState): string {
  switch (control.kind) {
    case "loading": return "Checking the computer…";
    case "ready": return "";
    case "disabled": return "Turn on Remote control on the computer running this session.";
    case "offline": return control.waits
      ? `${control.waits.computer} is away. Your message can wait until it's back.`
      : "Waiting for the computer. Reconnects automatically when Nexus is available.";
    case "reconnecting": return "Reconnecting to Nexus…";
    case "unsupported": return "Update Gateway and Nexus desktop to control Code sessions from this phone.";
    case "locked": return "This phone needs the session's current key before it can send commands.";
    case "error": return control.message;
    default: {
      const exhaustive: never = control;
      return exhaustive;
    }
  }
}

/** The text of a message still on its way: the composer keeps it until the computer confirms. */
function unsentText(delivery: Delivery): string {
  return delivery.kind !== "completed" && "action" in delivery && delivery.action.kind === "send" ? delivery.action.text : "";
}

function failedDelivery(delivery: Delivery): boolean {
  return delivery.kind === "failed" || delivery.kind === "rejected" || delivery.kind === "expired";
}

/**
 * One slot's recovery buttons: retry an unconfirmed command, stop checking one past its deadline, or cancel a message
 * that waits for a sleeping computer. After a cancel Gateway did not confirm, Check again only reads the
 * receipt: it never sends the message.
 */
function SlotActions({ slot, delivery, canStopChecking, canCancel, onRetry, onStopChecking, onCancel, onRecheck }: { slot: Slot; delivery: Delivery; canStopChecking: boolean; canCancel: boolean; onRetry: () => void; onStopChecking: () => void; onCancel: () => void; onRecheck: () => void }) {
  const theme = useTheme();
  const what = slot === "stop" ? "Stop" : "delivery";
  return (
    <>
      {canCancel ? (
        <Pressable onPress={onCancel} style={styles.retry} accessibilityRole="button" accessibilityHint="The message will not run. Your draft stays here." testID="code-control-cancel">
          <Text style={[styles.buttonText, { color: theme.foreground }]}>Cancel</Text>
        </Pressable>
      ) : null}
      {delivery.kind === "recheck" ? (
        <Pressable onPress={onRecheck} style={styles.retry} accessibilityRole="button" accessibilityHint="Checks whether the cancel went through. Sends nothing." testID="code-control-recheck">
          <Text style={[styles.buttonText, { color: theme.foreground }]}>Check again</Text>
        </Pressable>
      ) : null}
      {delivery.kind === "retry" ? (
        <Pressable onPress={onRetry} style={styles.retry} accessibilityRole="button">
          <Text style={[styles.buttonText, { color: theme.foreground }]}>{slot === "stop" ? "Retry Stop" : "Retry delivery"}</Text>
        </Pressable>
      ) : null}
      {canStopChecking ? (
        <Pressable onPress={onStopChecking} style={styles.retry} accessibilityRole="button" accessibilityHint="The action may already have run. Check the session before sending it again.">
          <Text style={[styles.buttonText, { color: theme.foreground }]}>{`Stop checking ${what}`}</Text>
        </Pressable>
      ) : null}
    </>
  );
}

function ApprovalCard({ approval, disabled, onSubmit }: { approval: Approval; disabled: boolean; onSubmit: (action: CodeAction) => void }) {
  const theme = useTheme();
  const questions = useMemo(() => codeQuestions(approval.input), [approval.input]);
  const [choices, setChoices] = useState<Record<string, QuestionChoice>>({});
  // A tool denial can say what to do instead; the runner passes it to the agent with the denial.
  const [note, setNote] = useState<string | null>(null);
  const question = approval.tool === "AskUserQuestion";
  const complete = questions.length > 0 && questions.every((item) => answerValues(item, choices[item.key]).length > 0);
  const instead = note?.trim() || null;
  const answer = (decision: "allow" | "allowSession" | "deny") => onSubmit({
    kind: "approve", requestId: approval.requestId, decision,
    message: question ? decision === "allow" ? answerReceipt(questions, choices) : "The person skipped the question." : decision === "deny" ? instead : null,
    answers: question && decision === "allow" ? codeAnswers(questions, choices) : null,
  });
  const preview = useMemo(() => (question ? null : approvalPreview(approval.tool, approval.input)), [question, approval.tool, approval.input]);
  const heading = question ? "The agent has a question" : preview?.verb ?? approval.tool;
  return (
    <View style={[styles.approval, { backgroundColor: theme.card, borderColor: theme.border }]} testID={`code-approval-${approval.requestId}`}>
      <View style={styles.approvalHead} accessible accessibilityRole="header" accessibilityLabel={[heading, preview?.detail, preview?.extra].filter(Boolean).join(", ")}>
        <SessionMark state="needsYou" />
        <Text style={[styles.heading, { color: theme.foreground }]} numberOfLines={2}>
          {heading}
          {preview?.detail ? <Text style={preview.code ? styles.headingCode : undefined}>{` ${preview.detail}`}</Text> : null}
        </Text>
        {preview?.extra ? <Text style={[styles.extra, { color: theme.tertiary }]}>{preview.extra}</Text> : null}
      </View>
      {approval.description ? <Text style={[styles.description, { color: theme.muted }]}>{approval.description}</Text> : null}
      {question ? questions.map((item) => {
        const choice = choices[item.key] ?? { selected: [], own: "" };
        return (
          <View key={item.key} style={styles.question}>
            <Text style={[styles.description, { color: theme.foreground }]}>{item.question}</Text>
            {item.options.map((option) => {
              const selected = choice.selected.includes(option.label) && (item.multiSelect || !choice.own.trim());
              return (
                <Pressable
                  key={option.label}
                  onPress={() => {
                    haptic("selection");
                    const values = item.multiSelect ? selected ? choice.selected.filter((label) => label !== option.label) : [...choice.selected, option.label] : [option.label];
                    setChoices((current) => ({ ...current, [item.key]: { selected: values, own: item.multiSelect ? choice.own : "" } }));
                  }}
                  disabled={disabled}
                  style={[styles.option, { borderColor: selected ? theme.foreground : theme.border }]}
                  accessibilityRole={item.multiSelect ? "checkbox" : "radio"}
                  accessibilityState={{ checked: selected, disabled }}
                  accessibilityLabel={[option.label, option.description].filter(Boolean).join(". ")}
                >
                  <Icon name={selected ? "picked" : "unpicked"} size={18} color={theme.foreground} />
                  <View style={styles.optionWords}>
                    <Text style={[styles.description, { color: theme.foreground }]}>{option.label}</Text>
                    {option.description ? <Text style={[styles.note, { color: theme.muted }, styles.left]}>{option.description}</Text> : null}
                  </View>
                </Pressable>
              );
            })}
            <TextInput
              value={choice.own}
              onChangeText={(own) => setChoices((current) => ({ ...current, [item.key]: { ...choice, own } }))}
              placeholder="Write your answer…"
              placeholderTextColor={theme.tertiary}
              editable={!disabled}
              secureTextEntry={item.isSecret}
              autoCorrect={!item.isSecret}
              autoCapitalize={item.isSecret ? "none" : "sentences"}
              maxLength={8000}
              style={[styles.answer, { color: theme.foreground, borderColor: theme.border }]}
              accessibilityLabel={`Your answer to: ${item.question}`}
            />
          </View>
        );
      }) : preview?.body ? (
        <PreviewView body={preview.body} theme={theme} />
      ) : null}
      {question && !questions.length ? <Text style={[styles.description, { color: theme.muted }]}>This question couldn't be displayed. Skip it to ask the agent to try again.</Text> : null}
      {!question && note === null ? (
        <Pressable onPress={() => { haptic("tap"); setNote(""); }} disabled={disabled} style={styles.noteToggle} accessibilityRole="button" accessibilityHint="Adds a note that is sent to the agent when you deny" testID={`code-approval-note-${approval.requestId}`}>
          <Text style={[styles.description, { color: theme.muted }]}>Tell the agent what to do instead</Text>
        </Pressable>
      ) : null}
      {!question && note !== null ? (
        <TextInput
          value={note}
          onChangeText={setNote}
          autoFocus
          placeholder="Tell the agent what to do instead"
          placeholderTextColor={theme.tertiary}
          editable={!disabled}
          multiline
          maxLength={8000}
          style={[styles.answer, styles.noteInput, { color: theme.foreground, borderColor: theme.border }]}
          accessibilityLabel="What the agent should do instead. Sent when you deny."
        />
      ) : null}
      <View style={styles.buttons}>
        <Pressable onPress={() => answer("deny")} disabled={disabled} style={[styles.button, { borderColor: theme.border }, disabled && styles.dim]} accessibilityRole="button" accessibilityState={{ disabled }}>
          <Text style={[styles.buttonText, { color: theme.foreground }]}>{question ? "Skip" : instead ? "Deny with note" : preview?.choices.deny ?? "Deny"}</Text>
        </Pressable>
        {preview?.choices.always ? (
          <Pressable onPress={() => answer("allowSession")} disabled={disabled} style={[styles.button, { borderColor: theme.border }, disabled && styles.dim]} accessibilityRole="button" accessibilityState={{ disabled }} accessibilityHint="Allows this kind of request for the rest of the session" testID={`code-approval-always-${approval.requestId}`}>
            <Text style={[styles.buttonText, { color: theme.foreground }]}>{preview.choices.always}</Text>
          </Pressable>
        ) : null}
        <Pressable onPress={() => answer("allow")} disabled={disabled || (question && !complete)} style={[styles.button, { backgroundColor: theme.primary, borderColor: theme.primary }, (disabled || (question && !complete)) && styles.dim]} accessibilityRole="button" accessibilityState={{ disabled: disabled || (question && !complete) }}>
          <Text style={[styles.buttonText, { color: theme.onPrimary }]}>{question ? "Submit answer" : preview?.choices.allow ?? "Allow once"}</Text>
        </Pressable>
      </View>
    </View>
  );
}

/**
 * What the request would do, as the desktop's dock previews it: a command or the tool's input in a code box, an edit
 * as its lines (added ones filled), a plan as Markdown. Code keeps its lines and scrolls sideways in its own box.
 */
function PreviewView({ body, theme }: { body: PreviewBody; theme: Theme }) {
  if (body.kind === "plan") {
    return (
      <ScrollView style={[styles.preview, styles.plan, { backgroundColor: theme.grouped }]} nestedScrollEnabled>
        <Markdown source={body.text} />
      </ScrollView>
    );
  }
  return (
    <ScrollView style={[styles.preview, { backgroundColor: theme.grouped }]} nestedScrollEnabled>
      <ScrollView horizontal contentContainerStyle={styles.previewInner} showsHorizontalScrollIndicator={false}>
        {body.kind === "diff" ? (
          <View>
            {body.rows.map((row, i) => (
              <Text key={i} selectable style={[styles.code, { color: row.sign === "+" ? theme.foreground : theme.tertiary, backgroundColor: row.sign === "+" ? theme.fill : undefined }]}>
                {`${row.sign === "-" ? "−" : row.sign} ${row.text}`}
              </Text>
            ))}
          </View>
        ) : (
          <Text selectable style={[styles.code, { color: theme.foreground }]}>
            {body.kind === "command" ? `$ ${body.text}` : body.text}
          </Text>
        )}
      </ScrollView>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  dock: { gap: 4 },
  activity: { flexDirection: "row", alignItems: "center", gap: 8, minHeight: 28, paddingHorizontal: 18 },
  activityWords: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", gap: 2 },
  activityText: { ...text.caption, fontFamily: font.medium, flexShrink: 1 },
  planChip: { flexDirection: "row", alignItems: "center", gap: 5, height: 24, paddingHorizontal: 9, borderRadius: 12 },
  planText: { ...text.caption, fontFamily: font.medium, fontVariant: ["tabular-nums"] },
  approvalHead: { flexDirection: "row", alignItems: "center", gap: 6, marginLeft: -4 },
  headingCode: { fontFamily: mono, fontSize: 14 },
  extra: { ...text.caption, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
  preview: { maxHeight: 180, borderRadius: 10, borderCurve: "continuous" },
  plan: { paddingHorizontal: 12, paddingVertical: 4 },
  previewInner: { padding: 10 }, status: { minHeight: 24, paddingHorizontal: 24, justifyContent: "center" }, toolbar: composerControls.toolbar, settings: composerControls.choices,
  approvals: { maxHeight: 340 },
  approvalList: { gap: 10, paddingBottom: 8, paddingHorizontal: 16 },
  approval: { borderWidth: StyleSheet.hairlineWidth, borderRadius: 16, padding: 14, gap: 10 },
  heading: { ...text.body, fontFamily: font.medium, flex: 1 },
  description: { ...text.sm, fontFamily: font.regular },
  question: { gap: 8 },
  option: { minHeight: 44, borderWidth: 1, borderRadius: 12, padding: 10, flexDirection: "row", alignItems: "center", gap: 9 },
  optionWords: { flex: 1, gap: 2 },
  answer: { minHeight: 44, borderWidth: StyleSheet.hairlineWidth, borderRadius: 12, paddingHorizontal: 12, ...text.base, fontFamily: font.regular },
  code: { fontFamily: mono, fontSize: 12, lineHeight: 18 },
  buttons: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: 12, paddingTop: 4 },
  button: { minHeight: 44, borderWidth: 1, borderRadius: 22, paddingHorizontal: 18, alignItems: "center", justifyContent: "center" },
  buttonText: { ...text.sm, fontFamily: font.medium },
  dim: { opacity: 0.4 },
  later: { minHeight: 36, maxWidth: 240, borderRadius: 18, paddingHorizontal: 14, alignItems: "center", justifyContent: "center" },
  note: { ...text.caption, fontFamily: font.regular, textAlign: "center" },
  left: { textAlign: "left" },
  retry: { minHeight: 44, justifyContent: "center", alignItems: "center" },
  noteToggle: { minHeight: 44, justifyContent: "center" },
  noteInput: { paddingVertical: 10, maxHeight: 120 },
});
