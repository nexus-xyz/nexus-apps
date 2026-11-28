import { WRITE_EDITED_LABEL, type AgentWriteDecision, type AgentWriteEditResult, type AgentWriteResult } from "@nexus/kit";
import { useState } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";
import { afterEdit, approvalCard, EDIT_UNSENT, editChanges, editStart, endedView, NOTHING_CHANGED, phaseAfter, UNSENT, type ApprovalPhase, type PendingMessage } from "@/assistant-cards";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { open } from "@/markdown";
import { font, mono, text, type Theme } from "@/theme";
import { WriteConsentPrompt } from "@/write-consent-prompt";

/** How the card reaches the agent API: the app's agent client, with the person's own sign-in. */
export type DecideWrite = (card: string, decision: AgentWriteDecision, options: { always?: boolean }) => Promise<AgentWriteResult>;

/** How the card edits a waiting write (A24·b): the agent API's `PATCH /agent/writes/{card}`, with the person's own sign-in. */
export type EditWrite = (card: string, changes: Record<string, string | string[]>) => Promise<AgentWriteEditResult>;

/** Edit while it is open: what the fields hold, whether Save is sent, and why the last Save did not go. */
type Editing = { values: Record<string, string>; saving: boolean; problem?: string };

/**
 * The approval card (A9·b, AIPDAsstApprovals), in the thread as Nexus UI and desktop draw it (`agent-approval-card.tsx`):
 * the tier by its shape (a dot for Asks first, a square for Always asks, in the text's colour: A1·D's cards are never
 * coloured), what it
 * does with the verb first, To, the amount, Why, the exact text as it goes, then Decline and Approve, "Always allow
 * this kind" where the card offers it, and when it expires. Approve and Decline call the agent API (`decide`); the
 * card then shows how it ended in its place. The exact text reads as A7·a draws it: an email's subject
 * first, the body, then the signature or "Sent using Nexus" quieter under it. A write that made something with a
 * title, such as an event, ends with that title in its own row and Open (A7·f).
 * A write Gateway refused with 403 `write_consent_required` ends with the write consent prompt under it
 * (`write-consent-prompt.tsx`): the person allows writes at the service, then asks again. Nothing is sent again.
 *
 * A write that always asks is approved on its page in Nexus alone: the card offers Decline and Review in Nexus. Without
 * `decide`, or from an agent API without the routes (no `inChat`), the card offers only that link. Everything in the
 * card is the agent's text, drawn as text.
 *
 * Edit (A24·b; A24's phone frames): with `edit`, a write that asks first has Edit before Decline. It opens
 * the fields the card lists in place of To and the text. Save sends only the changed fields; the edited card replaces
 * this one, says "Edited by you", and Decline and Approve decide it as before. The To line says "new to this errand"
 * after a recipient the errand has not written to yet.
 */
