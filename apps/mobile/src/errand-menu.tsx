import { AGENT_ERRAND_PROJECT_MAX, AGENT_ERRAND_TITLE_MAX, errandName, errandProjects, type AgentClient, type AgentErrand, type AgentErrandEdit, type AgentErrandView } from "@nexus/kit";
import { useEffect, useState } from "react";
import { Alert, Platform, StyleSheet } from "react-native";
import { editedText, errandMenu, menuEdit, moveEdit, movedText, renameEdit, type ErrandMenuItem } from "@/agent-errands";
import { TextInput } from "@/controls";
import type { RowAction } from "@/row-menu";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { font, text, useTheme } from "@/theme";
import { useToast } from "@/toast";

/** iOS drops a sheet presented while its context menu still closes: the rename and move sheets wait it out. */
const AFTER_CONTEXT_MENU_MS = Platform.OS === "ios" ? 400 : 0;

type Step = "menu" | "rename" | "move";

const ICONS: Record<ErrandMenuItem["id"], (errand: Pick<AgentErrand, "pinned" | "archived">) => RowAction["icon"]> = {
  pin: (errand) => (errand.pinned ? { ios: "pin.slash", android: "pin" } : { ios: "pin", android: "pin" }),
  rename: () => ({ ios: "pencil", android: "edit" }),
  move: () => ({ ios: "folder", android: "folder" }),
  done: () => ({ ios: "checkmark.circle", android: "check" }),
  archive: (errand) => (errand.archived ? { ios: "tray.and.arrow.up", android: "unarchive" } : { ios: "archivebox", android: "archive" }),
};

/**
 * An errand's long-press menu (A2·B): Pin, Rename, Move to project (A14), Mark done and Archive,
 * through `PATCH /agent/errands/{id}`. `actions` gives a row's or a pill's menu (`RowMenu`). `open` shows the same items
 * in a sheet, for the thread's More button. Rename opens a field in a sheet. Move to project lists the projects the
 * person's errands are filed in (`errandProjects`), takes a new name, and moves a filed errand out. Each edit says what
 * it did in a toast, and `onChanged` gets the errand as it is now.
 */
export function useErrandMenu(agent: AgentClient, onChanged: (view: AgentErrandView) => void) {
  const toast = useToast();
  const [sheet, setSheet] = useState<{ errand: AgentErrand; step: Step } | null>(null);
  const [projects, setProjects] = useState<string[]>([]);

  const edit = (errand: AgentErrand, change: AgentErrandEdit) => {
    agent.errandEdit(errand.id, change).then(
      (read) => {
        if (read.kind === "errand") {
          toast({ text: change.project !== undefined ? movedText(change.project, errand) : editedText(change), haptic: "success" });
          onChanged(read.view);
        } else Alert.alert("That was not done", read.kind === "gone" ? "That errand is gone." : read.failure.detail || "Try again in a moment.");
      },
      () => Alert.alert("That was not done", "Nexus could not reach the Assistant. Try again in a moment."),
    );
  };

  /** Opens the move step with the projects the person's errands are filed in, read now. */
  const openMove = (errand: AgentErrand) => {
    setProjects([]);
    setSheet({ errand, step: "move" });
    agent.errands().then(
      (list) => list.kind === "list" && setProjects(errandProjects(list.errands, list.history ?? [])),
      () => undefined,
    );
  };

  const run = (errand: AgentErrand, id: ErrandMenuItem["id"], delay: number) => {
    if (id === "rename") setTimeout(() => setSheet({ errand, step: "rename" }), delay);
    else if (id === "move") setTimeout(() => openMove(errand), delay);
    else edit(errand, menuEdit(id, errand));
  };

  const actions = (errand: AgentErrand): RowAction[] =>
    errandMenu(errand).map((item) => ({ key: item.id, label: item.label, icon: ICONS[item.id](errand), onPress: () => run(errand, item.id, AFTER_CONTEXT_MENU_MS) }));

  const view = (
    <ErrandMenuSheet
      errand={sheet?.errand ?? null}
      step={sheet?.step ?? "menu"}
      projects={projects}
      onClose={() => setSheet(null)}
      onPick={(errand, id) => {
        if (id === "rename") return setSheet({ errand, step: "rename" });
        if (id === "move") return openMove(errand);
        setSheet(null);
        run(errand, id, 0);
      }}
      onRename={(errand, title) => {
        setSheet(null);
        edit(errand, renameEdit(title));
      }}
      onMove={(errand, project) => {
        setSheet(null);
        edit(errand, moveEdit(project));
      }}
    />
  );

  return { actions, open: (errand: AgentErrand) => setSheet({ errand, step: "menu" }), sheet: view };
}

