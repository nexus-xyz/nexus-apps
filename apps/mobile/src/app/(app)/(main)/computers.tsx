import { router } from "expo-router";
import { ActivityIndicator, StyleSheet, View } from "react-native";
import { ConnectButton, ConnectHeader } from "@/connect-controls";
import { useComputers } from "@/computers";
import { Footnote, Page, Row, Section } from "@/list";
import { useTheme } from "@/theme";

export default function Computers() {
  const computers = useComputers();
  const theme = useTheme();
  const loading = computers.at === 0 && !computers.error && !computers.needsPairing;
  const empty = !loading && !computers.computers.length && !computers.pending.length && (!computers.error || computers.needsPairing);
  // Gateway vouches for the person's devices: computers show up with no scan. Only a removed phone acts.
  const connect = !computers.gateway
    ? <ConnectButton label="Connect a computer" icon="scan" primary onPress={() => router.push("/computers/connect")} />
    : computers.needsPairing ? <ConnectButton label="Set up this phone again" icon="keys" primary onPress={() => router.push("/computers/connect")} /> : null;
  // A tap opens Approve computer, where the person compares the code with the computer's screen.
  const waiting = computers.pending.length ? (
    <Section title="Waiting for approval" footer="Compare the code with the one on the computer, then approve it.">
      {computers.pending.map((device) => (
        <Row
          key={device.id}
          label={device.name}
          icon="computer"
          value="Approve"
          href={{ pathname: "/computers/connect", params: { approve: device.approval } }}
        />
      ))}
    </Section>
  ) : null;

  return (
    <Page onRefresh={() => void computers.refresh()}>
      {waiting}
      {empty ? <View style={styles.empty}>
        {computers.gateway ? <ConnectHeader title="No computer yet" icon="computer">
          {computers.needsPairing ? computers.error : "Sign in to Nexus on your computer with this account. It shows here, ready to follow and steer its sessions."}
        </ConnectHeader> : <ConnectHeader title="No computer paired" icon="computer">
          Connect Nexus on your computer to follow and steer its sessions from here.
        </ConnectHeader>}
        {connect}
      </View> : <>
        {loading || computers.computers.length ? <Section title="Your computers" footer="Turn on Phone control on a computer to start sessions, send messages and answer approvals. The computer must be awake and online.">
          {loading ? <Row label="Loading computers…" accessory={<ActivityIndicator color={theme.muted} />} /> : null}
          {computers.computers.map((computer) => (
            <Row
              key={computer.computerId}
              label={computer.name}
              icon="computer"
              value={!computer.enabled ? "Phone control off" : computers.awake(computer) ? "Awake" : "Offline"}
              detail="Open a new coding session in a saved project."
              href={{ pathname: "/session/[id]", params: { id: "new", computer: computer.computerId } }}
            />
          ))}
        </Section> : null}
        {computers.error && !computers.needsPairing ? <Footnote>{computers.error}</Footnote> : null}
        {connect}
      </>}
      <Footnote>Your files and coding tools stay on the computer. Your phone reads synced history and sends encrypted instructions.</Footnote>
    </Page>
  );
}

const styles = StyleSheet.create({
  empty: { minHeight: 420, justifyContent: "center", gap: 20, width: "100%", maxWidth: 380, alignSelf: "center" },
});