export function ApprovalCard({ message: given, theme, decide, edit }: { message: PendingMessage; theme: Theme; decide?: DecideWrite; edit?: EditWrite }) {
  // When the card was drawn: its expiry reads from then. A decision past it is refused anyway (410).
  const [drawnAt] = useState(Date.now);
  const [phase, setPhase] = useState<ApprovalPhase>({ kind: "open" });
  const [always, setAlways] = useState(false);
  // The edited card replaces this one (A24·b).
  const [replacement, setReplacement] = useState<PendingMessage>();
  const [editing, setEditing] = useState<Editing>();
  const message = replacement ?? given;
  const card = approvalCard(message, drawnAt, !!decide, !!edit);
  const working = phase.kind === "working";
  const locked = working || card.expired;

  const save = async () => {
    if (!edit || !editing || editing.saving) return;
    const changes = editChanges(message, editing.values);
    if (!Object.keys(changes).length) return setEditing({ ...editing, problem: NOTHING_CHANGED });
    haptic("tap");
    setEditing({ ...editing, saving: true, problem: undefined });
    let next: ReturnType<typeof afterEdit>;
    try {
      next = afterEdit(await edit(message.card, changes));
    } catch {
      next = { kind: "refused", text: EDIT_UNSENT };
    }
    if (next.kind === "edited") {
      haptic("success");
      setReplacement(next.card);
      setEditing(undefined);
      setAlways(false);
      return;
    }
    haptic("error");
    setEditing({ ...editing, saving: false, problem: next.text });
  };

  const choose = async (decision: AgentWriteDecision) => {
    if (!decide || working) return;
    haptic("tap");
    setPhase({ kind: "working", decision });
    let next: ApprovalPhase;
    try {
      next = phaseAfter(await decide(message.card, decision, { always: decision === "approve" && always }));
    } catch {
      next = UNSENT;
    }
    setPhase(next);
    haptic(next.kind === "ended" && next.decided.outcome !== "failed" ? "success" : "error");
  };

  const shape = card.tier === "always-asks" ? styles.square : styles.dot;
  return (
    <View style={[styles.card, { backgroundColor: theme.card, borderColor: theme.border }]} accessibilityLabel="A write that waits for you">
      <View style={styles.tier}>
        <View style={[shape, { backgroundColor: theme.foreground }]} />
        <Text style={[styles.tierText, { color: theme.muted }]}>{card.edited ? `${card.tierLabel} · ${WRITE_EDITED_LABEL}` : card.tierLabel}</Text>
      </View>
      <Text style={[styles.what, { color: theme.foreground }]}>{card.summary}</Text>
      {(card.to && !editing) || card.amount || card.why ? (
        <View style={styles.facts}>
          {card.to && !editing ? <Fact label="To" value={card.to} theme={theme} /> : null}
          {card.amount ? <Fact label="Amount" value={card.amount} theme={theme} /> : null}
          {card.why ? <Fact label="Why" value={card.why} theme={theme} quiet /> : null}
        </View>
      ) : null}
      {editing ? (
        <View style={styles.editFields}>
          {card.edit.map((field) => (
            <View key={field.name} style={styles.field}>
              <Text style={[styles.small, { color: theme.tertiary }]}>{field.label}</Text>
              <TextInput
                style={[styles.input, field.kind === "lines" && styles.inputLines, { backgroundColor: theme.fill, color: theme.foreground, fontFamily: font.regular }]}
                value={editing.values[field.name] ?? ""}
                onChangeText={(value) => setEditing({ ...editing, values: { ...editing.values, [field.name]: value }, problem: undefined })}
                editable={!editing.saving}
                multiline={field.kind === "lines"}
                autoCapitalize={field.kind === "addresses" || field.kind === "time" ? "none" : "sentences"}
                autoCorrect={field.kind === "line" || field.kind === "lines"}
                keyboardType={field.kind === "addresses" ? "email-address" : "default"}
                placeholder={field.kind === "addresses" ? "name@example.com, …" : field.kind === "time" ? "2026-11-20T15:00:00Z" : undefined}
                placeholderTextColor={theme.tertiary}
                accessibilityLabel={field.label}
              />
            </View>
          ))}
        </View>
      ) : card.text ? (
        <View style={[styles.exact, { backgroundColor: theme.fill }]}>
          {card.subject ? (
            <Text style={[styles.small, { color: theme.foreground, fontFamily: font.medium }]} selectable>
              Subject: {card.subject}
            </Text>
          ) : null}
          <Text style={[styles.body, { color: theme.foreground }]} selectable>
            {card.text}
          </Text>
          {card.end ? (
            <Text style={[styles.small, { color: theme.muted }]} selectable>
              {card.end}
            </Text>
          ) : null}
        </View>
      ) : (
        <Text style={[styles.code, { color: theme.muted }]}>{card.technical}</Text>
      )}
      {phase.kind === "ended" ? (
        <Ended phase={phase} theme={theme} />
      ) : editing ? (
        <>
          {editing.problem ? (
            <Text style={[styles.small, { color: theme.danger }]} accessibilityRole="alert">
              {editing.problem}
            </Text>
          ) : null}
          <View style={styles.actions}>
            <Button label="Cancel" theme={theme} disabled={editing.saving} onPress={() => setEditing(undefined)} />
            <Button label={editing.saving ? "Saving…" : "Save"} theme={theme} primary disabled={editing.saving || card.expired} busy={editing.saving} onPress={() => void save()} />
          </View>
        </>
      ) : (
        <>
          {phase.kind === "refused" ? (
            <Text style={[styles.small, { color: theme.danger }]} accessibilityRole="alert">
              {phase.text}
            </Text>
          ) : null}
          {card.decline || card.approve || card.review ? (
            <View style={styles.actions}>
              {card.edit.length ? <Button label="Edit" theme={theme} disabled={locked} onPress={() => setEditing({ values: editStart(message), saving: false })} /> : null}
              {card.decline ? (
                <Button label={phase.kind === "working" && phase.decision === "decline" ? "Declining…" : "Decline"} theme={theme} disabled={locked} onPress={() => void choose("decline")} />
              ) : null}
              {card.approve ? (
                <Button
                  label={phase.kind === "working" && phase.decision === "approve" ? "Approving…" : "Approve"}
                  theme={theme}
                  primary
                  disabled={locked}
                  busy={phase.kind === "working" && phase.decision === "approve"}
                  onPress={() => void choose("approve")}
                />
              ) : card.review ? (
                <Button label="Review in Nexus" theme={theme} primary={!card.decline} link onPress={() => open(card.review!)} />
              ) : null}
            </View>
          ) : null}
          {card.always || card.expiry ? (
            <View style={styles.foot}>
              {card.always ? (
                <Pressable
                  style={styles.always}
                  onPress={() => {
                    haptic("selection");
                    setAlways((on) => !on);
                  }}
                  disabled={locked}
                  accessibilityRole="checkbox"
                  accessibilityState={{ checked: always, disabled: locked }}
                  hitSlop={8}
                >
                  <Icon name={always ? "picked" : "unpicked"} size={18} color={locked ? theme.tertiary : theme.foreground} />
                  <Text style={[styles.small, { color: locked ? theme.tertiary : theme.foreground }]}>Always allow this kind</Text>
                </Pressable>
              ) : null}
              <View style={styles.spacer} />
              {card.expiry ? <Text style={[styles.small, { color: card.expired ? theme.danger : theme.tertiary }]}>{card.expiry}</Text> : null}
            </View>
          ) : null}
          {!card.decline && !card.approve ? <Text style={[styles.small, { color: theme.muted }]}>{card.detail}</Text> : null}
        </>
      )}
    </View>
  );
}

