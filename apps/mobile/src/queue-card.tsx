import { useState } from "react";
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { cannotSendQueued, type Queued } from "@/chat-queue";
import { TextInput } from "@/controls";
import { Glass } from "@/glass";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { CHROME_FONT_SCALE } from "@/text-size";
import { font, text, useTheme } from "@/theme";

type Held = "Stopped" | "The turn failed" | "Ready to send";
type ViewState = { kind: "closed" | "all" } | { kind: "actions"; id: string } | { kind: "edit"; id: string; value: string };
const PREVIEW_COUNT = 3;

function labelOf(item: Queued) {
  return item.content || item.files?.map((file) => file.input.name).join(", ") || "Images";
}

export function QueueCard({ queue, held, sending, onSendNow, onSendTextOnly, onRemove, onClear, onMove, onEdit, onSave }: {
  queue: Queued[];
  held?: Held;
  sending?: string;
  onSendTextOnly: (id: string) => void;
  onSendNow: (id: string) => void;
  onRemove: (id: string) => void;
  onClear: () => void;
  onMove: (id: string, direction: -1 | 1) => void;
  onEdit: (id: string | null) => void;
  onSave: (id: string, value: string) => void;
}) {
  const theme = useTheme();
  const { height } = useWindowDimensions();
  const [view, setView] = useState<ViewState>({ kind: "closed" });
  const close = () => {
    if (view.kind === "edit") onEdit(null);
    setView({ kind: "closed" });
  };
  const active = view.kind === "actions" || view.kind === "edit" ? queue.find((item) => item.id === view.id) : undefined;
  const index = active ? queue.indexOf(active) : -1;
  const openActions = (id: string) => {
    haptic("tap");
    setView({ kind: "actions", id });
  };
  const action = (run: () => void) => {
    close();
    run();
  };
  const row = (item: Queued, index: number) => {
    const label = labelOf(item);
    return (
      <View key={item.id} style={[styles.row, index > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: theme.separator }]}>
        <Text style={[styles.index, { color: theme.tertiary }]} maxFontSizeMultiplier={CHROME_FONT_SCALE}>{index + 1}</Text>
        <View style={styles.content}>
          <Text style={[styles.message, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>{label}</Text>
          {item.missingAttachments || item.delivery ? (
            <Text style={[styles.status, { color: theme.muted }]} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
              {item.missingAttachments ? "Attachment unavailable" : item.delivery === "unconfirmed" ? "Check the transcript" : "Waiting for retry"}
            </Text>
          ) : null}
        </View>
        {sending === item.id ? <View style={styles.button}><ActivityIndicator color={theme.muted} accessibilityLabel="Sending" /></View> : <Pressable style={styles.button} onPress={() => openActions(item.id)} accessibilityRole="button" accessibilityLabel={`Actions for queued message ${index + 1}: ${label}`}>
          <Icon name="more" size={18} color={theme.foreground} />
        </Pressable>}
        {sending !== item.id && !cannotSendQueued(item) ? (
          <Pressable style={styles.button} onPress={() => action(() => onSendNow(item.id))} accessibilityRole="button" accessibilityLabel={`${item.delivery === "retry" ? "Retry" : "Send now"}: ${label}`}>
            <Icon name="send" size={18} color={theme.foreground} />
          </Pressable>
        ) : null}
      </View>
    );
  };
  const first = queue[0];
  if (!first) return null;
  const starting = !!sending || first.now;
  return (
    <View style={styles.room}>
      <Glass style={styles.card}>
        <Text style={[styles.title, { color: theme.muted }]} accessibilityRole="header" maxFontSizeMultiplier={CHROME_FONT_SCALE}>
          {starting ? "Sending next" : held ? `${held} · ${queue.length} held` : `Queued · ${queue.length}`}
        </Text>
        {held && !starting ? (
          <View style={styles.held}>
            {!cannotSendQueued(first) ? (
              <Pressable style={styles.textButton} onPress={() => onSendNow(first.id)} accessibilityRole="button">
                <Text style={[styles.actionText, { color: theme.foreground }]}>{first.delivery === "retry" ? "Retry" : "Send next"}</Text>
              </Pressable>
            ) : null}
            <Pressable style={styles.textButton} onPress={onClear} accessibilityRole="button">
              <Text style={[styles.actionText, { color: theme.muted }]}>Clear</Text>
            </Pressable>
          </View>
        ) : null}
        {queue.slice(0, PREVIEW_COUNT).map(row)}
        {queue.length > PREVIEW_COUNT ? (
          <Pressable style={styles.textButton} onPress={() => { haptic("tap"); setView({ kind: "all" }); }} accessibilityRole="button">
            <Text style={[styles.actionText, { color: theme.foreground }]}>{queue.length - PREVIEW_COUNT} more · Show all</Text>
          </Pressable>
        ) : null}
      </Glass>
      <Sheet visible={view.kind === "all" || !!active} onClose={close}>
        {view.kind === "all" ? <>
          <SheetTitle>Queued messages</SheetTitle>
          <ScrollView style={{ maxHeight: height * 0.55 }}>{queue.map(row)}</ScrollView>
        </> : null}
        {view.kind === "actions" && active && active.id !== sending ? <>
          <SheetTitle>Queued message {index + 1}</SheetTitle>
          {!active.attempt ? <SheetRow label="Edit message" onPress={() => {
            haptic("tap");
            onEdit(active.id);
            setView({ kind: "edit", id: active.id, value: active.content });
          }} /> : null}
          {active.missingAttachments && !active.attempt && active.content ? (
            <SheetRow label="Send text only" onPress={() => action(() => onSendTextOnly(active.id))} />
          ) : null}
          {index > 0 ? <SheetRow label="Move up" onPress={() => action(() => onMove(active.id, -1))} /> : null}
          {index < queue.length - 1 ? <SheetRow label="Move down" onPress={() => action(() => onMove(active.id, 1))} /> : null}
          <SheetRow label="Remove" tone="danger" onPress={() => action(() => onRemove(active.id))} />
        </> : null}
        {view.kind === "edit" && active ? <>
          <SheetTitle>Edit queued message {index + 1}</SheetTitle>
          <TextInput
            accessibilityLabel="Queued message"
            multiline
            autoFocus
            value={view.value}
            onChangeText={(value) => setView({ ...view, value })}
            style={[styles.editor, { backgroundColor: theme.fill, color: theme.foreground }]}
          />
          {view.value.trim() || active.images?.length || active.files?.length ? (
            <SheetRow label="Save" tone="primary" onPress={() => action(() => onSave(active.id, view.value.trim()))} />
          ) : null}
          <SheetRow label="Cancel" onPress={close} />
        </> : null}
      </Sheet>
    </View>
  );
}

const styles = StyleSheet.create({
  room: { paddingHorizontal: 12, paddingBottom: 8 },
  card: { borderRadius: 20, borderCurve: "continuous", overflow: "hidden", paddingTop: 8 },
  title: { ...text.caption, fontFamily: font.medium, paddingHorizontal: 16, paddingBottom: 2 },
  row: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 44, paddingLeft: 16, paddingRight: 4 },
  index: { ...text.caption, fontFamily: font.regular, fontVariant: ["tabular-nums"], minWidth: 16 },
  content: { flex: 1 },
  message: { ...text.secondary, fontFamily: font.regular },
  status: { ...text.xs, fontFamily: font.regular },
  button: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  held: { flexDirection: "row", flexWrap: "wrap", alignItems: "center" },
  textButton: { minHeight: 44, justifyContent: "center", paddingHorizontal: 16 },
  actionText: { ...text.sm, fontFamily: font.medium },
  editor: { ...text.body, fontFamily: font.regular, marginHorizontal: 16, marginVertical: 8, padding: 12, minHeight: 88, maxHeight: 170, borderRadius: 12 },
});
