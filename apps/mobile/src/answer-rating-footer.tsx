import type { AnswerRating, AnswerRatingReason, RatingAudience, ThreadAnswerRating } from "@nexus/kit";
import * as Clipboard from "expo-clipboard";
import { useState } from "react";
import { Keyboard, Pressable, ScrollView, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { haptic } from "./haptics";
import { Icon } from "./icon";
import { Field } from "./list";
import { pressedFill } from "./press";
import { RemoteChoice, SettingsAction } from "./remote-settings-ui";
import { Sheet, SheetTitle } from "./sheet";
import { text, useTheme } from "./theme";
import { useToast } from "./toast";

const REASONS: { id: AnswerRatingReason; label: string }[] = [
  { id: "wrong", label: "Wrong" },
  { id: "instructions", label: "Didn’t follow instructions" },
  { id: "slow", label: "Too slow" },
  { id: "other", label: "Other" },
];

type RatingChange = { rating: AnswerRating; reason?: AnswerRatingReason; detail?: string };

export function AnswerRatingFooter({
  answer, audience, saved, pending, failed, onRating,
}: {
  answer: string;
  audience?: RatingAudience;
  saved?: ThreadAnswerRating;
  pending?: AnswerRating;
  failed: boolean;
  onRating: (value: RatingChange) => void;
}) {
  const theme = useTheme();
  const toast = useToast();
  const { height } = useWindowDimensions();
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState<AnswerRatingReason>();
  const [detail, setDetail] = useState("");
  const selected = pending ?? saved?.rating;
  const sharing = audience === "personal" ? "Private to you" : "Shared with organization owners and admins";
  const startBad = () => {
    Keyboard.dismiss();
    haptic("selection");
    setReason(saved?.reason ?? undefined);
    setDetail(saved?.detail ?? "");
    setOpen(true);
  };
  const send = (value: RatingChange) => {
    haptic("selection");
    setOpen(false);
    onRating(value);
  };
  const copy = async () => {
    try {
      if (await Clipboard.setStringAsync(answer)) toast({ text: "Answer copied.", icon: "check", haptic: "success" });
    } catch {
      toast({ text: "Couldn’t copy the answer.", haptic: "error" });
    }
  };

  return <View style={styles.footer}>
    <View style={styles.actions}>
      <Pressable accessibilityRole="button" accessibilityLabel="Copy answer" onPress={() => void copy()}
        style={({ pressed }) => [styles.target, pressed && pressedFill(theme)]}>
        <Icon name="copy" size={18} color={theme.muted} />
      </Pressable>
      {audience ? <>
        <Pressable accessibilityRole="button" accessibilityLabel="Good answer" accessibilityHint={sharing}
          accessibilityState={{ selected: selected === "good", disabled: !!pending }} disabled={!!pending}
          onPress={() => send({ rating: selected === "good" ? "none" : "good" })}
          style={({ pressed }) => [styles.target, selected === "good" && { backgroundColor: theme.fill }, pressed && pressedFill(theme)]}>
          <Icon name={selected === "good" ? "goodFilled" : "good"} size={19} color={selected === "good" ? theme.foreground : theme.muted} />
        </Pressable>
        <Pressable accessibilityRole="button" accessibilityLabel="Bad answer" accessibilityHint={sharing}
          accessibilityState={{ selected: selected === "bad", disabled: !!pending }} disabled={!!pending} onPress={startBad}
          style={({ pressed }) => [styles.target, selected === "bad" && { backgroundColor: theme.fill }, pressed && pressedFill(theme)]}>
          <Icon name={selected === "bad" ? "badFilled" : "bad"} size={19} color={selected === "bad" ? theme.foreground : theme.muted} />
        </Pressable>
        <Text style={[styles.note, { color: theme.muted }]}>{sharing}</Text>
      </> : null}
    </View>
    {pending || failed ? <Text accessibilityLiveRegion="polite" style={[styles.note, { color: failed ? theme.danger : theme.muted }]}>
      {failed ? "Couldn’t save your rating. Try again." : "Rating waiting to sync"}
    </Text> : null}
    <Sheet visible={open && !!audience} onClose={() => setOpen(false)}>
      <SheetTitle>What went wrong?</SheetTitle>
      <ScrollView style={{ maxHeight: height * 0.65 }} keyboardShouldPersistTaps="handled">
        {REASONS.map(({ id, label }) => (
          <RemoteChoice key={id} label={label} selected={reason === id} onPress={() => setReason(reason === id ? undefined : id)} />
        ))}
        <View style={styles.field}>
          <Field multiline value={detail} onChangeText={setDetail} maxLength={1000}
            accessibilityLabel="Details, optional" placeholder="Add details (optional)" />
          <Text style={[styles.note, { color: theme.muted }]}>{sharing}</Text>
        </View>
        <SettingsAction label="Send rating" onPress={() => send({ rating: "bad", reason, detail: detail.trim() || undefined })} />
        {saved?.rating === "bad" ? <SettingsAction label="Remove rating" onPress={() => send({ rating: "none" })} /> : null}
        <SettingsAction label="Cancel" onPress={() => setOpen(false)} />
      </ScrollView>
    </Sheet>
  </View>;
}

const styles = StyleSheet.create({
  footer: { gap: 4 },
  actions: { flexDirection: "row", alignItems: "center", flexWrap: "wrap", columnGap: 4 },
  target: { minWidth: 44, minHeight: 44, borderRadius: 12, alignItems: "center", justifyContent: "center" },
  note: { ...text.caption, flexShrink: 1 },
  field: { paddingHorizontal: 16, paddingVertical: 8, gap: 8 },
});
