import type { ChatSessionSummary } from "@nexus/kit";
import { useEffect, useState } from "react";
import { StyleSheet } from "react-native";
import { useCore } from "@/auth";
import { TextInput } from "@/controls";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { font, text, useTheme } from "@/theme";

type Step = "menu" | "rename" | "delete";

/**
 * A chat's long-press menu: Rename, Archive and Delete (with a confirmation), or Restore and
 * Delete in Archived. It reports what changed; the list updates itself. `start` opens it on one step, for a menu that
 * already offered the choice (the drawer's context menu).
 */
export function ChatMenu(props: {
  chat: ChatSessionSummary | null;
  start?: Exclude<Step, "menu">;
  pin?: { label: string; onPress: () => void };
  onClose: () => void;
  onChanged: (chat: ChatSessionSummary) => void;
  onRemoved: (id: string) => void;
  onError: (message: string) => void;
}) {
  const core = useCore();
  const theme = useTheme();
  const [step, setStep] = useState<Step>("menu");
  const [title, setTitle] = useState("");
  const chat = props.chat;
  useEffect(() => {
    setStep(props.start ?? "menu");
    setTitle(chat?.title ?? "");
  }, [chat, props.start]);

  const run = (work: (chat: ChatSessionSummary) => Promise<void>) => {
    if (!chat) return;
    props.onClose();
    work(chat).catch((e: Error) => props.onError(e.message));
  };
  const rename = () => {
    const next = title.trim();
    if (next) run(async (c) => props.onChanged(await core.updateChatSession(c.id, { title: next })));
  };
  // Moved out of this list either way: archived leaves Chats, restored leaves Archived.
  const setArchived = (archived: boolean) =>
    run(async (c) => {
      await core.updateChatSession(c.id, { archived });
      props.onRemoved(c.id);
    });

  return (
    <Sheet visible={!!chat} onClose={props.onClose}>
      {step === "menu" ? (
        <>
          <SheetTitle>{chat?.title || "Untitled"}</SheetTitle>
          {chat?.archived ? (
            <SheetRow label="Restore" onPress={() => setArchived(false)} />
          ) : (
            <>
              {props.pin ? <SheetRow label={props.pin.label} onPress={() => { props.onClose(); props.pin?.onPress(); }} /> : null}
              <SheetRow label="Rename" onPress={() => setStep("rename")} />
              <SheetRow label="Archive" onPress={() => setArchived(true)} />
            </>
          )}
          <SheetRow label="Delete" tone="danger" onPress={() => setStep("delete")} />
          <SheetRow label="Cancel" tone="muted" onPress={props.onClose} />
        </>
      ) : step === "rename" ? (
        <>
          <SheetTitle>Rename</SheetTitle>
          <TextInput
            style={[styles.input, { color: theme.foreground, backgroundColor: theme.screen, borderColor: theme.border }]}
            value={title}
            onChangeText={setTitle}
            onSubmitEditing={rename}
            autoFocus
            selectTextOnFocus
            returnKeyType="done"
            accessibilityLabel="Chat title"
          />
          <SheetRow label="Save" tone={title.trim() ? "primary" : "muted"} onPress={rename} />
          <SheetRow label="Cancel" tone="muted" onPress={props.onClose} />
        </>
      ) : (
        <>
          <SheetTitle>Delete "{chat?.title || "Untitled"}" and its messages? This can't be undone.</SheetTitle>
          <SheetRow
            label="Delete"
            tone="danger"
            onPress={() =>
              run(async (c) => {
                await core.deleteChatSession(c.id);
                props.onRemoved(c.id);
              })
            }
          />
          <SheetRow label="Cancel" tone="muted" onPress={props.onClose} />
        </>
      )}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  input: { ...text.base, fontFamily: font.regular, minHeight: 48, borderRadius: 14, borderWidth: 1, marginHorizontal: 16, marginBottom: 4, paddingHorizontal: 14 },
});
