import { errandName, memoryChangeProblem, memoryProblem, projectSubtitle, type AgentClient, type AgentMemoryEdit, type AgentMemoryNote, type AgentProjectView } from "@nexus/kit";
import { router, Stack, useLocalSearchParams } from "expo-router";
import { useEffect, useState } from "react";
import { Alert } from "react-native";
import { errandLine, historyLine } from "@/agent-errands";
import { useAuth } from "@/auth";
import { errandHref } from "@/drawer-places";
import { ErrandShape } from "@/errand-shape";
import { Blank, Empty, Footnote, Loading, Page, Row, Section } from "@/list";
import { open } from "@/markdown";
import { NoteSheet } from "@/memory-state";
import { FILES_FOOTER, KNOWS_FOOTER, linkedText, linkFailedText, linkLabel, noteDetail, noteToast, NO_FILES, projectNotes, projectOfParams, sessionLine, sessionOpen } from "@/project-words";
import { Sheet } from "@/sheet";
import { SheetRow, SheetTitle } from "@/sheet-rows";
import { useTheme } from "@/theme";
import { useToast } from "@/toast";

/** iOS drops what opens while a sheet still closes (`memory-file.tsx`). */
const AFTER_CLOSE = 450;

type Read = { kind: "loading" } | { kind: "gone" } | { kind: "problem"; text: string } | { kind: "ready"; view: AgentProjectView };

/**
 * One project (A14), as board A14 draws it for the phone: its errands, its sessions (a link card; a session
 * on the desktop says where to open it), its files (the repositories of a Gateway project, which keeps no file of its
 * own) and "What Nexus knows here", the notes only its errands read, with add, edit and forget. A name-only project
 * with a Gateway project of the same name offers "Link to <name>".
 */
export default function Project() {
  const { agent } = useAuth();
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  return <ProjectPage agent={agent} />;
}

