import type { CurrentTenantResponse, MemberSummary, TokenSummary } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import * as Clipboard from "expo-clipboard";
import { ActivityIndicator, Alert, Platform, Pressable, StyleSheet, Text, View } from "react-native";
import { keyLine } from "@/account";
import { useCore } from "@/auth";
import { Switch } from "@/controls";
import { Empty, Field, Loading, Page, Row, Section } from "@/list";
import { RowMenu, type RowAction } from "@/row-menu";
import { Sheet } from "@/sheet";
import { SwipeActions } from "@/swipe-actions";
import { font, mono, text, useTheme } from "@/theme";
import { useToast } from "@/toast";

const KIND: Record<TokenSummary["kind"], string> = { api: "API key", cli: "CLI sign-in", ui: "Nexus UI" };

/**
 * API keys (brand v5's P68·K): your keys, each with its start and when it stops working (owners and admins can show
 * everyone's), Create key in a sheet that shows the new key once with Copy key, and Revoke by a swipe on iPhone or a
 * long press on either phone, after asking. The organization's provider keys moved to Providers (P26·D), linked under
 * them.
 */
export default function Keys() {
  const core = useCore();
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  useEffect(() => {
    core.me().then(setMe, () => undefined);
  }, [core]);
  const [attempt, setAttempt] = useState(0);
  return (
    <Page onRefresh={() => setAttempt((n) => n + 1)}>
      <ApiKeys admin={!!me?.permissions?.includes("keys.manage_all")} attempt={attempt} />
      {/* The organization's provider keys are Providers' (P26·D), in Controls. */}
      <Section footer="Your organization's own keys with the model makers, which serve its model calls.">
        <Row label="Provider keys" href="/providers" />
      </Section>
    </Page>
  );
}

