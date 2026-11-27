import { forwardRef, type ComponentRef } from "react";
import { ActivityIndicator, Pressable, StyleSheet, View, type StyleProp, type ViewStyle } from "react-native";
import { LINE, MAX_LINES } from "./composer-text";
import { Icon } from "./icon";
import { font, text, type Theme } from "./theme";

export const INPUT_PAD = 16;
export const composerControls = StyleSheet.create({
  input: { fontFamily: font.regular, fontSize: 16, lineHeight: LINE, maxHeight: LINE * MAX_LINES + INPUT_PAD, paddingHorizontal: 10, paddingTop: 10, paddingBottom: 6 },
  toolbar: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingLeft: 6 },
  choices: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", gap: 10 },
  picker: { flexDirection: "row", alignItems: "center", gap: 4, flexShrink: 1, minHeight: 44, minWidth: 44 },
  pickerText: { ...text.sm, fontFamily: font.medium, flexShrink: 1 },
  target: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  send: { width: 34, height: 34, borderRadius: 17, alignItems: "center", justifyContent: "center" },
  idle: { opacity: 0.25 },
});

export const ComposerPicker = forwardRef<ComponentRef<typeof Pressable>, {
  label: string; hint?: string; disabled?: boolean; onPress: () => void; style?: StyleProp<ViewStyle>; children: React.ReactNode;
}>(function ComposerPicker({ label, hint, disabled, onPress, style, children }, ref) {
  return <Pressable ref={ref} accessibilityRole="button" accessibilityLabel={label} accessibilityHint={hint} accessibilityState={{ disabled }} disabled={disabled}
    onPress={onPress} style={({ pressed }) => [composerControls.picker, style, { opacity: disabled ? 0.4 : pressed ? 0.65 : 1 }]}>{children}</Pressable>;
});

export const ComposerSend = forwardRef<ComponentRef<typeof Pressable>, {
  theme: Theme; onPress: () => void; onLongPress?: () => void; disabled?: boolean; loading?: boolean; stopping?: boolean;
  label?: string; hint?: string; testID?: string; children?: React.ReactNode;
}>(function ComposerSend({ theme, onPress, onLongPress, disabled, loading, stopping, label, hint, testID, children }, ref) {
  return <Pressable ref={ref} accessibilityRole="button" accessibilityLabel={label ?? (stopping ? "Stop" : "Send")} accessibilityHint={hint}
    accessibilityState={{ disabled, busy: loading }} disabled={disabled} onPress={onPress} onLongPress={onLongPress} testID={testID} style={composerControls.target}>
    <View style={[composerControls.send, { backgroundColor: theme.primary }, disabled && !loading && composerControls.idle]}>
      {loading ? <ActivityIndicator size="small" color={theme.onPrimary} /> : children ?? <Icon name={stopping ? "stop" : "send"} size={stopping ? 13 : 16} color={theme.onPrimary} />}
    </View>
  </Pressable>;
});