/**
 * The menu as a sheet, titled with the errand's name; its Rename step, a field with the name, Save and Cancel; or its
 * Move step: the projects, a field for a new one, and Move out of the one it is in.
 */
function ErrandMenuSheet({
  errand,
  step,
  projects,
  onClose,
  onPick,
  onRename,
  onMove,
}: {
  errand: AgentErrand | null;
  step: Step;
  projects: string[];
  onClose: () => void;
  onPick: (errand: AgentErrand, id: ErrandMenuItem["id"]) => void;
  onRename: (errand: AgentErrand, title: string) => void;
  onMove: (errand: AgentErrand, project: string | null) => void;
}) {
  const theme = useTheme();
  const [title, setTitle] = useState("");
  const [project, setProject] = useState("");
  useEffect(() => {
    setTitle(errand ? errandName(errand) : "");
    setProject("");
  }, [errand, step]);
  const save = () => errand && onRename(errand, title);
  const fieldStyle = [styles.input, { color: theme.foreground, backgroundColor: theme.screen, borderColor: theme.border }];
  return (
    <Sheet visible={!!errand} onClose={onClose}>
      {errand && step === "menu" ? (
        <>
          <SheetTitle>{errandName(errand)}</SheetTitle>
          {errandMenu(errand).map((item) => (
            <SheetRow key={item.id} label={item.label} onPress={() => onPick(errand, item.id)} />
          ))}
          <SheetRow label="Cancel" tone="muted" onPress={onClose} />
        </>
      ) : errand && step === "rename" ? (
        <>
          <SheetTitle>Rename</SheetTitle>
          <TextInput
            style={fieldStyle}
            value={title}
            onChangeText={setTitle}
            onSubmitEditing={save}
            maxLength={AGENT_ERRAND_TITLE_MAX}
            placeholder={errand.goal}
            placeholderTextColor={theme.muted}
            autoFocus
            selectTextOnFocus
            returnKeyType="done"
            accessibilityLabel="Errand name"
            accessibilityHint="Leave it empty to name it by its goal"
          />
          <SheetRow label="Save" tone="primary" onPress={save} />
          <SheetRow label="Cancel" tone="muted" onPress={onClose} />
        </>
      ) : errand ? (
        <>
          <SheetTitle>Move {errandName(errand)} to a project</SheetTitle>
          {projects
            .filter((name) => name !== errand.project)
            .map((name) => (
              <SheetRow key={name} label={name} onPress={() => onMove(errand, name)} />
            ))}
          <TextInput
            style={fieldStyle}
            value={project}
            onChangeText={setProject}
            onSubmitEditing={() => project.trim() && onMove(errand, project)}
            maxLength={AGENT_ERRAND_PROJECT_MAX}
            placeholder="New project"
            placeholderTextColor={theme.muted}
            returnKeyType="done"
            accessibilityLabel="New project"
          />
          {project.trim() ? <SheetRow label={`Move to ${project.trim()}`} tone="primary" onPress={() => onMove(errand, project)} /> : null}
          {errand.project ? <SheetRow label={`Move out of ${errand.project}`} onPress={() => onMove(errand, null)} /> : null}
          <SheetRow label="Cancel" tone="muted" onPress={onClose} />
        </>
      ) : null}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  input: { ...text.base, fontFamily: font.regular, minHeight: 48, borderRadius: 14, borderWidth: 1, marginHorizontal: 16, marginBottom: 4, paddingHorizontal: 14 },
});
