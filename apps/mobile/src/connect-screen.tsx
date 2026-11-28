import type { PhoneInvitation } from "@nexus/kit/computer-types";
import { formatCode, PAIRING_QR_PREFIX, readPairingInput, type PairingInput } from "@nexus/kit/pairing";
import { parsePhoneInvitation } from "@nexus/kit/phone-link";
import * as Clipboard from "expo-clipboard";
import { router, useFocusEffect, useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { ActivityIndicator, AppState, KeyboardAvoidingView, Platform, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { ApproveComputer } from "@/approve-computer";
import { useAuth, useCore, useStored } from "@/auth";
import { Pairing } from "@/code-pairing";
import { useReader } from "@/code-reader";
import { ApproverConfirmation } from "@/confirm-approver";
import { ConnectButton, ConnectHeader, ConnectMessage, ConnectScanner } from "@/connect-controls";
import { ConnectedComputer } from "@/connected-computer";
import { connectionIntent, consumeRunOnContinuation, forgetConnectionIntent, watchConnectionIntent } from "@/connection-intent";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { font, text, useTheme } from "@/theme";
import { EXPIRED, usePhoneConnection } from "@/use-phone-connection";

type Method = "scan" | "link" | "manual";
type ConnectionCode = { kind: "invitation"; invitation: PhoneInvitation } | { kind: "approval"; input: PairingInput };
export default function Connect({ fromSettings = false }: { fromSettings?: boolean }) {
  const params = useLocalSearchParams<{ from?: string; returnOnly?: string; approve?: string }>();
  const from = fromSettings ? "settings" : params.from;
  const { returnOnly, approve: approval } = params;
  const core = useCore();
  const { gateway, switchOrg } = useAuth();
  const chosen = useStored<string>("choice", "run-on");
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const incoming = useSyncExternalStore(watchConnectionIntent, connectionIntent, connectionIntent);
  const [active, setActive] = useState(false);
  const [method, setMethod] = useState<Method>("scan");
  const [link, setLink] = useState("");
  const [inputError, setInputError] = useState<string>();
  const [code, setCode] = useState<ConnectionCode>();
  const invitation = code?.kind === "invitation" ? code.invitation : undefined;
  const connection = usePhoneConnection({ invitation, core, gateway, active });
  const { attempt } = connection;
  const [working, setWorking] = useState(false);
  const actionOwner = useRef(0);
  const accepted = useRef(false);
  // Gateway vouches for the person's devices: no QR code, link or code to pair. A link that still arrives
  // runs as before.
  const reader = useReader(core);
  const vouched = reader.trust === "gateway";

  useFocusEffect(useCallback(() => {
    setActive(AppState.currentState === "active");
    const listener = AppState.addEventListener("change", (state) => setActive(state === "active"));
    return () => { setActive(false); listener.remove(); };
  }, []));

  useEffect(() => {
    if (from === "run-on") return () => { consumeRunOnContinuation(core); };
  }, [core, from]);

  function show(next: Method) {
    actionOwner.current += 1;
    setWorking(false);
    setInputError(undefined);
    setMethod(next);
  }

  const accept = useCallback((raw: string) => {
    if (accepted.current) return;
    try {
      const value = raw.trim();
      let next: ConnectionCode;
      if (value.startsWith(PAIRING_QR_PREFIX)) {
        next = { kind: "approval", input: readPairingInput(value) };
      } else {
        const invitation = parsePhoneInvitation(value);
        const expected = gateway?.replace(/\/+$/, "");
        if (invitation.scope.gateway !== expected) {
          throw new Error("This computer uses a different Nexus Gateway. Connect from a phone signed in to the same Gateway.");
        }
        if (invitation.version === 1 && Date.now() >= invitation.expiresAt) throw new Error(EXPIRED);
        next = { kind: "invitation", invitation };
      }
      accepted.current = true;
      setInputError(undefined);
      setCode(next);
      setLink("");
    } catch (error) {
      setInputError(errorMessage(error));
    }
  }, [gateway]);

  useEffect(() => {
    if (fromSettings || !incoming) return;
    forgetConnectionIntent();
    accept(incoming);
  }, [fromSettings, incoming, accept]);

  // Computers' Waiting for approval row opens this screen with the device's approval text. It leads to
  // Approve computer, where the person compares the code with the computer's screen, as after a scan.
  useEffect(() => {
    if (approval) accept(approval);
  }, [approval, accept]);

  useEffect(() => {
    if (code) haptic("selection");
  }, [code]);

  useEffect(() => {
    if (inputError) haptic(inputError === EXPIRED ? "warning" : "error");
  }, [inputError]);

  const reset = () => {
    connection.reset();
    setLink("");
    accepted.current = false;
    if (!fromSettings) forgetConnectionIntent();
    setCode(undefined);
    show("scan");
  };
  const close = () => {
    reset();
    if (router.canGoBack()) router.back();
    else router.replace(from === "settings" ? "/computers" : "/session/new");
  };
  const perform = async <T,>(action: () => Promise<T>, onSuccess?: (value: T) => void) => {
    if (working) return;
    const owner = ++actionOwner.current;
    setWorking(true);
    setInputError(undefined);
    try {
      const value = await action();
      if (actionOwner.current === owner) onSuccess?.(value);
    } catch (error) {
      if (actionOwner.current === owner) setInputError(errorMessage(error));
    } finally {
      if (actionOwner.current === owner) setWorking(false);
    }
  };
  const open = (computerId: string) => {
    if (from === "run-on") consumeRunOnContinuation(core)?.();
    chosen.save(computerId);
    reset();
    router.replace({ pathname: "/session/[id]", params: { id: "new", computer: computerId } });
  };

  const scanError = !code && method === "scan" ? inputError : undefined;
  const scanAgain = <ConnectButton label="Scan a new QR code" icon="scan" primary onPress={reset} />;
  const pairWithCode = <ConnectButton label="Pair with a code" icon="keys" onPress={() => {
    reset();
    show("manual");
  }} />;

  return <KeyboardAvoidingView style={[styles.screen, { backgroundColor: theme.screen }]} behavior={Platform.OS === "ios" ? "padding" : undefined}>
    {!fromSettings && <View style={[styles.bar, { paddingTop: insets.top + 4 }]}>
      <Pressable accessibilityRole="button" accessibilityLabel="Back" onPress={() => { haptic("tap"); close(); }}
        style={({ pressed }) => [styles.back, { backgroundColor: pressed ? theme.fill : theme.card, borderColor: theme.border }]}>
        <Icon name="back" size={20} color={theme.foreground} />
      </Pressable>
    </View>}
    <ScrollView keyboardShouldPersistTaps="handled"
      contentContainerStyle={[styles.scroll, { paddingBottom: insets.bottom + 12 }]}>
      <View style={styles.content}>
        {vouched && !code && <PhoneSetUp reader={reader} onDone={close} />}
        {!vouched && !code && method === "scan" && (scanError ? <>
          <ConnectHeader title={scanError === EXPIRED ? "The code expired" : "Couldn’t connect"}
            icon={scanError === EXPIRED ? "clock" : "link"}>{scanError}</ConnectHeader>
          <View style={styles.actions}>
            {scanAgain}
            {pairWithCode}
            <ConnectButton label="Use a connection link" icon="link" onPress={() => show("link")} />
          </View>
        </> : <>
          <ConnectHeader title="Connect a computer">
            In Nexus on your computer, open Settings → Devices and show a connection code. Scan it here.
          </ConnectHeader>
          <View style={styles.actions}>
            <ConnectScanner active={active} onScan={accept} />
            <ConnectButton label="Use a connection link" icon="link" onPress={() => show("link")} />
          </View>
        </>)}
        {!vouched && !code && method === "link" && <>
          <ConnectHeader title="Use a connection link" icon="link">
            Copy the connection link under the QR code on your computer.
          </ConnectHeader>
          <View style={styles.actions}>
            <TextInput accessibilityLabel="Connection link" value={link} onChangeText={setLink} autoCapitalize="none" autoCorrect={false}
              placeholder="Connection link" placeholderTextColor={theme.muted} onSubmitEditing={() => accept(link)}
              style={[styles.field, { color: theme.foreground, borderColor: theme.border, backgroundColor: theme.card }]} />
            <ConnectButton label="Paste from clipboard" icon="paste" onPress={() => { void perform(async () => {
              const value = await Clipboard.getStringAsync();
              if (!value.trim()) throw new Error("The clipboard is empty. Copy the link on your computer first.");
              return value;
            }, (value) => {
              setLink(value);
              accept(value);
            }); }} busy={working} />
          </View>
          {inputError && <ConnectMessage error>{inputError}</ConnectMessage>}
          <View style={styles.actions}>
            <ConnectButton label="Connect" icon="forward" primary disabled={!link.trim() || working} onPress={() => accept(link)} />
            <ConnectButton label="Scan QR instead" icon="scan" onPress={() => show("scan")} />
            <ConnectButton label="Pair with a code" icon="keys" onPress={() => show("manual")} />
          </View>
        </>}
        {!vouched && !code && method === "manual" && <>
          <ManualPairing />
          <ConnectButton label="Scan QR instead" icon="scan" onPress={() => show("scan")} />
        </>}

        {code?.kind === "approval" && <ApproveComputer core={core} input={code.input} onScan={reset} onOpen={returnOnly === "1" ? undefined : open} onDone={close} />}
        {invitation && attempt.kind === "connecting" && <>
          <ConnectHeader title="Connecting…" icon="link">Sending a request to {invitation.computerName}.</ConnectHeader>
          <ActivityIndicator color={theme.foreground} />
        </>}
        {invitation && attempt.kind === "approval" && <>
          <ConnectHeader title="Approve on your computer" icon="computer">
            Choose {invitation.version === 2 ? "Approve device" : "Approve phone"} on {invitation.computerName}.
          </ConnectHeader>
          {invitation.version === 1 && <ConnectMessage>Connection code · {formatCode(attempt.code)}</ConnectMessage>}
          {attempt.reconnecting ? <ConnectMessage>Connection interrupted. Retrying…</ConnectMessage> :
            <ActivityIndicator accessibilityLabel="Waiting for approval" color={theme.muted} />}
        </>}
        {invitation && attempt.kind === "confirm" && <>
          <ConnectHeader title="Confirm the approving device" icon="shield">
            Open Settings → Devices → This device on {attempt.approver ?? "the device that approved this phone"} and enter its fingerprint here.
          </ConnectHeader>
          <ApproverConfirmation ready={attempt.ready} onConfirmed={connection.confirm} />
        </>}
        {invitation && attempt.kind === "connected" && <ConnectedComputer
          target={{ kind: "computer", id: attempt.computerId, deviceId: invitation.inviterDeviceId }}
          name={invitation.computerName}
          onOpen={returnOnly === "1" ? undefined : open}
          onDone={from ? close : undefined}
        />}
        {invitation && attempt.kind === "failed" && <>
          <ConnectHeader title={attempt.message === EXPIRED ? "The code expired" : "Couldn’t connect"}
            icon={attempt.message === EXPIRED ? "clock" : attempt.organization ? "computer" : "link"}>{attempt.message}</ConnectHeader>
          {inputError && <ConnectMessage error>{inputError}</ConnectMessage>}
          <View style={styles.actions}>
            {attempt.organization ? <ConnectButton label="Switch organization and connect" icon="forward" primary busy={working} onPress={() => {
              const org = attempt.organization;
              if (org) void perform(() => switchOrg(org), connection.retry);
            }} /> : attempt.retryable && <ConnectButton label="Try again" icon="restore" primary onPress={connection.retry} />}
            <ConnectButton label="Scan a new QR code" icon="scan" primary={!attempt.retryable && !attempt.organization} onPress={reset} />
            {!attempt.retryable && !attempt.organization && pairWithCode}
          </View>
        </>}
      </View>
    </ScrollView>
  </KeyboardAvoidingView>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * This phone when Gateway vouches for the person's devices: it set itself up at sign-in, and every computer
 * signed in to the account shows in Computers. Only a removed phone has something to do.
 */
function PhoneSetUp({ reader, onDone }: { reader: ReturnType<typeof useReader>; onDone: () => void }) {
  const { state } = reader;
  if (state.kind === "ready") return <>
    <ConnectHeader title="This phone is set up" icon="computer">
      Your computers signed in to this account show in Computers. There is nothing to scan or approve.
    </ConnectHeader>
    <ConnectButton label="Done" icon="forward" primary onPress={onDone} />
  </>;
  if (state.kind === "error") return <>
    <ConnectHeader title="Couldn’t set up this phone" icon="link">{state.message}</ConnectHeader>
    <ConnectButton label="Try again" icon="restore" primary onPress={reader.refresh} />
  </>;
  if (state.kind === "revoked") return <Pairing state={state} busy={reader.busy} onPair={() => void reader.pair()} onPairAgain={() => void reader.pairAgain()} gateway />;
  return <>
    <ConnectHeader title="Setting up this phone…" icon="computer" />
    <ActivityIndicator />
  </>;
}

function ManualPairing() {
  const reader = useReader(useCore());
  const { state } = reader;
  if (state.kind === "ready") return <>
    <ConnectHeader title="Confirm the approving device" icon="shield" />
    <ApproverConfirmation ready={state} />
    <ConnectMessage>To start sessions from this phone, turn on Remote control in Settings → Devices on the computer.</ConnectMessage>
  </>;
  if (state.kind === "error") return <>
    <ConnectHeader title="Couldn’t pair this phone" icon="link">{state.message}</ConnectHeader>
    <ConnectButton label="Try again" icon="restore" primary onPress={reader.refresh} />
  </>;
  if (state.kind === "loading") return <>
    <ConnectHeader title="Checking this phone…" icon="computer" />
    <ActivityIndicator />
  </>;
  return <Pairing state={state} busy={reader.busy} onPair={() => void reader.pair()} onPairAgain={() => void reader.pairAgain()} />;
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  bar: { alignSelf: "center", width: "100%", maxWidth: 420, paddingHorizontal: 12 },
  back: { alignSelf: "flex-start", width: 48, height: 48, borderRadius: 24, borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center" },
  scroll: { flexGrow: 1, justifyContent: "center", alignItems: "center", paddingHorizontal: 20, paddingTop: 12 },
  content: { width: "100%", maxWidth: 380, gap: 20 },
  actions: { gap: 8 },
  field: { ...text.body, fontFamily: font.regular, minHeight: 48, paddingHorizontal: 16, paddingVertical: 12, borderWidth: 1, borderRadius: 12 },
});