function Fact({ label, value, theme, quiet }: { label: string; value: string; theme: Theme; quiet?: boolean }) {
  return (
    <View style={styles.fact}>
      <Text style={[styles.factLabel, { color: theme.tertiary }]}>{label}</Text>
      <Text style={[styles.small, styles.factValue, { color: quiet ? theme.muted : theme.foreground }]}>{value}</Text>
    </View>
  );
}

function Button({ label, theme, onPress, primary, disabled, busy, link }: { label: string; theme: Theme; onPress: () => void; primary?: boolean; disabled?: boolean; busy?: boolean; link?: boolean }) {
  return (
    <Pressable
      style={[styles.button, { backgroundColor: primary ? theme.primary : theme.fill }, disabled && styles.disabled]}
      onPress={onPress}
      disabled={disabled}
      accessibilityRole={link ? "link" : "button"}
      accessibilityState={{ disabled: !!disabled, busy: !!busy }}
      {...(link ? { accessibilityHint: "Opens Nexus, where you approve or decline it" } : {})}
    >
      {busy ? <ActivityIndicator size="small" color={theme.onPrimary} /> : null}
      <Text style={[styles.buttonText, { color: primary ? theme.onPrimary : theme.foreground }]}>{label}</Text>
    </Pressable>
  );
}