function ProjectPage({ agent }: { agent: AgentClient }) {
  const params = useLocalSearchParams<{ id?: string; name?: string }>();
  const target = projectOfParams(params);
  const toast = useToast();
  const theme = useTheme();
  const [read, setRead] = useState<Read>({ kind: "loading" });
  const [reads, setReads] = useState(0);
  const [working, setWorking] = useState(false);
  const [picked, setPicked] = useState<AgentMemoryNote | undefined>();
  const [draft, setDraft] = useState<{ note?: string; text: string } | undefined>();
  const key = target ? `${target.id ?? ""}\n${target.name}` : "";

  useEffect(() => {
    if (!target) return;
    const controller = new AbortController();
    agent.projectView(target, controller.signal).then(
      (call) => {
        if (controller.signal.aborted) return;
        if (call.kind === "project") setRead({ kind: "ready", view: call.view });
        else if (call.kind === "gone") setRead({ kind: "gone" });
        else setRead({ kind: "problem", text: call.failure.detail || "This project could not be read. Try again in a moment." });
      },
      () => !controller.signal.aborted && setRead({ kind: "problem", text: "Nexus could not reach the Assistant. Try again in a moment." }),
    );
    return () => controller.abort();
    // `key` stands for the target's fields.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agent, key, reads]);

  if (!target) return <Blank title="This project is not here" body="Go back to Projects for the others." />;
  if (read.kind === "loading") return <Loading alone />;
  if (read.kind === "gone") return <Blank title="This project is not here" body="It is not one of yours, or it is gone. Go back to Projects for the others." />;
  if (read.kind === "problem") return <Blank title="The project did not open" body={read.text} tone="danger" />;
  const { view } = read;
  const project = view.project;
  const refresh = () => setReads((count) => count + 1);
  const offer = linkLabel(project);
  const notes = projectNotes(view.notes, project);

  const link = () => {
    if (!project.link || working) return;
    setWorking(true);
    agent.linkProject(project.name, project.link.id).then(
      (done) => {
        setWorking(false);
        if (done.kind === "failed") return Alert.alert("That was not done", linkFailedText(done.failure));
        toast({ text: linkedText(done), haptic: "success" });
        // The page now reads by the Gateway id.
        router.setParams({ id: done.project.id, name: done.project.name });
      },
      () => {
        setWorking(false);
        Alert.alert("That was not done", "Nexus could not reach the Assistant. Try again in a moment.");
      },
    );
  };

  /** Sends one note change, then reads the project again. */
  const change = (edit: AgentMemoryEdit, said: string) => {
    if (working) return;
    setWorking(true);
    agent.changeMemory(edit).then(
      (done) => {
        setWorking(false);
        if (done.kind === "failed") {
          const problem = "forget" in edit ? memoryProblem(done.failure.status, done.failure) : memoryChangeProblem(done.failure.status, done.failure);
          return Alert.alert(problem.title, problem.detail);
        }
        setDraft(undefined);
        toast({ text: said, haptic: "success" });
        refresh();
      },
      () => {
        setWorking(false);
        const problem = memoryChangeProblem(502);
        Alert.alert(problem.title, problem.detail);
      },
    );
  };

  const forget = (note: AgentMemoryNote) =>
    Alert.alert("Forget this note?", "Nexus forgets it on every surface.", [
      { text: "Cancel", style: "cancel" },
      { text: "Forget", style: "destructive", onPress: () => change({ forget: note.id }, noteToast("forgot")) },
    ]);
  const then = (run: () => void) => {
    setPicked(undefined);
    setTimeout(run, AFTER_CLOSE);
  };
  const submit = (text: string) => {
    if (!draft) return;
    if (draft.note) change({ edit: draft.note, text }, noteToast("edited"));
    else change({ add: text, project: { ...(project.id ? { id: project.id } : {}), name: project.name } }, noteToast("added"));
  };

  const errandRow = (errand: (typeof view.errands)[number], line: string) => (
    <Row
      key={errand.id}
      leading={<ErrandShape status={errand.status} color={theme.foreground} />}
      label={errandName(errand)}
      detail={line}
      detailTone="tertiary"
      chevron
      onPress={() => router.push(errandHref(errand.id))}
      accessibilityLabel={`${errandName(errand)}, ${line}`}
    />
  );

  return (
    <Page onRefresh={refresh}>
      <Stack.Screen options={{ title: project.name }} />
      <Footnote>{projectSubtitle(project, view.gateway)}</Footnote>
      {offer ? (
        <Section footer="Its errands and notes move to the Gateway project, and its repositories become its files.">
          <Row label={offer} tone="primary" disabled={working} onPress={link} />
        </Section>
      ) : null}
      <Section title="Errands">
        {view.errands.length ? view.errands.map((errand) => errandRow(errand, errandLine(errand))) : <Empty>No open errand here. Move one in from its menu.</Empty>}
      </Section>
      {view.history.length ? <Section title={`History · ${view.history.length}`}>{view.history.map((errand) => errandRow(errand, historyLine(errand)))}</Section> : null}
      {view.sessions.length ? (
        <Section title="Sessions">
          {view.sessions.map((session) => {
            const where = sessionOpen(session);
            return (
              <Row
                key={`${session.errand}:${session.url}`}
                label={session.title}
                detail={where.kind === "desktop" ? `${sessionLine(session)} · ${where.text}` : sessionLine(session)}
                detailTone="tertiary"
                chevron={where.kind === "link"}
                disabled={where.kind === "desktop"}
                onPress={where.kind === "link" ? () => open(where.url) : undefined}
              />
            );
          })}
        </Section>
      ) : null}
      <Section title="Files" footer={view.files.repos.length ? FILES_FOOTER : undefined}>
        {view.files.repos.length ? view.files.repos.map((repo) => <Row key={repo.fullName} label={repo.fullName} chevron onPress={() => open(repo.url)} />) : <Empty>{NO_FILES}</Empty>}
      </Section>
      <Section title="What Nexus knows here" footer={KNOWS_FOOTER}>
        {notes.map((note) => (
          <Row key={note.id} label={note.text} detail={noteDetail(note)} detailTone="tertiary" disabled={working} onPress={() => setPicked(note)} accessibilityHint="Lets you edit or forget the note" />
        ))}
        <Row label="Add a note" tone="primary" disabled={working} onPress={() => setDraft({ text: "" })} />
      </Section>
      <Sheet visible={picked !== undefined} onClose={() => setPicked(undefined)}>
        {picked ? (
          <>
            <SheetTitle>{picked.text}</SheetTitle>
            <SheetRow label="Edit" onPress={() => then(() => setDraft({ note: picked.id, text: picked.text }))} />
            <SheetRow label="Forget" tone="danger" onPress={() => then(() => forget(picked))} />
            <SheetRow label="Cancel" tone="muted" onPress={() => setPicked(undefined)} />
          </>
        ) : null}
      </Sheet>
      <NoteSheet
        title={draft?.note ? "Edit the note" : "Add a note"}
        text={draft?.text}
        submit={draft?.note ? "Save" : "Add"}
        hint={`Only errands filed in ${project.name} read it.`}
        placeholder="Raising a $12M Series A; lead by 15 Dec."
        working={working}
        onText={(text) => setDraft((now) => now && { ...now, text })}
        onClose={() => setDraft(undefined)}
        onSubmit={submit}
      />
    </Page>
  );
}
