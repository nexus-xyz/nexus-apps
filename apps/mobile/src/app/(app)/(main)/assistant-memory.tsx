import { EXPORT_FILE, factsText, memoryFiles, memoryZip, type AgentClient, type AgentMemoryView } from "@nexus/kit";
import { File, Paths } from "expo-file-system";
import type { Href } from "expo-router";
import { useState } from "react";
import { Alert, Platform, Share } from "react-native";
import { useAuth } from "@/auth";
import { Blank, Empty, Footnote, Loading, Page, Row, Section } from "@/list";
import { NoteSheet, useMemory } from "@/memory-state";
import { exportText, FORGET_ALL, forgotText, memoryFileHref, memoryHead } from "@/memory-words";
import { Switch } from "@/switch";
import { useToast } from "@/toast";
import { useAssistantName } from "@/use-assistant-name";

/**
 * Account › Assistant › Memory (A16), as board A16 draws it for the phone: "Personal · 40 facts · learning
 * on", then the memory files, one row per kind and one per errand under `errands/`, each with its fact count. A row
 * opens the file, with each fact and where it came from (`memory-file.tsx`). With the agent API's memory controls on,
 * the page also adds a note and pauses learning. Export shares a ZIP of the Markdown files and `memory.json`, built
 * on the phone from what it shows (the kit's `memoryZip`); Android's share sheet takes text, so there it shares the
 * Markdown. Forget all asks first. It reads the same agent API routes as Nexus UI's Memory view.
 */
export default function AssistantMemory() {
  const { agent } = useAuth();
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  return <MemoryPage agent={agent} />;
}

function MemoryPage({ agent }: { agent: AgentClient }) {
  const { org, orgName } = useAuth();
  const assistant = useAssistantName();
  const toast = useToast();
  const memory = useMemory(agent);
  const [adding, setAdding] = useState<string | undefined>();
  const { read, working, change } = memory;

  if (read.kind === "loading") return <Loading alone />;
  if (read.kind === "problem") return <Blank title={read.title} body={read.detail} tone="danger" />;
  const view = read.view;
  const files = memoryFiles(view.notes);
  const kinds = files.filter((file) => !file.errand);
  const errands = files.filter((file) => file.errand);
  const controls = view.controls === true;
  const paused = view.learning?.paused === true;

  const forgetAll = () =>
    Alert.alert(FORGET_ALL.title, FORGET_ALL.body(assistant), [
      { text: "Cancel", style: "cancel" },
      {
        text: "Forget all",
        style: "destructive",
        onPress: () => void change({ forgetAll: true }).then((done) => done?.kind === "forgotten" && toast({ text: forgotText(assistant, "all", done.count), haptic: "success" })),
      },
    ]);
  const learn = (next: boolean) =>
    void change({ paused: !next }).then((done) => done?.kind === "learning" && toast({ text: done.learning.paused ? "Learning is paused." : "Learning is on again." }));
  const add = (text: string) =>
    void change({ add: text }).then((done) => {
      if (done?.kind !== "note") return;
      setAdding(undefined);
      toast({ text: `${assistant} keeps the note, from its next message.`, haptic: "success" });
    });

  return (
    <Page onRefresh={memory.refresh}>
      <Section footer={memoryHead(view, orgName)}>
        {files.length === 0 ? (
          <Empty>{`${assistant} keeps no notes about you. After a conversation, ${assistant} writes a short note, and uses it later in the same place only.`}</Empty>
        ) : (
          kinds.map((file) => <Row key={file.path} label={file.name} monoLabel value={String(file.lines.length)} accessibilityLabel={`${file.name}, ${factsText(file.lines.length)}`} href={memoryFileHref(file.path) as Href} />)
        )}
      </Section>
      {errands.length ? (
        <Section title="errands/">
          {errands.map((file) => (
            <Row key={file.path} label={file.name} monoLabel detail={file.title} detailTone="tertiary" value={String(file.lines.length)} accessibilityLabel={`${file.title}, ${factsText(file.lines.length)}`} href={memoryFileHref(file.path) as Href} />
          ))}
        </Section>
      ) : null}
      {controls ? (
        <Section
          footer={paused ? `Learning is paused. ${assistant} keeps no new notes from your conversations, and still uses what it knows.` : `After a conversation, ${assistant} keeps a short note. Pause learning to stop new notes.`}
        >
          <Row label="Learning" accessory={<Switch value={!paused} disabled={working} accessibilityLabel="Learning" onValueChange={learn} />} />
          <Row label="Add a note" tone="primary" disabled={working} onPress={() => setAdding("")} />
        </Section>
      ) : null}
      <Section footer="A ZIP of the Markdown files and memory.json, made on this phone from what this page shows.">
        <Row label="Export" tone="primary" onPress={() => void exportMemory(view, org ?? "personal")} />
      </Section>
      {files.length ? (
        <Section>
          <Row label="Forget all" tone="danger" disabled={working} onPress={forgetAll} />
        </Section>
      ) : null}
      {orgName && orgName !== "Personal" ? <Footnote>{`Kept by ${orgName}’s Assistant. Your personal memory is kept apart.`}</Footnote> : null}
      <NoteSheet
        title={`A note for ${assistant}`}
        text={adding}
        submit="Add"
        hint={`${assistant} uses it in your direct messages, from its next message.`}
        placeholder="Lily is in 1st grade at Rooftop."
        working={working}
        onText={setAdding}
        onClose={() => setAdding(undefined)}
        onSubmit={add}
      />
    </Page>
  );
}

/** Shares the export: the ZIP as a file on iOS, the Markdown as text on Android. */
async function exportMemory(view: AgentMemoryView, org: string) {
  try {
    if (Platform.OS === "ios") {
      const file = new File(Paths.cache, EXPORT_FILE);
      if (file.exists) file.delete();
      file.writeSync(memoryZip(view, org));
      await Share.share({ url: file.uri, title: "Nexus memory" });
    } else {
      await Share.share({ title: "Nexus memory", message: exportText(view) });
    }
  } catch (error) {
    Alert.alert("The export did not open", error instanceof Error ? error.message : "Try again.");
  }
}
