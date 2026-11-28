import type { E2eDevice } from "@nexus/kit";
import { deviceFingerprint, formatCode, PAIRING_TTL_MS } from "@nexus/kit/pairing";
import { useEffect, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { type ReaderState, waitText } from "@/code-sessions";
import { ConnectButton, ConnectHeader } from "@/connect-controls";
import { StatusMark } from "@/status-mark";
import { font, mono, text, useTheme } from "@/theme";
import { useNow } from "@/use-now";

type NotReady = Extract<ReaderState, { kind: "unpaired" | "pending" | "expired" | "revoked" }>;

/**
 * Code before this phone can read: pair it with a device you already trust, as Settings › Devices does on
 * the web and desktop. The phone makes its keys and registers; then it shows its 16-character code until that
 * device approves it. A computer types the code: only the desktop shows a QR code to scan. Expired and revoked phones
 * start again with new keys.
 */
export function Pairing({ state, busy, onPair, onPairAgain, gateway = false }: { state: NotReady; busy: boolean; onPair: () => void; onPairAgain: () => void; gateway?: boolean }) {
  if (state.kind === "pending") return <Waiting self={state.self} trusted={state.devices.filter((device) => device.state === "trusted")} />;
  // Gateway vouches for the person's devices: only a removed phone stops here, and no code is shown.
  const copy = gateway && state.kind === "revoked" ? GATEWAY_REVOKED : COPY[state.kind];
  return (
    <View style={styles.block}>
      <ConnectHeader title={copy.title} icon={state.kind === "unpaired" ? "computer" : state.kind === "expired" ? "clock" : "lock"}>
        {copy.body}
      </ConnectHeader>
      <ConnectButton label={copy.action} icon="keys" primary busy={busy} onPress={state.kind === "unpaired" ? onPair : onPairAgain} />
    </View>
  );
}

const COPY: Record<"unpaired" | "expired" | "revoked", { title: string; body: string; action: string }> = {
  unpaired: {
    title: "Your coding sessions, on this phone",
    body: "Pair it with a computer you already trust to read your sessions here, live. Only your devices can open them: they are encrypted end to end.",
    action: "Pair this phone",
  },
  expired: {
    title: "The code expired",
    body: "A pairing code works for 15 minutes. Show a new one, then approve it from your computer.",
    action: "Show a new code",
  },
  revoked: {
    title: "This phone was revoked",
    body: "It gets no keys for your sessions. Pair it again from a computer you trust.",
    action: "Pair again",
  },
};

const GATEWAY_REVOKED = {
  title: "This phone was removed",
  body: "It gets no keys for your sessions. Set it up again to use it with your computers.",
  action: "Set up again",
};

/** The phone waits: its code, the minutes left, and the fingerprints of the devices that may approve it. */
function Waiting({ self, trusted }: { self: E2eDevice; trusted: E2eDevice[] }) {
  const theme = useTheme();
  const now = useNow(true);
  const [pairing, setPairing] = useState<{ code: string }>();
  const [codes, setCodes] = useState<Record<string, string>>({});
  useEffect(() => {
    let live = true;
    void deviceFingerprint(self).then(({ code }) => live && setPairing({ code }));
    void Promise.all(trusted.map(async (device) => [device.id, (await deviceFingerprint(device).catch(() => ({ code: "" }))).code] as const)).then(
      (pairs) => live && setCodes(Object.fromEntries(pairs)),
    );
    return () => {
      live = false;
    };
    // The trusted devices' ids say when to read their codes again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [self, trusted.map((device) => device.id).join(",")]);
  const spoken = pairing ? pairing.code.split("").join(" ") : "";
  return (
    <View style={styles.block}>
      <ConnectHeader title="Approve this phone">
        In Nexus on your computer, open Settings › Devices and type the code below.
      </ConnectHeader>
      <Text style={[styles.code, { color: theme.foreground }]} selectable accessibilityLabel={`Pairing code ${spoken}`}>
        {pairing ? formatCode(pairing.code) : " "}
      </Text>
      <View style={styles.waiting} accessible accessibilityLiveRegion="polite">
        <StatusMark status="running" size={10} />
        <Text style={[styles.meta, { color: theme.tertiary }]}>Waiting for approval · {waitText(self.createdAt, now, PAIRING_TTL_MS)}</Text>
      </View>
      {trusted.length ? (
        <View style={[styles.compare, { borderColor: theme.border }]}>
          <Text style={[styles.compareLead, { color: theme.muted }]}>Your computer shows its own fingerprint as it approves. It must be one of these:</Text>
          {trusted.map((device) => (
            <View key={device.id} style={styles.compareRow} accessible accessibilityLabel={`${device.name}, ${(codes[device.id] ?? "").split("").join(" ")}`}>
              <Text style={[styles.compareName, { color: theme.foreground }]} numberOfLines={1}>
                {device.name}
              </Text>
              <Text style={[styles.compareCode, { color: theme.muted }]}>{codes[device.id] ? formatCode(codes[device.id]!) : ""}</Text>
            </View>
          ))}
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  block: { alignItems: "center", gap: 12 },
  // Geist Mono, four groups of four, a little tracked so the groups read apart.
  code: { fontFamily: mono, fontSize: 20, lineHeight: 28, letterSpacing: 1, fontVariant: ["tabular-nums"], marginTop: 4 },
  waiting: { flexDirection: "row", alignItems: "center", gap: 8 },
  meta: { ...text.caption, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
  compare: { alignSelf: "stretch", marginTop: 16, paddingTop: 16, borderTopWidth: StyleSheet.hairlineWidth, gap: 8 },
  compareLead: { ...text.caption, fontFamily: font.regular },
  compareRow: { flexDirection: "row", alignItems: "center", gap: 12 },
  compareName: { ...text.secondary, fontFamily: font.regular, flex: 1 },
  compareCode: { fontFamily: mono, fontSize: 13, lineHeight: 18, fontVariant: ["tabular-nums"] },
});
