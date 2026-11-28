import type { ComponentProps } from "react";
import { Platform, TextInput as NativeTextInput } from "react-native";
import { useTheme } from "@/theme";

// React Native's text field in the theme's colors, and the switch (`switch.tsx`, Material 3's on Android). Left alone,
// iOS draws the switch green and the caret blue, and brand v4 puts no hue on a control (brand/v4.0/identity/color.md).
// Use these, not react-native's (controls.test.ts checks). Their colors are not props, so a call site cannot put a hue
// back.

export { Switch } from "@/switch";

/** A text field whose caret, handles and selection follow the text's color, as Nexus UI's caret does. */
export function TextInput(props: Omit<ComponentProps<typeof NativeTextInput>, "selectionColor" | "cursorColor" | "selectionHandleColor">) {
  const theme = useTheme();
  return (
    <NativeTextInput
      {...props}
      selectionColor={Platform.OS === "android" ? theme.selection : theme.foreground}
      cursorColor={theme.foreground}
      selectionHandleColor={theme.foreground}
    />
  );
}
