import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, StyleSheet, Text } from "react-native";
import { haptic } from "@/haptics";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { font, text, useTheme } from "@/theme";

export type ExportOption = { label: string; run: (signal: AbortSignal) => Promise<void> };

export function ArtifactExportSheet({ title, visible, options, extra, onClose }: {
  title: string;
  visible: boolean;
  options: readonly ExportOption[];
  extra?: React.ReactNode;
  onClose: () => void;
}) {
  const theme = useTheme();
  const current = useRef<AbortController>(undefined);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<{ message: string; option: ExportOption }>();

  useEffect(() => () => current.current?.abort(), []);
  function close() {
    current.current?.abort();
    current.current = undefined;
    setBusy(false);
    setFailure(undefined);
    onClose();
  }

  async function run(option: ExportOption) {
    if (current.current) return;
    haptic("tap");
    const controller = new AbortController();
    current.current = controller;
    setBusy(true);
    setFailure(undefined);
    try {
      await option.run(controller.signal);
      if (!controller.signal.aborted) {
        haptic("success");
        close();
      }
    } catch (error) {
      if (!controller.signal.aborted) {
        haptic("error");
        setFailure({ option, message: error instanceof Error ? error.message : "Try the export again." });
      }
    } finally {
      if (current.current === controller) {
        current.current = undefined;
        setBusy(false);
      }
    }
  }

  return (
    <Sheet visible={visible} onClose={close} solid>
      <SheetTitle>{title}</SheetTitle>
      {busy ? <>
        <ActivityIndicator color={theme.muted} />
        <Text style={[styles.note, { color: theme.muted }]} accessibilityLiveRegion="polite">Preparing your file…</Text>
      </> : failure ? <>
        <Text style={[styles.note, { color: theme.danger }]} accessibilityLiveRegion="polite">{failure.message}</Text>
        <SheetRow label="Try again" tone="primary" onPress={() => void run(failure.option)} />
        {(options.length > 1 || extra) && <SheetRow label="Back to export" onPress={() => setFailure(undefined)} />}
      </> : <>
        {extra}
        {options.map((option) => <SheetRow key={option.label} label={option.label} onPress={() => void run(option)} />)}
      </>}
      <SheetRow label="Cancel" onPress={close} />
    </Sheet>
  );
}

const styles = StyleSheet.create({
  note: { ...text.secondary, fontFamily: font.regular, textAlign: "center", padding: 12 },
});