function ApiKeys({ admin, attempt }: { admin: boolean; attempt: number }) {
  const core = useCore();
  const theme = useTheme();
  const toast = useToast();
  const [everyone, setEveryone] = useState(false);
  const [tokens, setTokens] = useState<TokenSummary[] | null>(null);
  const [members, setMembers] = useState<MemberSummary[]>([]);
  const [creating, setCreating] = useState(false);
  const [label, setLabel] = useState("");
  const [created, setCreated] = useState<{ label: string; token: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);

  const load = useCallback(() => {
    core.tokens(everyone).then((r) => setTokens(r.tokens), (e: Error) => setMessage({ text: e.message, error: true }));
    if (everyone) core.members().then((r) => setMembers(r.members), () => undefined);
  }, [core, everyone]);
  useEffect(load, [load, attempt]);

  const open = () => {
    setLabel("");
    setCreated(null);
    setMessage(null);
    setCreating(true);
  };
  const create = () => {
    const name = label.trim();
    if (!name || busy) return;
    setBusy(true);
    setMessage(null);
    core
      .createToken(name)
      .then((r) => {
        setCreated({ label: name, token: r.token });
        load();
      })
      .catch((e: Error) => setMessage({ text: e.message, error: true }))
      .finally(() => setBusy(false));
  };
  const copy = async () => {
    if (!created) return;
    await Clipboard.setStringAsync(created.token);
    toast({ text: "Key copied", icon: "copy" });
  };
  const revoke = (token: TokenSummary) =>
    Alert.alert(`Revoke ${token.label ?? KIND[token.kind]}?`, token.current ? "It's this session: it signs you out here." : "Anything using it stops working now.", [
      { text: "Keep", style: "cancel" },
      {
        text: "Revoke",
        style: "destructive",
        onPress: () =>
          core.revokeToken(token.id).then(
            () => {
              toast({ text: `Revoked ${token.label ?? KIND[token.kind]}` });
              load();
            },
            (e: Error) => setMessage({ text: e.message, error: true }),
          ),
      },
    ]);
  // Short-lived Nexus UI session tokens are not worth listing.
  const rows = (tokens ?? []).filter((t) => t.kind !== "ui");
  const who = (userId: string) => members.find((m) => m.userId === userId)?.email ?? userId;

  return (
    <>
      <Section title="API keys" footer={message && !creating ? message.text : ANDROID ? "Hold a key to revoke it." : "Swipe a key to revoke it."} footerTone={message?.error && !creating ? "danger" : undefined}>
        {admin ? <Row label="Show everyone's keys" accessory={<Switch value={everyone} onValueChange={setEveryone} accessibilityLabel="Show everyone's keys" />} /> : null}
        {tokens === null && !message ? <Loading bare rows={2} /> : null}
        {tokens && rows.length === 0 ? <Empty>No API keys yet. Create one for a CI job or a script.</Empty> : null}
        {rows.map((token) => {
          const name = token.label ?? KIND[token.kind];
          const revokeAction: RowAction = { key: "revoke", label: "Revoke", icon: { ios: "trash", android: "delete" }, onPress: () => revoke(token), destructive: true };
          return (
            <RowMenu key={token.id} title={name} actions={[revokeAction]}>
              {(onLongPress) => (
                <SwipeActions actions={[{ key: "revoke", label: "Revoke", icon: "close", color: theme.danger, ink: "#ffffff", onPress: () => revoke(token) }]}>
                  {/* The card's colour on the row itself, so Revoke stays hidden under it until the swipe. */}
                  <View style={{ backgroundColor: theme.card }}>
                    <Row
                      label={name}
                      detail={[
                        keyLine(token.prefix, token.expiresAt),
                        token.kind !== "api" ? KIND[token.kind] : undefined,
                        token.current ? "this session" : undefined,
                        everyone ? who(token.userId) : undefined,
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                      onLongPress={onLongPress}
                      accessibilityActions={[{ name: "revoke", label: "Revoke" }]}
                      onAccessibilityAction={() => revoke(token)}
                    />
                  </View>
                </SwipeActions>
              )}
            </RowMenu>
          );
        })}
        <Row label="Create key" tone="primary" onPress={open} />
      </Section>
      {/* Create (P68·K): name it, then the key once, with Copy key. Nexus keeps only its start after this. */}
      <Sheet visible={creating} onClose={() => setCreating(false)}>
        {created ? (
          <View style={styles.created}>
            <Text style={[styles.sheetTitle, { color: theme.foreground }]}>{created.label}</Text>
            <Text style={[styles.sheetLabel, { color: theme.muted }]}>Your new key</Text>
            <Text style={[styles.secret, { color: theme.foreground, backgroundColor: theme.fill }]} selectable>
              {created.token}
            </Text>
            <Pressable style={({ pressed }) => [styles.button, { backgroundColor: theme.primary }, pressed && styles.pressed]} onPress={copy} accessibilityRole="button">
              <Text style={[styles.buttonText, { color: theme.onPrimary }]}>Copy key</Text>
            </Pressable>
            <Text style={[styles.sheetNote, { color: theme.tertiary }]}>Shown once. Copy it now; Nexus keeps only its start.</Text>
          </View>
        ) : (
          <View style={styles.created}>
            <Text style={[styles.sheetTitle, { color: theme.foreground }]}>New API key</Text>
            <Field
              style={[styles.field, { backgroundColor: theme.fill }]}
              value={label}
              onChangeText={setLabel}
              maxLength={80}
              placeholder="What it is for, e.g. CI deploys"
              accessibilityLabel="What the new key is for"
              autoFocus
              onSubmitEditing={create}
              returnKeyType="done"
            />
            <Pressable
              style={({ pressed }) => [styles.button, { backgroundColor: theme.primary }, (!label.trim() || busy) && styles.idle, pressed && styles.pressed]}
              onPress={create}
              disabled={!label.trim() || busy}
              accessibilityRole="button"
              accessibilityState={{ disabled: !label.trim() || busy, busy }}
            >
              {busy ? <ActivityIndicator color={theme.onPrimary} /> : <Text style={[styles.buttonText, { color: theme.onPrimary }]}>Create key</Text>}
            </Pressable>
            <Text style={[styles.sheetNote, { color: message?.error ? theme.danger : theme.tertiary }]}>{message?.error ? message.text : "Send it as Authorization: Bearer nx_…"}</Text>
          </View>
        )}
      </Sheet>
    </>
  );
}

const ANDROID = Platform.OS === "android";
// The board's sheet (P68·K): the key's name 20 pt, the key in mono on the fill, one full-width button 50 pt.
const styles = StyleSheet.create({
  created: { paddingHorizontal: 20, paddingTop: 8, paddingBottom: 8, gap: 12 },
  sheetTitle: { fontFamily: font.medium, fontSize: 20, lineHeight: 25 },
  sheetLabel: { ...text.sm, fontFamily: font.medium },
  field: { minHeight: 50, paddingHorizontal: 14, borderRadius: 12, borderCurve: "continuous" },
  secret: { fontFamily: mono, fontSize: 14, lineHeight: 20, padding: 14, borderRadius: 12, borderCurve: "continuous", overflow: "hidden" },
  button: { minHeight: 50, borderRadius: 25, alignItems: "center", justifyContent: "center" },
  buttonText: { ...text.body, fontFamily: font.medium },
  sheetNote: { ...text.caption, fontFamily: font.regular },
  idle: { opacity: 0.4 },
  pressed: { opacity: 0.7 },
});
