import type { AnswerReportReason, ChatSessionMessage } from "@nexus/kit";
import * as Clipboard from "expo-clipboard";
import { useEffect, useMemo, useState } from "react";
import { Alert, Platform, ScrollView, Share, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { answerText } from "@/answer-text";
import { useAnswerSpeech } from "@/answer-speech";
import { useCore } from "@/auth";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { REASONS, WHAT_IS_SENT, reportBody, reportFailure } from "@/report";
import { device } from "@/reporting";
import { Sheet, SheetRow } from "@/sheet";
import { font, text, useTheme } from "@/theme";

export type AnswerMenuTarget = Pick<ChatSessionMessage, "id" | "content" | "model">;

/**
 * An answer's menu, opened from the More button under it. Report (App Review 1.2) asks why,
 * says what goes with it, and sends it to Nexus's team (`report.ts`). One sheet changes its content step by step: iOS
 * drops a modal presented while another is closing. A failed send stays in the sheet with its reason, to try again.
 */
export function AnswerSheet({ answer, onClose, onCopied, onReported, onQuote }: {
  answer: AnswerMenuTarget | null;
  onClose: () => void;
  onCopied: () => void;
  onReported: () => void;
  onQuote?: (sourceId: string, passage: string, ask: boolean) => void;
}) {
  const theme = useTheme();
  const core = useCore();
  const [step, setStep] = useState<"menu" | "select" | "report" | "quote">("menu");
  const plainText = useMemo(() => answerText(answer?.content ?? ""), [answer?.content]);
  const { height } = useWindowDimensions();
  const speech = useAnswerSpeech();
  const [passage, setPassage] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  // Each answer opens on its menu.
  useEffect(() => {
    if (!answer) return;
    setStep("menu");
    setBusy(false);
    setProblem(undefined);
  }, [answer]);

  const send = (reason: AnswerReportReason) => {
    if (!answer || busy) return;
    setBusy(true);
    setProblem(undefined);
    core
      .sendFeedback(reportBody(answer, reason, device))
      .then(() => {
        onClose();
        onReported();
      })
      .catch((error: unknown) => setProblem(reportFailure(error)))
      .finally(() => setBusy(false));
  };

  const copy = (content: string) => {
    void Clipboard.setStringAsync(content).then(onCopied).catch(() => Alert.alert("Couldn't copy this answer", "Try again."));
  };

  const quote = (content: string, ask = false) => {
    if (!answer || !onQuote || !content.trim()) return;
    onClose();
    onQuote(answer.id, content.trim(), ask);
  };

  const share = () => {
    onClose();
    // iOS cannot present its system share sheet until this sheet has finished closing.
    setTimeout(() => {
      void Share.share({ message: plainText }).catch((error: unknown) => Alert.alert("Couldn't share this answer", error instanceof Error ? error.message : "Try again."));
    }, Platform.OS === "ios" ? 450 : 0);
  };

  return (
    <Sheet visible={!!answer} onClose={onClose}>
      {answer && step === "menu" ? (
        <>
          <SheetRow
            label="Copy"
            onPress={() => {
              onClose();
              copy(plainText);
            }}
          />
          <SheetRow label="Select text" onPress={() => setStep("select")} />
          {onQuote && plainText ? <>
            <SheetRow label="Quote answer" onPress={() => quote(plainText)} />
            <SheetRow label="Quote a passage" onPress={() => { haptic("tap"); setPassage(""); setStep("quote"); }} />
          </> : null}
          <SheetRow label="Copy as Markdown" onPress={() => { onClose(); copy(answer.content); }} />
          <SheetRow label="Share response" onPress={share} />
          <SheetRow label={speech.reading === answer.id ? "Stop reading" : "Read aloud"} onPress={() => {
            onClose();
            void (speech.reading === answer.id ? speech.stop() : speech.read(answer.id, plainText))
              .catch(() => Alert.alert("Couldn't read this answer", "Try again."));
          }} />
          <SheetRow label="Report" tone="danger" onPress={() => setStep("report")} />
          <SheetRow label="Cancel" tone="muted" onPress={onClose} />
        </>
      ) : null}
      {answer && step === "select" ? (
        <View style={{ maxHeight: height * 0.8 }}>
          <View style={styles.selectionHead}>
            <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">Select text</Text>
          </View>
          <ScrollView style={styles.selectionBody} contentContainerStyle={styles.body}>
            <Text selectable selectionColor={theme.selection} style={[styles.selectionText, { color: theme.foreground }]} testID="response-selection-text">
              {plainText}
            </Text>
          </ScrollView>
          <SheetRow label="Copy all text" onPress={() => copy(plainText)} />
          <SheetRow label="Done" tone="muted" onPress={onClose} />
        </View>
      ) : null}
      {answer && step === "quote" ? (
        <View style={{ maxHeight: height * 0.75 }}>
          <View style={styles.body}>
            <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">Quote a passage</Text>
            <Text style={[styles.text, { color: theme.muted }]}>Touch and hold the text, then adjust the selection.</Text>
          </View>
          <TextInput
            multiline
            value={plainText}
            showSoftInputOnFocus={false}
            scrollEnabled
            selectTextOnFocus={false}
            onSelectionChange={({ nativeEvent: { selection } }) => setPassage(plainText.slice(selection.start, selection.end))}
            style={[styles.quoteInput, styles.selectionText, { color: theme.foreground, maxHeight: height * 0.36 }]}
            accessibilityLabel="Answer text to quote"
          />
          <SheetRow label="Quote selection" tone={passage.trim() ? "primary" : "muted"} onPress={() => quote(passage)} />
          <SheetRow label="Ask about this" tone={passage.trim() ? "normal" : "muted"} onPress={() => quote(passage, true)} />
          <SheetRow label="Cancel" tone="muted" onPress={onClose} />
        </View>
      ) : null}
      {answer && step === "report" ? (
        <>
          <View style={styles.body}>
            <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">
              Report this answer
            </Text>
            <Text style={[styles.text, { color: problem ? theme.danger : theme.muted }]} accessibilityLiveRegion="polite">
              {busy ? "Sending…" : (problem ?? `Why are you reporting it? ${WHAT_IS_SENT}`)}
            </Text>
          </View>
          {REASONS.map((reason) => (
            <SheetRow key={reason.id} label={reason.label} tone={busy ? "muted" : "normal"} onPress={() => send(reason.id)} />
          ))}
          <SheetRow label="Cancel" tone="muted" onPress={onClose} />
        </>
      ) : null}
    </Sheet>
  );
}

const styles = StyleSheet.create({
  body: { gap: 8, paddingHorizontal: 20, paddingTop: 12, paddingBottom: 8 },
  title: { ...text.lg, fontFamily: font.medium },
  text: { ...text.sm, fontFamily: font.regular },
  selectionHead: { paddingHorizontal: 20, paddingTop: 12 },
  selectionBody: { flexShrink: 1 },
  quoteInput: { minHeight: 80, flexShrink: 1, paddingHorizontal: 20, paddingVertical: 6, textAlignVertical: "top" },
  selectionText: { ...text.reading, fontFamily: font.regular },
});
