import { parseRange } from "@nexus/kit/sheet";
import { useCallback, useEffect, useState } from "react";
import { Pressable, StyleSheet, Text } from "react-native";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import type { CellRange, SheetSelection } from "@/sheet-cell";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { font, mono, text, useTheme } from "@/theme";

export function useSheetCellNavigation() {
  const [range, setRange] = useState<CellRange>();
  const [selection, setSelection] = useState<Extract<SheetSelection, { kind: "selected" }>>();
  const [result, setResult] = useState<SheetSelection>();
  const receive = useCallback((next: SheetSelection) => {
    if (next.kind === "selected") setSelection(next);
    setResult(next);
  }, []);
  function reset() {
    setRange(undefined);
    setSelection(undefined);
    setResult(undefined);
  }
  return { range, selection, result, receive, setRange, reset };
}

export function SheetCellControl({ selection, result, onRange }: {
  selection: Extract<SheetSelection, { kind: "selected" }>;
  result: SheetSelection | undefined;
  onRange: (range: CellRange) => void;
}) {
  const theme = useTheme();
  const [open, setOpen] = useState(false);
  const [input, setInput] = useState("");
  const [invalid, setInvalid] = useState(false);
  const [submitted, setSubmitted] = useState<SheetSelection | undefined>();

  useEffect(() => {
    if (!open || result === submitted) return;
    if (result?.kind === "outside") haptic("error");
    if (result?.kind === "selected" && result.jumped) {
      haptic("selection");
      setOpen(false);
    }
  }, [open, result, submitted]);

  function change(value: string) {
    setInput(value);
    setInvalid(false);
  }

  function go() {
    const range = parseRange(input.trim().toUpperCase());
    if (!range) {
      haptic("error");
      setInvalid(true);
      return;
    }
    setSubmitted(result);
    onRange(range);
  }

  const error = invalid ? "Enter a cell or range, such as B2 or A1:D5." : result !== submitted && result?.kind === "outside"
    ? `This view ends at ${result.boundary}.` : undefined;

  return <>
    <Pressable accessibilityRole="button" accessibilityLabel={`Go to cell, ${selection.address}`}
      style={({ pressed }) => [styles.trigger, { backgroundColor: pressed ? theme.fill : theme.card, borderColor: theme.border }]}
      onPress={() => { haptic("tap"); change(selection.address); setSubmitted(result); setOpen(true); }}>
      <Text style={[styles.address, { color: theme.foreground }]} numberOfLines={1}>
        {[selection.address, selection.value].filter(Boolean).join(" · ")}
      </Text>
      <Icon name="chevron" color={theme.muted} size={14} />
    </Pressable>
    <Sheet visible={open} onClose={() => setOpen(false)} solid>
      <SheetTitle>Go to cell</SheetTitle>
      <TextInput accessibilityLabel="Cell or range" placeholder="B2 or A1:D5" placeholderTextColor={theme.muted}
        autoCapitalize="characters" autoCorrect={false} value={input} onChangeText={change} onSubmitEditing={go}
        style={[styles.field, { borderColor: theme.border, color: theme.foreground }]} />
      <Text style={[styles.hint, { color: error ? theme.danger : theme.muted }]} accessibilityLiveRegion="polite">
        {error ?? "Enter a cell or range, such as B2 or A1:D5."}
      </Text>
      <SheetRow label="Go" tone="primary" onPress={go} />
      <SheetRow label="Close" onPress={() => setOpen(false)} />
    </Sheet>
  </>;
}

const styles = StyleSheet.create({
  trigger: { minHeight: 44, marginHorizontal: 16, marginVertical: 4, paddingHorizontal: 14, gap: 8, flexDirection: "row",
    alignItems: "center", borderRadius: 12, borderWidth: StyleSheet.hairlineWidth },
  address: { flex: 1, ...text.secondary, fontFamily: mono },
  field: { minHeight: 44, marginHorizontal: 16, paddingHorizontal: 12, borderRadius: 12, borderWidth: 1, ...text.body, fontFamily: mono },
  hint: { ...text.secondary, fontFamily: font.regular, padding: 12 },
});
