import { useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { sharePageExport } from "@/artifact-export";
import { ArtifactExportSheet } from "@/artifact-export-sheet";
import { useArtifacts } from "@/auth";
import { Switch } from "@/controls";
import type { DeckInfo } from "@/deck-export";
import { haptic } from "@/haptics";
import { font, text, useTheme } from "@/theme";

export function DeckExportSheet({ pageId, version, deck, visible, onClose }: {
  pageId: string;
  version?: number;
  deck: DeckInfo;
  visible: boolean;
  onClose: () => void;
}) {
  const artifacts = useArtifacts();
  const theme = useTheme();
  const [notes, setNotes] = useState(true);
  const description = [
    deck.visibleSlides === 1 ? "1 slide" : deck.visibleSlides + " slides",
    deck.hiddenSlides ? deck.hiddenSlides + " hidden" : "",
  ].filter(Boolean).join(" · ");
  return <ArtifactExportSheet
    title={version ? "Export v" + version : "Export Slides"}
    visible={visible}
    onClose={onClose}
    extra={<>
      <Text style={[styles.detail, { color: theme.muted }]}>{description}</Text>
      {deck.visibleSlides > 0 && <View style={styles.notes}>
        <Text style={[styles.label, { color: theme.foreground }]}>Speaker notes</Text>
        <Switch accessibilityLabel="Include speaker notes" value={notes}
          onValueChange={(value) => { haptic("selection"); setNotes(value); }} />
      </View>}
    </>}
    options={deck.visibleSlides > 0 ? [
      { label: "PDF", run: (signal) => sharePageExport(artifacts, pageId, "pdf", { version, notes }, signal) },
      { label: "PowerPoint (.pptx)", run: (signal) => sharePageExport(artifacts, pageId, "pptx", { version, notes }, signal) },
    ] : []}
  />;
}

const styles = StyleSheet.create({
  detail: { ...text.secondary, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 8 },
  notes: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", minHeight: 50, paddingHorizontal: 20 },
  label: { ...text.base, fontFamily: font.medium },
});
