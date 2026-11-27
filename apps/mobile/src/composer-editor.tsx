import type { ReactNode } from "react";
import { KeyboardAvoidingView, Modal, Platform, Pressable, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { draftCount } from "@/composer-text";
import { TextInput } from "@/controls";
import { Icon } from "@/icon";
import { ACCESSIBILITY_FONT_SCALE, CHROME_FONT_SCALE } from "@/text-size";
import { font, text, useTheme } from "@/theme";

/**
 * Room to write (brand v5's P65): the composer's draft in a large sheet, from ⤢ once the draft has three lines. It is
 * the same draft, so nothing is ever lost: Collapse, a swipe down (iOS) or Back (Android) all return to the composer
 * with the text as it is. Under the session's title, "Draft saved" and the count; the text 17/26 pt, 20 pt in; Return
 * makes a new line, and the bar on the keyboard (`bar`: +, the model, the mic, Send) is the only way to send. iOS shows
 * it as its page sheet, with the session behind it; Android as a full-screen dialog. `overlay` draws a sheet over it
 * (the model sheet, the + menu), which a sheet under the editor could not.
 */
export function ComposerEditor({
  visible,
  title,
  value,
  onChangeText,
  onSelectionChange,
  attachedBytes,
  onCollapse,
  bar,
  overlay,
}: {
  visible: boolean;
  title: string;
  value: string;
  onChangeText: (value: string) => void;
  onSelectionChange: (e: { nativeEvent: { selection: { start: number; end: number } } }) => void;
  /** What is attached, so the count's estimate includes it. */
  attachedBytes: number;
  onCollapse: () => void;
  bar: ReactNode;
  overlay?: ReactNode;
}) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const count = draftCount(value, attachedBytes);
  return (
    <Modal visible={visible} animationType="slide" presentationStyle="pageSheet" onRequestClose={onCollapse} statusBarTranslucent navigationBarTranslucent>
      <View style={[styles.sheet, { backgroundColor: theme.screen, paddingTop: ANDROID ? insets.top : 0 }]}>
        <KeyboardAvoidingView style={styles.fill} behavior={ANDROID ? undefined : "padding"}>
          {ANDROID ? null : <View style={[styles.grabber, { backgroundColor: theme.tertiary }]} />}
          <View style={styles.head}>
            <Pressable onPress={onCollapse} style={[styles.collapse, { backgroundColor: theme.fill }]} hitSlop={4} accessibilityRole="button" accessibilityLabel="Collapse editor">
              <Icon name="collapse" size={15} color={theme.foreground} />
            </Pressable>
            <View style={styles.heading} accessible accessibilityRole="header" accessibilityLabel={`${title}, ${value ? "Draft saved, " : ""}${count}`}>
              <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
                {title}
              </Text>
              <Text style={[styles.count, { color: theme.tertiary }]} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
                {value ? `Draft saved · ${count}` : count}
              </Text>
            </View>
            {/* The collapse button's room, so the title is centred on the sheet. */}
            <View style={styles.collapseRoom} />
          </View>
          <TextInput
            style={[styles.input, { color: theme.foreground }]}
            value={value}
            onChangeText={onChangeText}
            onSelectionChange={onSelectionChange}
            multiline
            autoFocus
            textAlignVertical="top"
            placeholder="Build anything"
            placeholderTextColor={theme.muted}
            maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}
            accessibilityLabel="Message"
          />
          {/* Clear of the home indicator while the keyboard is down; the keyboard takes its place when it is up. */}
          <View style={[styles.bar, { borderTopColor: theme.separator, paddingBottom: Math.max(insets.bottom, 8) }]}>{bar}</View>
        </KeyboardAvoidingView>
        {overlay}
      </View>
    </Modal>
  );
}

const ANDROID = Platform.OS === "android";
// The board's editor (P65·H): a 36 × 5 grabber; Collapse 36 pt in a 44 pt target; the title, and the count under it in
// 12 pt; the text 17/26, 20 pt in; the bar 52 pt on the keyboard, under a hairline.
const styles = StyleSheet.create({
  sheet: { flex: 1 },
  fill: { flex: 1 },
  grabber: { alignSelf: "center", width: 36, height: 5, borderRadius: 3, marginTop: 6, opacity: 0.5 },
  head: { flexDirection: "row", alignItems: "center", paddingHorizontal: 12, paddingTop: 6, paddingBottom: 4 },
  collapse: { width: 36, height: 36, borderRadius: 18, alignItems: "center", justifyContent: "center", margin: 4 },
  collapseRoom: { width: 44 },
  heading: { flex: 1, alignItems: "center" },
  title: { ...text.secondary, fontFamily: font.medium },
  count: { fontFamily: font.regular, fontSize: 12, lineHeight: 16, fontVariant: ["tabular-nums"] },
  input: { flex: 1, fontFamily: font.regular, fontSize: 17, lineHeight: 26, paddingHorizontal: 20, paddingTop: 12, paddingBottom: 12 },
  bar: { minHeight: 52, flexDirection: "row", alignItems: "center", paddingHorizontal: 12, paddingTop: 4, borderTopWidth: StyleSheet.hairlineWidth },
});
