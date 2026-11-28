import { factsText, type AgentClient, type MemoryLine } from "@nexus/kit";
import { router, Stack, useLocalSearchParams } from "expo-router";
import { useState } from "react";
import { Alert } from "react-native";
import { useAuth } from "@/auth";
import { errandHref } from "@/drawer-places";
import { Blank, Footnote, Loading, Page, Row, Section } from "@/list";
import { NoteSheet, useMemory } from "@/memory-state";
import { editDraft, fileAt, learnedFrom, lineKey, lineRemoval, lineSource } from "@/memory-words";
import { shortDay } from "@/people-controls-words";
import { Sheet } from "@/sheet";
import { SheetRow, SheetTitle } from "@/sheet-rows";
import { useToast } from "@/toast";
import { useAssistantName } from "@/use-assistant-name";

/** iOS drops what opens while a sheet still closes (`assistant-contact-sheet.tsx`). */
const AFTER_CLOSE = 450;

/**
 * One memory file (A16), as board A16 draws "A file" for the phone: each fact, and under it where it came
 * from and its day ("Contacts · 1 Oct"). A tap on a fact asks what to do: Show where (with Open the errand when an
 * errand taught it), Edit with the memory controls on, and Delete. Delete takes the line out of its note, or forgets
 * the note when the line is its only one. Text stays plain: a note is never drawn as Markdown.
 */
export default function MemoryFile() {
  const { agent } = useAuth();
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  return <FilePage agent={agent} />;
}

function FilePage({ agent }: { agent: AgentClient }) {
  const { path } = useLocalSearchParams<{ path?: string }>();
  const assistant = useAssistantName();
  const toast = useToast();
  const memory = useMemory(agent);
  const [editing, setEditing] = useState<{ note: string; text: string } | undefined>();
  const [picked, setPicked] = useState<MemoryLine | undefined>();
  const { read, working, change } = memory;

  if (read.kind === "loading") return <Loading alone />;
  if (read.kind === "problem") return <Blank title={read.title} body={read.detail} tone="danger" />;
  const view = read.view;
  const file = fileAt(view, path);
  if (!file) {
    return (
      <>
        <Stack.Screen options={{ title: "" }} />
        <Blank title="This file is empty" body={`${assistant} keeps nothing more in it. Go back to Memory for the other files.`} />
      </>
    );
  }
  const controls = view.controls === true;

  const remove = (line: MemoryLine) => {
    const removal = lineRemoval(line, controls, assistant);
    Alert.alert(removal.title, removal.body, [
      { text: "Cancel", style: "cancel" },
      {
        text: "Delete",
        style: "destructive",
        onPress: () =>
          void change(removal.edit !== undefined ? { edit: line.note.id, text: removal.edit } : { forget: line.note.id }).then(
            (done) => done && toast({ text: removal.edit !== undefined ? `${assistant} keeps the other lines.` : `${assistant} forgot the note.`, haptic: "success" }),
          ),
      },
    ]);
  };
  const where = (line: MemoryLine) => {
    const errand = line.note.source?.errand;
    Alert.alert("Learned from", learnedFrom(line, assistant), [
      ...(errand ? [{ text: "Open the errand", onPress: () => router.push(errandHref(errand.id)) }] : []),
      { text: "Close", style: "cancel" as const },
    ]);
  };
  // A sheet closes before the next sheet or alert opens, as the errand menu does (`errand-menu.tsx`).
  const then = (run: () => void) => {
    setPicked(undefined);
    setTimeout(run, AFTER_CLOSE);
  };
  const save = (text: string) => {
    if (!editing) return;
    void change({ edit: editing.note, text }).then((done) => {
      if (done?.kind !== "note") return;
      setEditing(undefined);
      toast({ text: `${assistant} uses the new note from its next message.`, haptic: "success" });
    });
  };

  return (
    <Page onRefresh={memory.refresh}>
      <Stack.Screen options={{ title: file.name }} />
      <Section title={file.title} footer={`${factsText(file.lines.length)} · edited ${shortDay(file.changedAt)}`}>
        {file.lines.map((line) => (
          <Row
            key={lineKey(line)}
            label={line.text}
            detail={lineSource(line)}
            detailTone="tertiary"
            disabled={working}
            onPress={() => setPicked(line)}
            accessibilityHint="Shows where it came from, and lets you edit or delete it"
          />
        ))}
      </Section>
      {file.errand ? (
        <Section>
          <Row label="Open the errand" tone="primary" onPress={() => router.push(errandHref(file.errand!.id))} />
        </Section>
      ) : null}
      <Footnote>{`${assistant} uses a change from its next message.`}</Footnote>
      <Sheet visible={picked !== undefined} onClose={() => setPicked(undefined)}>
        {picked ? (
          <>
            <SheetTitle>{picked.text}</SheetTitle>
            <SheetRow label="Show where" onPress={() => then(() => where(picked))} />
            {controls ? <SheetRow label="Edit" onPress={() => then(() => setEditing({ note: picked.note.id, text: editDraft(picked) }))} /> : null}
            <SheetRow label="Delete" tone="danger" onPress={() => then(() => remove(picked))} />
            <SheetRow label="Cancel" tone="muted" onPress={() => setPicked(undefined)} />
          </>
        ) : null}
      </Sheet>
      <NoteSheet
        title="Edit the note"
        text={editing?.text}
        submit="Save"
        hint={`Edit the note's lines. ${assistant} uses the new text from its next message.`}
        working={working}
        onText={(text) => setEditing((now) => now && { ...now, text })}
        onClose={() => setEditing(undefined)}
        onSubmit={save}
      />
    </Page>
  );
}
