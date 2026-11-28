import { askedLine, ownersLine, ownersOf, type BillingBlock } from "@nexus/kit";
import { useEffect, useState } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from "react-native";
import { useArtifacts, useCore } from "@/auth";
import { Icon } from "@/icon";
import { font, text, type Theme } from "@/theme";

type Asked = { status: "sent" | "already_asked" } | { error: string };

/**
 * "Ask an owner" on the phone, for a member who is not an owner, where only an owner fixes what stops them.
 * As brand v5's owners block draws it (`block`, on Billing): the owner rows, then a full-width primary "Ask an owner"
 * that becomes a disabled "Asked" with a check. In a notice (`compact`): the owners in a line, and the same button.
 * The words are the kit's (`ask-owner.ts`).
 */
export function AskOwner({ block, theme, layout = "compact" }: { block: BillingBlock; theme: Theme; layout?: "compact" | "block" }) {
  const core = useCore();
  const artifacts = useArtifacts();
  const [owners, setOwners] = useState<string[]>([]);
  const [asking, setAsking] = useState(false);
  const [asked, setAsked] = useState<Asked | null>(null);
  useEffect(() => {
    let live = true;
    Promise.all([core.members(), core.me()]).then(
      ([list, me]) => live && setOwners(ownersOf(list.members, me.user?.id)),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [core]);
  const ask = () => {
    if (asking) return;
    setAsking(true);
    artifacts
      .askOwners(block)
      .then(setAsked, (e: Error) => setAsked({ error: e.message }))
      .finally(() => setAsking(false));
  };
  const done = asked && "status" in asked ? asked.status : undefined;
  const failed = asked && "error" in asked ? asked.error : undefined;
  const line = ownersLine(owners);
  return (
    <View style={styles.wrap}>
      {layout === "block" && owners.length > 0 ? (
        <View style={[styles.group, { backgroundColor: theme.card }]} accessibilityLabel="Owners">
          {owners.map((name, index) => (
            <View key={name} style={[styles.ownerRow, index > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: theme.separator }]}>
              <Text style={[styles.name, { color: theme.foreground }]}>{name}</Text>
              <Text style={[styles.role, { color: theme.tertiary }]}>Owner</Text>
            </View>
          ))}
        </View>
      ) : line ? (
        <Text style={[styles.line, { color: theme.muted }]}>{line}</Text>
      ) : null}
      <Pressable
        onPress={ask}
        disabled={asking || Boolean(done)}
        accessibilityRole="button"
        accessibilityState={{ busy: asking, disabled: asking || Boolean(done) }}
        style={({ pressed }) => [styles.button, { backgroundColor: done ? theme.fill : theme.primary }, pressed && styles.pressed]}
      >
        {asking ? <ActivityIndicator size="small" color={theme.onPrimary} /> : null}
        {done ? <Icon name="check" size={14} color={theme.tertiary} /> : null}
        <Text style={[styles.label, { color: done ? theme.tertiary : theme.onPrimary }]}>{done ? "Asked" : asking ? "Asking…" : "Ask an owner"}</Text>
      </Pressable>
      {done ? (
        <Text style={[styles.line, { color: theme.tertiary }]} accessibilityLiveRegion="polite">
          {askedLine(done)}
        </Text>
      ) : null}
      {failed ? <Text style={[styles.line, { color: theme.danger }]}>Not sent: {failed}</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: { gap: 10, marginTop: 6 },
  // The boards' rows (P40·D): a radius-12 group, 48 pt rows, 16 pt in.
  group: { borderRadius: 12, borderCurve: "continuous", overflow: "hidden" },
  ownerRow: { minHeight: 48, paddingHorizontal: 16, flexDirection: "row", alignItems: "center", justifyContent: "space-between" },
  name: { ...text.base, fontFamily: font.regular },
  role: { ...text.sm, fontFamily: font.regular },
  line: { ...text.sm, fontFamily: font.regular },
  // The phone's buttons: 44 pt, radius 12, 16 pt medium text.
  button: { minHeight: 44, borderRadius: 12, borderCurve: "continuous", flexDirection: "row", alignItems: "center", justifyContent: "center", gap: 6, paddingHorizontal: 16 },
  pressed: { opacity: 0.8 },
  label: { fontSize: 16, lineHeight: 21, fontFamily: font.medium },
});
