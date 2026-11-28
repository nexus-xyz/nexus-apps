import { useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { sharePageExport } from "@/artifact-export";
import { ArtifactExportSheet } from "@/artifact-export-sheet";
import { useArtifacts } from "@/auth";
import type { DesignFocus, DesignOutline } from "@/design-outline";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { font, text, useTheme } from "@/theme";

export function DesignControls({ outline, setFocus, pageId, version }: {
  outline: DesignOutline;
  setFocus: (target: DesignFocus) => void;
  pageId?: string;
  version?: number;
}) {
  const theme = useTheme();
  const artifacts = useArtifacts();
  const [show, setShow] = useState<"pages" | "layers" | "export">();
  const [chosen, setChosen] = useState<DesignFocus>();
  const page = outline.pages[outline.active];
  const index = chosen?.page === outline.active ? chosen.board ?? 0 : 0;
  const board = page?.boards[index];
  const close = () => setShow(undefined);
  const focus = (target: DesignFocus) => {
    haptic("selection");
    setChosen(target);
    setFocus(target);
    close();
  };
  if (!page) return null;
  const caption = page.name + " · " + page.boards.length + (page.boards.length === 1 ? " board" : " boards");
  return <>
    <View style={styles.toolbar}>
      <Pressable accessibilityRole="button" accessibilityLabel="Design pages" style={[styles.button, { backgroundColor: theme.fill }]}
        onPress={() => { haptic("tap"); setShow("pages"); }}>
        <Text numberOfLines={1} style={[styles.label, { color: theme.foreground }]}>{caption}</Text>
        <Icon name="chevron" color={theme.muted} size={14} />
      </Pressable>
      {pageId && <Pressable accessibilityRole="button" style={[styles.button, { backgroundColor: theme.fill }]}
        onPress={() => { haptic("tap"); setShow("export"); }}>
        <Text style={[styles.label, { color: theme.foreground }]}>Export</Text>
      </Pressable>}
    </View>
    <Sheet visible={show === "pages" || show === "layers"} onClose={close} solid>
      <SheetTitle>{show === "layers" ? "Layers" : "Design pages"}</SheetTitle>
      <ScrollView style={styles.list}>
        {show === "layers" ? page.boards.map((entry, b) => (
          <View key={b}>
            <SheetRow label={entry.name} tone={b === index ? "primary" : "normal"}
              onPress={() => focus({ page: outline.active, board: b })} />
            {entry.layers.map((layer, l) => (
              <Pressable key={l} accessibilityRole="button" style={[styles.layer, { paddingLeft: 36 + Math.min(layer.depth, 4) * 12 }]}
                onPress={() => focus({ page: outline.active, board: b, layer: l })}>
                <Text style={[styles.label, { color: theme.foreground }]}>{layer.name}</Text>
              </Pressable>
            ))}
          </View>
        )) : outline.pages.map((entry, i) => (
          <SheetRow key={i} label={entry.name + " · " + entry.boards.length + (entry.boards.length === 1 ? " board" : " boards")}
            tone={i === outline.active ? "primary" : "normal"} onPress={() => focus({ page: i })} />
        ))}
      </ScrollView>
      {show === "pages" && page.boards.length > 0 &&
        <SheetRow label="Layers" onPress={() => { haptic("tap"); setShow("layers"); }} />}
      <SheetRow label="Close" onPress={close} />
    </Sheet>
    {pageId && <ArtifactExportSheet title={board ? "Export " + board.name : "Export Design"}
      visible={show === "export"} onClose={close} options={[
        ...(board ? [
          { label: "PNG at 1×", run: (signal: AbortSignal) => sharePageExport(artifacts, pageId, "png", { version, board: board.selector, scale: 1 }, signal) },
          { label: "PNG at 2×", run: (signal: AbortSignal) => sharePageExport(artifacts, pageId, "png", { version, board: board.selector, scale: 2 }, signal) },
          { label: "SVG", run: (signal: AbortSignal) => sharePageExport(artifacts, pageId, "svg", { version, board: board.selector }, signal) },
        ] : []),
        { label: "Every board as PDF", run: (signal) => sharePageExport(artifacts, pageId, "pdf", { version }, signal) },
      ]} />}
  </>;
}

const styles = StyleSheet.create({
  toolbar: { flexDirection: "row", gap: 8, marginHorizontal: 16, marginVertical: 4 },
  button: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 44, paddingHorizontal: 12, borderRadius: 12 },
  label: { ...text.secondary, fontFamily: font.medium, flexShrink: 1 },
  list: { maxHeight: 360 },
  layer: { minHeight: 44, justifyContent: "center", paddingRight: 20 },
});
