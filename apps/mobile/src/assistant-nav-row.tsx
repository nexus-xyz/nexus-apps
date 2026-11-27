import { errandName, type AgentClient, type AgentConversation, type AgentErrand, type AgentErrandList } from "@nexus/kit";
import { useEffect, useState, useSyncExternalStore } from "react";
import { StyleSheet, Text, View } from "react-native";
import { activityOf, subscribe } from "@/assistant-activity";
import { chipLabel, errandsOn, inProject, stripAfter } from "@/agent-errands";
import { AssistantMark } from "@/assistant-mark";
import { ErrandShape } from "@/errand-shape";
import { rowLabel, rowState, unreadPill, type RowState, type Seen } from "@/assistant-row";
import { useAuth, useStored } from "@/auth";
import { useErrandMenu } from "@/errand-menu";
import { NavRow } from "@/nav-row";
import { RowMenu } from "@/row-menu";
import { ACCESSIBILITY_FONT_SCALE } from "@/text-size";
import { font, useTheme } from "@/theme";

/** The errands list is read only where the strip shows errands (`errands-strip.tsx`). */
const errands = errandsOn(process.env.EXPO_PUBLIC_NEXUS_AGENT_ERRANDS);

/**
 * The Assistant's row at the top of the drawer, with its states by shape (A21·c, A2·B): the mark at 16 pt, thinking
 * while an answer streams, and the unread count in its pill. Under it, where errands show, one row per open errand,
 * indented 16 pt, with its state by shape and "Needs you" when it waits on the person; a tap opens it on the
 * errand's own thread (`onErrand`), and a long press opens its menu: Pin, Rename, Mark done and Archive
 * (`errand-menu.tsx`). `current`: the errand on screen, whose row is filled. The Assistant's own row says "Needs you"
 * only when its chat waits on an approval. Each time
 * the drawer opens, and each time the screen shows replies or an answer ends, it reads the kept conversation and,
 * where errands show, the errands (`assistant-row.ts` says what counts). A read that fails leaves the rows as they were.
 */
export function AssistantNavRow({
  agent,
  opened,
  selected,
  label,
  hiddenForMembers,
  current,
  onPress,
  onErrand,
}: {
  agent: AgentClient;
  opened: boolean;
  selected: boolean;
  label: string;
  hiddenForMembers?: boolean;
  current?: string;
  onPress: () => void;
  onErrand: (id: string) => void;
}) {
  const theme = useTheme();
  const { org } = useAuth();
  const kept = useStored<string>("cache", "agent-conversation");
  const seenStore = useStored<Seen>("cache", "assistant-seen");
  const activity = useSyncExternalStore(subscribe, () => activityOf(org));
  const [row, setRow] = useState<RowState>({ unread: 0, needsYou: false });
  const [open, setOpen] = useState<AgentErrand[]>([]);
  // An edit from a row's menu reads the list again.
  const [edits, setEdits] = useState(0);
  const menu = useErrandMenu(agent, () => setEdits((count) => count + 1));

  useEffect(() => {
    // While an answer streams the screen is the one to look at: the row reads again when it ends.
    if (!opened || activity.streaming) return;
    const controller = new AbortController();
    const conversation = kept.load();
    const quiet = <T,>(read: Promise<T>) => read.catch(() => undefined);
    void Promise.all([
      conversation ? quiet<AgentConversation>(agent.conversation(conversation, controller.signal)) : undefined,
      errands ? quiet<AgentErrandList>(agent.errands(controller.signal)) : undefined,
    ]).then(([read, list]) => {
      if (controller.signal.aborted || (conversation && !read)) return;
      const next = rowState({ conversation, read, errands: list, seen: seenStore.load() });
      if (next.baseline) seenStore.save(next.baseline);
      setRow(next);
      if (list) {
        const strip = stripAfter(list);
        setOpen(strip.kind === "on" ? strip.errands : []);
      }
    });
    return () => controller.abort();
  }, [opened, activity.streaming, activity.seen, agent, kept, seenStore, edits]);

  const thinking = activity.streaming;
  // An errand that needs the person says so on its own row (A2·B).
  const chatNeedsYou = row.needsYou && !open.some((errand) => errand.status === "needs-you");
  return (
    <>
      <NavRow
        leading={
          <View style={styles.markSlot}>
            <AssistantMark size={16} thinking={thinking} />
          </View>
        }
        label={label}
        detail={hiddenForMembers ? "Hidden for members" : undefined}
        badge={unreadPill(row.unread)}
        trailing={chatNeedsYou ? <NeedsYou color={theme.foreground} /> : null}
        selected={selected}
        onPress={onPress}
        accessibilityLabel={hiddenForMembers ? `${rowLabel({ ...row, thinking })}, Hidden for members` : rowLabel({ ...row, thinking })}
      />
      {open.map((errand) => (
        <RowMenu key={errand.id} title={errandName(errand)} actions={menu.actions(errand)}>
          {(onLongPress) => (
            <NavRow
              leading={
                <View style={[styles.markSlot, styles.indent]}>
                  <ErrandShape status={errand.status} color={theme.foreground} />
                </View>
              }
              label={errandName(errand)}
              detail={inProject(errand)}
              trailing={errand.status === "needs-you" ? <NeedsYou color={theme.foreground} /> : null}
              selected={current === errand.id}
              onPress={() => onErrand(errand.id)}
              onLongPress={onLongPress}
              accessibilityLabel={chipLabel(errand)}
            />
          )}
        </RowMenu>
      ))}
      {open.length ? <View style={styles.after} /> : null}
      {menu.sheet}
    </>
  );
}

/** A filled dot and its words: the row's Needs you (A21·c). */
function NeedsYou({ color }: { color: string }) {
  return (
    <View style={styles.needs} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <View style={[styles.dot, { backgroundColor: color }]} />
      <Text style={[styles.needsText, { color }]} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
        Needs you
      </Text>
    </View>
  );
}

const styles = StyleSheet.create({
  needs: { flexDirection: "row", alignItems: "center", gap: 6 },
  dot: { width: 7, height: 7, borderRadius: 3.5 },
  needsText: { fontSize: 13, lineHeight: 18, fontFamily: font.regular },
  // The leading slot is 18 pt; an errand row sits 16 pt in, and 10 pt part the last from New session (A2·B).
  markSlot: { width: 18, alignItems: "center", justifyContent: "center" },
  indent: { marginLeft: 16 },
  after: { height: 10 },
});
