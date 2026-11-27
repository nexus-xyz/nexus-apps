import type { CurrentTenantResponse, OrgSettings } from "@nexus/kit";
import { Stack } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { ActivityIndicator, Alert, Pressable, StyleSheet, Text, View } from "react-native";
import { useCore } from "@/auth";
import { Blank, Field, LargeTitle, Loading, Page, Row, Section } from "@/list";
import { Sheet } from "@/sheet";
import { font, text, useTheme } from "@/theme";
import { useToast } from "@/toast";

const onOff = (on: boolean | undefined) => (on ? "On" : "Off");

/**
 * Data, as brand v5's P71d·D4 draws it on the phone: the title in the list, how long chat messages are kept (owners and
 * admins change it in a sheet, P105·D), then how the organization's data is processed, as far as Gateway's settings say:
 * web search, zero-retention engines only, and whether Gateway records growth events. Left out, since Gateway reports
 * none of them: the audit log's retention, where data is stored, the training line, and Export (a computer's task, with
 * its files). Web search and growth events change on a computer.
 */
export default function Data() {
  const core = useCore();
  const [settings, setSettings] = useState<OrgSettings | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const load = useCallback(() => {
    core.orgSettings().then(
      (s) => {
        setSettings(s);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    core.me().then(setMe, () => undefined);
  }, [core]);
  useEffect(load, [load]);

  if (!settings) return error ? <Blank title="Data did not load" body={error} tone="danger" /> : <Loading alone />;
  const canManage = !!me?.permissions?.includes("org.settings");
  const engine = settings["webSearch.engine"];
  const days = settings.chatRetentionDays;
  return (
    <Page onRefresh={load}>
      <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>Data</LargeTitle>
      <Section
        title="Retention"
        footer={`${settings.defaults.chatRetentionDays} days by default. ${canManage ? "A shorter window deletes older messages at once." : "Only owners and admins can change it."}`}
      >
        <Row label="Chat messages" value={`${days} ${days === 1 ? "day" : "days"}`} chevron={canManage} onPress={canManage ? () => setEditing(true) : undefined} />
      </Section>
      {engine !== undefined || settings.growthEvents !== undefined ? (
        <Section title="Processing" footer="Change these on a computer.">
          {engine !== undefined ? <Row label="Web search" value={engine === "off" ? "Off" : "On"} /> : null}
          {settings["webSearch.requireZdr"] !== undefined ? <Row label="Zero-retention engines only" value={onOff(settings["webSearch.requireZdr"])} /> : null}
          {settings.growthEvents !== undefined ? <Row label="Growth events" detail="Recorded in this Gateway's own store" value={onOff(settings.growthEvents)} /> : null}
        </Section>
      ) : null}
      <Retention visible={editing} days={days} onClose={() => setEditing(false)} onSaved={setSettings} />
    </Page>
  );
}

/** The retention editor (P105·D: editors as sheets): 1 to 3650 days; a shorter window asks before it deletes. */
function Retention({ visible, days, onClose, onSaved }: { visible: boolean; days: number; onClose: () => void; onSaved: (s: OrgSettings) => void }) {
  const core = useCore();
  const theme = useTheme();
  const toast = useToast();
  const [value, setValue] = useState(String(days));
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    if (visible) {
      setValue(String(days));
      setProblem(null);
    }
  }, [visible, days]);
  const next = Number(value);
  const valid = /^\d+$/.test(value) && next >= 1 && next <= 3650;
  const write = () => {
    setBusy(true);
    setProblem(null);
    core
      .setOrgSettings({ chatRetentionDays: next })
      .then((s) => {
        onSaved(s);
        onClose();
        toast({ text: `Chat messages kept ${next} ${next === 1 ? "day" : "days"}` });
      }, (e: Error) => setProblem(e.message))
      .finally(() => setBusy(false));
  };
  // A shorter window deletes at once, so it asks first.
  const save = () => {
    if (!valid || busy) return;
    if (next === days) return onClose();
    if (next > days) return write();
    Alert.alert("Delete older messages?", `Chat messages older than ${next} ${next === 1 ? "day" : "days"} are deleted now, for everyone.`, [
      { text: "Cancel", style: "cancel" },
      { text: "Delete", style: "destructive", onPress: write },
    ]);
  };
  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.sheet}>
        <Text style={[styles.title, { color: theme.foreground }]}>Keep chat messages for</Text>
        <Field
          style={[styles.field, { backgroundColor: theme.fill }]}
          value={value}
          onChangeText={setValue}
          keyboardType="number-pad"
          maxLength={4}
          placeholder="Days, 1 to 3650"
          accessibilityLabel="Days to keep chat messages"
          returnKeyType="done"
          onSubmitEditing={save}
        />
        <Pressable
          style={({ pressed }) => [styles.button, { backgroundColor: theme.primary }, (!valid || busy) && styles.idle, pressed && styles.pressed]}
          onPress={save}
          disabled={!valid || busy}
          accessibilityRole="button"
          accessibilityState={{ disabled: !valid || busy, busy }}
        >
          {busy ? <ActivityIndicator color={theme.onPrimary} /> : <Text style={[styles.buttonText, { color: theme.onPrimary }]}>Save</Text>}
        </Pressable>
        <Text style={[styles.note, { color: problem ? theme.danger : theme.tertiary }]}>{problem ?? "Days, 1 to 3650. A shorter window deletes older messages at once."}</Text>
      </View>
    </Sheet>
  );
}

// As Keys' sheet (P68·K): the title 20 pt, the field on the fill, one full-width button 50 pt.
const styles = StyleSheet.create({
  sheet: { paddingHorizontal: 20, paddingVertical: 8, gap: 12 },
  title: { fontFamily: font.medium, fontSize: 20, lineHeight: 25 },
  field: { minHeight: 50, paddingHorizontal: 14, borderRadius: 12, borderCurve: "continuous" },
  button: { minHeight: 50, borderRadius: 25, alignItems: "center", justifyContent: "center" },
  buttonText: { ...text.body, fontFamily: font.medium },
  note: { ...text.caption, fontFamily: font.regular },
  idle: { opacity: 0.4 },
  pressed: { opacity: 0.7 },
});
