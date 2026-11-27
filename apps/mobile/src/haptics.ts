import * as Haptics from "expo-haptics";
import { AppState, Platform } from "react-native";
import { createHapticFeedback, type HapticEffect } from "@/haptic-feedback";
import { load, save } from "@/store";

const KEY = "pref:haptics";
let enabled = load<unknown>(KEY) !== false;

export const hapticsEnabled = () => enabled;

export function setHapticsEnabled(value: boolean) {
  enabled = value;
  save(KEY, value);
}

async function play(effect: HapticEffect) {
  if (Platform.OS === "android") {
    const patterns = {
      tap: Haptics.AndroidHaptics.Virtual_Key,
      selection: Haptics.AndroidHaptics.Segment_Tick,
      send: Haptics.AndroidHaptics.Gesture_Start,
      receive: Haptics.AndroidHaptics.Segment_Tick,
      longPress: Haptics.AndroidHaptics.Long_Press,
      success: Haptics.AndroidHaptics.Confirm,
      warning: Haptics.AndroidHaptics.Reject,
      error: Haptics.AndroidHaptics.Reject,
    } satisfies Record<HapticEffect, Haptics.AndroidHaptics>;
    await Haptics.performAndroidHapticsAsync(patterns[effect]);
    return;
  }
  switch (effect) {
    case "tap": return Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Soft);
    case "selection": return Haptics.selectionAsync();
    case "send": return Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    // A message from Nexus landing: as light as a send, so a reply feels like a text, not an alert.
    case "receive": return Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    case "longPress": return Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium);
    case "success": return Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
    case "warning": return Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning);
    case "error": return Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error);
    default: {
      const exhaustive: never = effect;
      return exhaustive;
    }
  }
}

export const haptic = createHapticFeedback({
  enabled: hapticsEnabled,
  active: () => Platform.OS !== "web" && AppState.currentState === "active",
  now: Date.now,
  play,
});
