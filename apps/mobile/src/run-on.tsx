import { router } from "expo-router";
import { useState } from "react";
import { Alert, Keyboard, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useCore, useStored } from "./auth";
import { useComputers } from "./computers";
import { keepCloudDraft, keepRunOnContinuation, transferDraft } from "./connection-intent";
import { haptic } from "./haptics";
import { Icon } from "./icon";
import { Sheet, SheetRow, SheetTitle } from "./sheet";
import { font, text, useTheme } from "./theme";
import { useCloudSessions } from "./use-cloud-sessions";

const cloudOn = process.env.EXPO_PUBLIC_NEXUS_CLOUD_SESSIONS === "on";

/** Run on is available only before a session starts. Existing sessions keep their origin. */
export function RunOn({ computerId, draft = "", onLeaveChat, canRunOnComputer = true, disabled = false }: { computerId?: string; draft?: string; onLeaveChat?: () => void; canRunOnComputer?: boolean; disabled?: boolean }) {
  const theme = useTheme();
  const core = useCore();
  const kept = useStored<string>("choice", "run-on");
  const [open, setOpen] = useState(false);
  const computers = useComputers();
  const cloud = useCloudSessions(core, open && cloudOn);
  const selected = computers.computers.find((c) => c.computerId === computerId);
  const choose = (id?: string) => {
    if (disabled) return;
    if (id && !canRunOnComputer) { Alert.alert("Computer sessions accept text", "Remove attachments before choosing a computer. Your message will stay here."); return; }
    setOpen(false);
    kept.save(id ?? "chat");
    if (id === computerId) return;
    transferDraft(core, draft);
    if (id) onLeaveChat?.();
    haptic("selection");
    router.replace({ pathname: "/session/[id]", params: { id: "new", runOn: id ? "computer" : "chat", ...(id ? { computer: id } : {}) } });
  };
  return <>
    <Pressable style={styles.chip} disabled={disabled} onPress={() => { haptic("tap"); Keyboard.dismiss(); void computers.refresh(); setOpen(true); }} accessibilityRole="button" accessibilityState={{ disabled }} accessibilityLabel={`Run on ${computerId ? selected?.name ?? "Computer" : "Chat"}`}>
      <Icon name={computerId ? "agents" : "compose"} size={15} color={theme.muted} />
      <Text style={[styles.label, { color: theme.foreground }]} numberOfLines={1}>Run on · {computerId ? selected?.name ?? "Computer" : "Chat"}</Text>
      <Icon name="chevron" size={10} color={theme.muted} />
    </Pressable>
    <Sheet visible={open} onClose={() => setOpen(false)}>
      <SheetTitle>Run on</SheetTitle>
      <ScrollView style={styles.options} keyboardShouldPersistTaps="handled">
        <SheetRow label={`${computerId ? "" : "✓ "}Chat`} onPress={() => choose()} />
        {cloudOn && cloud.result?.state === "ready" ? <SheetRow label="Cloud" onPress={() => {
          if (!canRunOnComputer) { Alert.alert("Cloud sessions accept text", "Remove attachments before choosing Cloud. Your message will stay here."); return; }
          keepCloudDraft(core, draft, onLeaveChat);
          setOpen(false);
          haptic("selection");
          router.push("/cloud-sessions/new");
        }} /> : null}
        {cloudOn && cloud.result?.state === "error" ? <SheetRow label="Retry Cloud availability" onPress={cloud.refresh} /> : null}
        {cloudOn && cloud.loading && !cloud.result ? <Text style={[styles.note, { color: theme.muted }]}>Checking Cloud availability…</Text> : null}
        {computers.computers.map((computer) => <SheetRow key={computer.computerId} label={`${computerId === computer.computerId ? "✓ " : ""}${computer.name} · ${!computer.enabled ? "Remote control off" : computers.awake(computer) ? "Awake" : "Offline"}`} onPress={() => choose(computer.computerId)} />)}
        {computers.error && !computers.needsPairing ? <Text style={[styles.note, { color: theme.muted }]}>{computers.error}</Text> : null}
        {/* Gateway vouches for the person's devices: their computers list here with no scan. */}
        {computers.gateway && !computers.needsPairing ? null : <SheetRow label={computers.gateway ? "Set up this phone again" : "Connect a computer"} onPress={() => {
          haptic("tap");
          setOpen(false);
          if (canRunOnComputer) keepRunOnContinuation(core, () => {
            transferDraft(core, draft);
            onLeaveChat?.();
          });
          router.push({ pathname: "/connect", params: { from: "run-on", ...(canRunOnComputer ? {} : { returnOnly: "1" }) } });
        }} />}
      </ScrollView>
    </Sheet>
  </>;
}
const styles = StyleSheet.create({ chip: { minHeight: 44, flexDirection: "row", alignItems: "center", gap: 7, alignSelf: "flex-start", paddingHorizontal: 16 }, label: { ...text.secondary, fontFamily: font.medium, flexShrink: 1 }, options: { maxHeight: 420 }, note: { ...text.secondary, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 8 } });
