import { confirmDeviceApprover, trustedDevices } from "@nexus/kit/device-trust";
import { thisDevice } from "@nexus/kit/pairing";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, Text, View } from "react-native";
import type { ReaderState } from "./code-sessions";
import { ConnectButton } from "./connect-controls";
import { TextInput } from "./controls";
import { haptic } from "./haptics";
import { text, useTheme } from "./theme";

export function ApproverConfirmation({ ready, onConfirmed }: { ready: Extract<ReaderState, { kind: "ready" }>; onConfirmed?: () => void }) {
  const theme = useTheme();
  const [confirmed, setConfirmed] = useState<boolean>();
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const pending = useRef<object | undefined>(undefined);
  const self = thisDevice(ready.devices, ready.reader.holder);
  const identity = { deviceId: ready.reader.deviceId, ...ready.reader.holder, holder: ready.reader.holder };
  useEffect(() => {
    let live = true;
    setBusy(false);
    trustedDevices(ready.devices, identity).then((trust) => {
      if (live) setConfirmed(!trust.me.approvedBy || trust.trusted.has(trust.me.approvedBy));
    }, (failure: unknown) => { if (live) setError(failure instanceof Error ? failure.message : String(failure)); });
    return () => {
      live = false;
      pending.current = undefined;
    };
  }, [ready]);
  const confirm = async () => {
    if (pending.current) return;
    const request = {};
    pending.current = request;
    setBusy(true);
    setError(undefined);
    try {
      await confirmDeviceApprover(ready.devices, identity, code);
      if (pending.current !== request) return;
      setConfirmed(true);
      if (onConfirmed) onConfirmed();
      else haptic("success");
    } catch (failure) {
      if (pending.current !== request) return;
      setError(failure instanceof Error ? failure.message : String(failure));
      haptic("error");
    } finally {
      if (pending.current === request) {
        pending.current = undefined;
        setBusy(false);
      }
    }
  };
  if (confirmed) return <Text style={[text.secondary, { color: theme.muted }]}>This phone's approving device is confirmed.</Text>;
  return <View style={{ gap: 16 }}>
    {!onConfirmed && <>
      <Text style={[text.body, { color: theme.foreground }]}>Confirm the device that approved this phone.</Text>
      <Text style={[text.secondary, { color: theme.muted }]}>On that device, open Settings › Devices. Type its own fingerprint here.</Text>
    </>}
    {!self?.approvedBy && confirmed === undefined ? <ActivityIndicator /> : <>
      <TextInput accessibilityLabel="Approving device's fingerprint" value={code} onChangeText={setCode} autoCapitalize="characters" autoCorrect={false} style={[text.body, { color: theme.foreground, borderColor: theme.border, borderWidth: 1, padding: 12 }]} />
      <ConnectButton label="Confirm approver" icon="check" primary busy={busy} disabled={!code.trim()} onPress={() => void confirm()} />
    </>}
    {error && <Text accessibilityRole="alert" style={[text.secondary, { color: theme.foreground }]}>{error}</Text>}
  </View>;
}
