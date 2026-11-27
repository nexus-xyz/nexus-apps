import type { ReactNode } from "react";
import { StyleSheet, Text, View } from "react-native";
import { ConnectButton, ConnectHeader, ConnectMessage } from "./connect-controls";
import { useComputers } from "./computers";
import { Icon } from "./icon";
import { font, text, useTheme } from "./theme";

type Target = { kind: "computer"; id: string; deviceId: string } | { kind: "device"; id: string };

export function ConnectedComputer({ target, name, onOpen, onDone, waiting }: {
  target: Target;
  name: string;
  onOpen?: (computerId: string) => void;
  onDone?: () => void;
  waiting?: ReactNode;
}) {
  const computers = useComputers();
  const theme = useTheme();
  const computer = computers.computers.find((entry) =>
    target.kind === "computer"
      ? entry.computerId === target.id && entry.runnerDeviceId === target.deviceId
      : entry.runnerDeviceId === target.id
  );
  const ready = computer && computers.awake(computer);

  let message = computers.error ?? `Waiting for ${name} to appear in this organization.`;
  if (computer) {
    if (!computer.enabled) message = `Turn on Remote control in Settings → Devices on ${name}.`;
    else if (!ready) message = `Wake ${name} and keep Nexus open to control it.`;
  }

  return <>
    <ConnectHeader title={!computer && waiting ? "Computer approved" : "Computer connected"} icon="check" />
    {computer && <View style={[styles.computer, { backgroundColor: theme.card, borderColor: theme.border }]}>
      <View style={[styles.tile, { backgroundColor: theme.fill }]}>
        <Icon name="computer" color={theme.foreground} size={20} />
      </View>
      <View style={styles.identity}>
        <Text style={[styles.name, { color: theme.foreground }]}>{name}</Text>
        <Text style={[styles.detail, { color: theme.muted }]}>{computer.enabled ? "Paired · Phone control on" : "Paired · Phone control off"}</Text>
      </View>
    </View>}
    {!computer && waiting}
    {!ready && <ConnectMessage>{message}</ConnectMessage>}
    <View style={styles.actions}>
      {onOpen && ready && <ConnectButton label="Start a session" icon="forward" primary onPress={() => onOpen(computer.computerId)} />}
      {onDone && <ConnectButton label="Done" icon="check" primary={!onOpen || !ready} onPress={onDone} />}
    </View>
  </>;
}

const styles = StyleSheet.create({
  computer: { flexDirection: "row", alignItems: "center", gap: 12, padding: 16, borderRadius: 16, borderWidth: StyleSheet.hairlineWidth },
  tile: { width: 36, height: 36, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  identity: { flex: 1, gap: 2 },
  name: { ...text.body, fontFamily: font.medium },
  detail: { ...text.secondary, fontFamily: font.regular },
  actions: { gap: 8 },
});
