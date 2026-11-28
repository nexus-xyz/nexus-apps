import type { TrashedPage } from "@nexus/kit";
import { Stack, useFocusEffect } from "expo-router";
import { useCallback, useRef, useState } from "react";
import { Alert } from "react-native";
import { useArtifacts } from "@/auth";
import { haptic } from "@/haptics";
import { Blank, Loading, Page, Row, Section } from "@/list";
import { kindOf, KIND_LABEL } from "@/library";
import { useToast } from "@/toast";

const DAY = 24 * 60 * 60 * 1_000;

export default function ArtifactTrash() {
  const artifacts = useArtifacts();
  const toast = useToast();
  const [entries, setEntries] = useState<TrashedPage[]>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const working = useRef(false);
  const load = useCallback(() => {
    void artifacts.pageTrash().then(
      (pages) => { setEntries(pages); setError(undefined); },
      (error: Error) => setError(error.message),
    );
  }, [artifacts]);
  useFocusEffect(load);

  async function change(rows: TrashedPage[], restore: boolean) {
    if (working.current) return;
    working.current = true;
    setBusy(true);
    haptic("tap");
    try {
      for (const row of rows) {
        if (restore) await artifacts.setPageTrash(row.page.id, false);
        else await artifacts.purgePageTrash(row.page.id);
        setEntries((previous) => previous?.filter((entry) => entry.page.id !== row.page.id));
      }
      haptic("success");
      toast({ text: restore ? "Artifact restored" : "Artifacts deleted" });
    } catch (error) {
      haptic("error");
      toast({ text: error instanceof Error ? error.message : "Couldn't update Trash" });
    } finally {
      working.current = false;
      setBusy(false);
      load();
    }
  }

  function confirm(rows: TrashedPage[]) {
    const name = rows.length === 1 ? rows[0]!.page.title || "Untitled" : rows.length + " artifacts";
    Alert.alert("Delete " + name + " permanently?", "This removes every version and comment. It cannot be undone.", [
      { text: "Cancel", style: "cancel" },
      { text: "Delete permanently", style: "destructive", onPress: () => void change(rows, false) },
    ]);
  }

  return <Page onRefresh={load}>
    <Stack.Screen options={{ title: "Artifact Trash" }} />
    {error && <Section>
      <Blank title="Couldn't load Trash" body={error} tone="danger" />
      <Row label="Try again" onPress={load} />
    </Section>}
    {entries?.length === 0 && <Blank title="Trash is empty" body="Artifacts moved to Trash stay here for 30 days." />}
    {entries && entries.length > 0 && <>
      <Section footer="Trashed artifacts are unavailable to readers. After 30 days, their versions and comments are deleted.">
        <Row label="Empty Trash" tone="danger" onPress={busy ? undefined : () => confirm(entries)} />
      </Section>
      {entries.map((entry) => {
        const days = Math.max(1, Math.ceil((Date.parse(entry.purgeAfter) - Date.now()) / DAY));
        return <Section key={entry.page.id} title={entry.page.title || "Untitled"}
          footer={KIND_LABEL[kindOf(entry.page.format)] + " · " + days + (days === 1 ? " day left" : " days left")}>
          <Row label="Restore" onPress={busy ? undefined : () => void change([entry], true)} />
          <Row label="Delete permanently" tone="danger" onPress={busy ? undefined : () => confirm([entry])} />
        </Section>;
      })}
    </>}
    {!entries && !error && <Loading />}
  </Page>;
}
