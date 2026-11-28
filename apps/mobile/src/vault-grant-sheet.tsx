import { vaultDay, VAULT_MANAGER_NAMES, type VaultAsk } from "@nexus/kit";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { APPROVE_ON_DESKTOP, fillerOf } from "@/vault-words";
import { Icon } from "@/icon";
import { Sheet } from "@/sheet";
import { font, text, useTheme } from "@/theme";

/**
 * The grant sheet (A8·b, A2's Face ID frame): one login, one site, one task, for this once or until the date the ask
 * offers. Approve takes Touch ID on the person's Mac, so the phone says where to approve it and offers Decline only.
 * It shows the login's name and its masked account only, as the kit read them: never a password.
 */
export function VaultGrantSheet({ ask, working, timeZone, onClose, onDecide }: { ask: VaultAsk | null; working: boolean; timeZone: string; onClose: () => void; onDecide: (ask: VaultAsk, decision: "decline") => void }) {
  const theme = useTheme();
  const rows = ask
    ? [
        ["Login", `${ask.login.name} · ${ask.login.account} (${VAULT_MANAGER_NAMES[ask.manager]})`],
        ["Only on", ask.site],
        ["To do", ask.task],
      ]
    : [];
  return (
    <Sheet visible={!!ask} onClose={onClose}>
      {ask ? (
        <View style={styles.body}>
          <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">
            Use your {ask.login.name} login?
          </Text>
          {rows.map(([label, value]) => (
            <View key={label} style={styles.row} accessible accessibilityLabel={`${label}: ${value}`}>
              <Text style={[styles.label, { color: theme.tertiary }]}>{label}</Text>
              <Text style={[styles.value, { color: theme.foreground }]}>{value}</Text>
            </View>
          ))}
          <View style={styles.row}>
            <Text style={[styles.label, { color: theme.tertiary }]}>For</Text>
            <Text style={[styles.value, { color: theme.foreground }]}>{ask.until ? `This once, or until ${vaultDay(ask.until, timeZone)}` : "This once"}</Text>
          </View>
          <View style={styles.note}>
            <Icon name="lock" size={14} color={theme.muted} />
            <Text style={[styles.noteText, { color: theme.muted }]}>{APPROVE_ON_DESKTOP} {fillerOf(ask)} fills it in. Nexus never sees it, stores it or repeats it.</Text>
          </View>
          <View style={styles.actions}>
            <Pressable
              disabled={working}
              onPress={() => onDecide(ask, "decline")}
              style={[styles.button, { borderColor: theme.border, borderWidth: StyleSheet.hairlineWidth }, working && styles.idle]}
              accessibilityRole="button"
            >
              <Text style={[styles.buttonText, { color: theme.foreground }]}>Decline</Text>
            </Pressable>
          </View>
        </View>
      ) : null}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  body: { gap: 12, paddingHorizontal: 20, paddingTop: 8, paddingBottom: 16 },
  title: { ...text.body, fontFamily: font.medium },
  row: { flexDirection: "row", gap: 12, alignItems: "center" },
  label: { ...text.sm, fontFamily: font.regular, width: 64 },
  value: { ...text.sm, fontFamily: font.regular, flex: 1 },
  note: { flexDirection: "row", gap: 8, alignItems: "flex-start" },
  noteText: { ...text.caption, fontFamily: font.regular, flex: 1 },
  actions: { flexDirection: "row", gap: 8, marginTop: 4 },
  button: { minHeight: 44, paddingHorizontal: 16, borderRadius: 22, alignItems: "center", justifyContent: "center" },
  primary: { flex: 1 },
  buttonText: { ...text.sm, fontFamily: font.medium },
  idle: { opacity: 0.4 },
});
