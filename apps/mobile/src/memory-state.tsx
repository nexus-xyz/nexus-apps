import { AGENT_NOTE_CHARS, memoryChangeProblem, memoryProblem, noteDraft, type AgentClient, type AgentMemoryChange, type AgentMemoryEdit, type AgentMemoryView } from "@nexus/kit";
import { useEffect, useState } from "react";
import { Alert, StyleSheet, Text } from "react-native";
import { TextInput } from "@/controls";
import { useScreenFocused } from "@/screen-focus";
import { Sheet } from "@/sheet";
import { SheetRow, SheetTitle } from "@/sheet-rows";
import { font, text, useTheme } from "@/theme";

/** How a read of the memory stands. `problem`: its title and its words. */
export type MemoryRead = { kind: "loading" } | { kind: "view"; view: AgentMemoryView } | { kind: "problem"; title: string; detail: string };

/** The view after a change: a note forgotten, every note forgotten, a note kept or edited, or the switch. */
function applied(view: AgentMemoryView, edit: AgentMemoryEdit, change: AgentMemoryChange): AgentMemoryView {
  if (change.kind === "forgotten") return { ...view, notes: "forget" in edit ? view.notes.filter((note) => note.id !== edit.forget) : [] };
  if (change.kind === "note") {
    const kept = change.note;
    return { ...view, notes: "edit" in edit ? view.notes.map((note) => (note.id === edit.edit ? kept : note)) : [kept, ...view.notes] };
  }
  if (change.kind === "learning") return { ...view, learning: change.learning };
  return view;
}

/**
 * The person's memory (A16) for a page of the phone: read each time the page shows, so the list and a file agree
 * after a change on the other. `change` sends one change with the person's own sign-in, and the view follows it. A
 * failure says what happened in an alert, in the kit's words.
 */
export function useMemory(agent: AgentClient) {
  const focused = useScreenFocused();
  const [read, setRead] = useState<MemoryRead>({ kind: "loading" });
  const [reads, setReads] = useState(0);
  const [working, setWorking] = useState(false);

  useEffect(() => {
    if (!focused) return;
    const controller = new AbortController();
    agent.memory(controller.signal).then(
      (call) => {
        if (controller.signal.aborted) return;
        if (call.kind === "view") setRead({ kind: "view", view: call.view });
        else setRead({ kind: "problem", ...memoryProblem(call.failure.status, call.failure) });
      },
      () => !controller.signal.aborted && setRead({ kind: "problem", ...memoryProblem(502) }),
    );
    return () => controller.abort();
  }, [agent, focused, reads]);

  /** Sends one change; the change once done, or undefined once the problem is shown. */
  const change = async (edit: AgentMemoryEdit): Promise<AgentMemoryChange | undefined> => {
    if (working) return undefined;
    setWorking(true);
    try {
      const call = await agent.changeMemory(edit);
      if (call.kind === "failed") {
        const forget = "forget" in edit || "forgetAll" in edit;
        const problem = forget ? memoryProblem(call.failure.status, call.failure) : memoryChangeProblem(call.failure.status, call.failure);
        Alert.alert(problem.title, problem.detail);
        return undefined;
      }
      setRead((now) => (now.kind === "view" ? { kind: "view", view: applied(now.view, edit, call) } : now));
      return call;
    } catch {
      const problem = memoryChangeProblem(502);
      Alert.alert(problem.title, problem.detail);
      return undefined;
    } finally {
      setWorking(false);
    }
  };

  return { read, working, change, refresh: () => setReads((count) => count + 1) };
}

/**
 * A note's text in a sheet: Add or Save, and Cancel. The text is checked as the web checks it (`noteDraft`) before it
 * goes. `text` undefined closes the sheet.
 */
export function NoteSheet(props: {
  title: string;
  text: string | undefined;
  submit: string;
  hint: string;
  placeholder?: string;
  working: boolean;
  onText: (text: string) => void;
  onClose: () => void;
  onSubmit: (text: string) => void;
}) {
  const theme = useTheme();
  const [error, setError] = useState<string>();
  const open = props.text !== undefined;
  // A sheet that opens again starts without the last one's error.
  useEffect(() => setError(undefined), [open]);
  const save = () => {
    const draft = noteDraft(props.text ?? "");
    if ("error" in draft) return setError(draft.error);
    setError(undefined);
    props.onSubmit(draft.text);
  };
  return (
    <Sheet visible={props.text !== undefined} onClose={props.onClose}>
      <SheetTitle>{props.title}</SheetTitle>
      <TextInput
        style={[styles.input, { color: theme.foreground, backgroundColor: theme.screen, borderColor: error ? theme.danger : theme.border }]}
        value={props.text ?? ""}
        onChangeText={props.onText}
        multiline
        maxLength={AGENT_NOTE_CHARS}
        placeholder={props.placeholder}
        placeholderTextColor={theme.muted}
        editable={!props.working}
        autoFocus
        accessibilityLabel={props.title}
        accessibilityHint={props.hint}
      />
      <Text style={[styles.hint, { color: error ? theme.danger : theme.muted }]} accessibilityLiveRegion="polite">
        {error ?? props.hint}
      </Text>
      <SheetRow label={props.working ? "Saving…" : props.submit} tone="primary" onPress={save} />
      <SheetRow label="Cancel" tone="muted" onPress={props.onClose} />
    </Sheet>
  );
}

const styles = StyleSheet.create({
  input: { ...text.base, fontFamily: font.regular, minHeight: 88, maxHeight: 200, borderRadius: 14, borderWidth: 1, marginHorizontal: 16, paddingHorizontal: 14, paddingVertical: 12, textAlignVertical: "top" },
  hint: { ...text.xs, fontFamily: font.regular, marginHorizontal: 20, marginTop: 6, marginBottom: 4 },
});