/** How it ended, in the card's place: written with its link, declined, still running, or why it did not run. */
function Ended({ phase, theme }: { phase: Extract<ApprovalPhase, { kind: "ended" }>; theme: Theme }) {
  const ended = endedView(phase.decided);
  const color = ended.tone === "danger" ? theme.danger : ended.tone === "done" ? theme.foreground : theme.muted;
  return (
    <View style={[styles.ended, { borderColor: theme.border }]} accessibilityRole="text" accessibilityLiveRegion="polite">
      <Text style={[styles.body, { color }]}>{ended.text}</Text>
      {ended.title ? (
        <View style={[styles.result, { backgroundColor: theme.fill }]}>
          <Text style={[styles.body, styles.resultTitle, { color: theme.foreground }]} numberOfLines={2}>
            {ended.title}
          </Text>
          {ended.url ? (
            <Pressable onPress={() => open(ended.url!)} accessibilityRole="link" accessibilityLabel={`Open ${ended.title}`} hitSlop={8}>
              <Text style={[styles.small, { color: theme.primary, fontFamily: font.medium }]}>Open</Text>
            </Pressable>
          ) : null}
        </View>
      ) : ended.url ? (
        <Pressable onPress={() => open(ended.url!)} accessibilityRole="link" hitSlop={8}>
          <Text style={[styles.small, styles.underline, { color: theme.primary }]}>Open it</Text>
        </Pressable>
      ) : null}
      {ended.always ? <Text style={[styles.small, { color: theme.muted }]}>{ended.always}</Text> : null}
      {phase.decided.consent ? <WriteConsentPrompt consent={phase.decided.consent} theme={theme} /> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  // A9·b and A2·A: raised with a hairline, never coloured; the tier a 7 pt shape in the text's colour beside 12 pt words,
  // the action at 15 pt, the facts at 13/1.45 beside a 64 pt label column, 12 pt between parts.
  card: { paddingVertical: 14, paddingHorizontal: 16, gap: 12, borderRadius: 18, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth },
  tier: { flexDirection: "row", alignItems: "center", gap: 6 },
  dot: { width: 7, height: 7, borderRadius: 3.5 },
  square: { width: 7, height: 7, borderRadius: 1.5 },
  tierText: { fontSize: 12, lineHeight: 16, fontFamily: font.regular },
  what: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
  facts: { gap: 6 },
  fact: { flexDirection: "row", gap: 10 },
  factLabel: { fontSize: 13, lineHeight: 19, fontFamily: font.regular, width: 64 },
  factValue: { flex: 1, fontSize: 13, lineHeight: 19 },
  // A24·b's Edit: a label over each field, on the fill, 15 pt text, 12 pt round; the message grows to five lines.
  editFields: { gap: 10 },
  field: { gap: 4 },
  input: { minHeight: 40, paddingHorizontal: 12, paddingVertical: 8, borderRadius: 12, borderCurve: "continuous", fontSize: 15, lineHeight: 20 },
  inputLines: { minHeight: 110, textAlignVertical: "top" },
  exact: { gap: 8, padding: 12, borderRadius: 12, borderCurve: "continuous" },
  body: { ...text.base, fontFamily: font.regular },
  small: { ...text.sm, fontFamily: font.regular },
  code: { ...text.xs, fontFamily: mono },
  underline: { textDecorationLine: "underline" },
  // The phone board's two buttons (A2·A): 38 pt, 12 pt round, sharing the width; the quiet one on the fill.
  actions: { flexDirection: "row", alignItems: "center", gap: 8 },
  spacer: { flex: 1 },
  button: { flex: 1, flexDirection: "row", alignItems: "center", gap: 6, height: 38, paddingHorizontal: 12, borderRadius: 12, borderCurve: "continuous", justifyContent: "center" },
  disabled: { opacity: 0.5 },
  buttonText: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
  foot: { flexDirection: "row", alignItems: "center", gap: 8 },
  always: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 32 },
  ended: { gap: 4, paddingTop: 10, borderTopWidth: StyleSheet.hairlineWidth },
  // A7·f's result row: the title it made, and Open, on the fill.
  result: { flexDirection: "row", alignItems: "center", gap: 12, marginTop: 4, paddingVertical: 10, paddingHorizontal: 12, borderRadius: 12, borderCurve: "continuous" },
  resultTitle: { flex: 1 },
});
