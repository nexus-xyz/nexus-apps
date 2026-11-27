import { useEffect, useMemo, useRef, useState } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";
import type { ComputerSession, ComputerState } from "./computer-session";
import { haptic } from "./haptics";
import { changedFiles, type TerminalCommand } from "./read-only-workbench";
import { RemoteChoice, SettingsAction, SettingsNote } from "./remote-settings-ui";
import { mono, useTheme } from "./theme";

export function SessionChanges({ session, state, error }: {
  session?: ComputerSession;
  state?: ComputerState;
  error?: string;
}) {
  const requested = useRef<ComputerSession | undefined>(undefined);
  const [file, setFile] = useState(0);
  const computer = state?.computers.find((candidate) => candidate.computerId === session?.computerId);
  const reach = computer ? session?.reach(computer) : undefined;
  const supports = computer?.capabilities.includes("changes") ?? false;
  const busy = !!state?.pending || !!state?.preparing;
  const changes = state?.changes;
  const files = useMemo(() => changes ? changedFiles(changes) : [], [changes]);
  const selectedIndex = file < files.length ? file : 0;
  const selected = files[selectedIndex];

  useEffect(() => {
    if (!session || !supports || reach !== "awake" || busy || requested.current === session) return;
    requested.current = session;
    void session.readChanges();
  }, [session, supports, reach, busy]);

  const status = error ?? state?.error ??
    (reach === "off" ? "Remote control is off on the computer." :
      !computer || reach !== "awake" ? "Waiting for the computer to reconnect." :
      !supports ? "Update Nexus on the computer to view recorded changes." :
      busy ? state?.notice ?? "Checking for changes…" :
      state?.notice ?? (changes ? undefined : "Checking for changes…"));

  return <View>
    {files.map(({ path }, index) => <RemoteChoice key={index} label={path} selected={selectedIndex === index} onPress={() => setFile(index)} />)}
    {selected?.patch ? <ReadOnlyText value={selected.patch} diff /> : null}
    {changes && !files.length ? <SettingsNote>No recorded file changes yet.</SettingsNote> : null}
    {changes?.truncated ? <SettingsNote>Showing the first part of this patch. Open Nexus on the computer for the full diff.</SettingsNote> : null}
    {status ? <SettingsNote>{status}</SettingsNote> : null}
    {state?.pending?.kind === "readSessionChanges" && state.pending.state === "uncertain" ? <SettingsAction label="Stop checking" onPress={() => session?.dismiss()} /> : null}
    {!busy && reach === "awake" && supports ? <SettingsAction label="Refresh changes" onPress={() => { haptic("tap"); void session?.readChanges(); }} /> : null}
  </View>;
}

export function SessionTerminal({ commands }: { commands: readonly TerminalCommand[] }) {
  const [active, setActive] = useState<string>();
  const selected = commands.find((command) => command.id === active) ?? commands[0];
  return <View>
    <SettingsNote>Synced agent command output. View only.</SettingsNote>
    {commands.map((command) => <RemoteChoice key={command.id} label={command.command} selected={selected?.id === command.id} onPress={() => setActive(command.id)} />)}
    {selected?.result?.output ? <ReadOnlyText value={selected.result.output} failed={selected.result.failed} /> : null}
    {selected && !selected.result?.output ? <SettingsNote>{selected.result ? "This command returned no output." : "No output has synced for this command yet."}</SettingsNote> : null}
    {selected?.result?.truncated ? <SettingsNote>This output was shortened. Open Nexus on the computer to see more.</SettingsNote> : null}
    {!commands.length ? <SettingsNote>No command output has synced for this session yet.</SettingsNote> : null}
  </View>;
}

function ReadOnlyText({ value, diff, failed }: { value: string; diff?: boolean; failed?: boolean }) {
  const theme = useTheme();
  return <ScrollView horizontal nestedScrollEnabled style={[styles.code, { backgroundColor: theme.grouped }]} contentContainerStyle={styles.inner}>
    <Text selectable style={[styles.line, { color: failed ? theme.danger : theme.foreground }]}>
      {diff ? value.split("\n").map((line, i) => {
        if (line.startsWith("+") && !line.startsWith("+++ ")) return <Text key={i} style={{ color: theme.foreground, backgroundColor: theme.fill }}>{line}{"\n"}</Text>;
        if (line.startsWith("-") && !line.startsWith("--- ")) return <Text key={i} style={{ color: theme.danger }}>{line}{"\n"}</Text>;
        return <Text key={i}>{line}{"\n"}</Text>;
      }) : value}
    </Text>
  </ScrollView>;
}

const styles = StyleSheet.create({
  code: { marginHorizontal: 20, borderRadius: 12 },
  inner: { padding: 12 },
  line: { fontFamily: mono, fontSize: 13, lineHeight: 19 },
});
