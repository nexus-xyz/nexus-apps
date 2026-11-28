import { useEffect, useRef, useState, type ComponentProps } from "react";
import { useDrawerStatus } from "expo-router/drawer";
// Not React Native's: that one moves the composer in one step after the keyboard starts; this one follows it frame by frame.
// It takes the same props (`behavior`, `keyboardVerticalOffset`).
import { KeyboardAvoidingView, KeyboardController, KeyboardEvents } from "react-native-keyboard-controller";

/**
 * Whether the screen under the drawer owns the keyboard. A keyboard raised while the sidebar is open is the sidebar's (its
 * search field), so the screen behind it keeps its layout: it does not rise or squeeze its composer.
 *
 * The screen lends the keyboard once the drawer is open with no keyboard up, and takes it back once the drawer is closed
 * and the keyboard is down. A change only while the keyboard is down moves nothing. The composer's own keyboard, still up
 * when the sidebar button opens the drawer, stays the screen's until it goes down.
 */
export function useScreenOwnsKeyboard(): boolean {
  const open = useDrawerStatus() === "open";
  const opened = useRef(open);
  const [lent, setLent] = useState(false);
  useEffect(() => {
    opened.current = open;
    if (!KeyboardController.isVisible()) setLent(open);
  }, [open]);
  useEffect(() => {
    const hidden = KeyboardEvents.addListener("keyboardDidHide", () => setLent(opened.current));
    return () => hidden.remove();
  }, []);
  return !lent;
}

/**
 * A screen's keyboard-avoiding frame, still while the sidebar holds the keyboard. It pads on Android too: under
 * KeyboardProvider the app draws edge to edge, so the window never resizes and nothing else lifts the composer.
 */
export function ScreenKeyboardAvoidingView({ enabled = true, behavior = "padding", ...props }: ComponentProps<typeof KeyboardAvoidingView>) {
  const owns = useScreenOwnsKeyboard();
  return <KeyboardAvoidingView {...props} behavior={behavior} enabled={enabled && owns} />;
}
