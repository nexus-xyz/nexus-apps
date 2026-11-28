import { createContext, useCallback, useContext, useEffect, useRef, useState, type ComponentProps, type ReactNode } from "react";
import { AccessibilityInfo, Pressable, StyleSheet, Text, View } from "react-native";
import Reanimated, { Keyframe } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { Glass, useGlass } from "@/glass";
import { Icon } from "@/icon";
import { haptic } from "@/haptics";
import type { HapticEffect } from "@/haptic-feedback";
import { DURATION, EASE, TRAVEL } from "@/motion";
import { font, text, useTheme } from "@/theme";

/** What a toast says: its words, a symbol before them, and at most one action (Undo, View, Retry). */
/** `stays`: how long it shows, when its action has a deadline of its own (a rollback's ten seconds of Undo). */
export type ToastMessage = { text: string; icon?: ComponentProps<typeof Icon>["name"]; action?: { label: string; onPress: () => void }; stays?: number; haptic?: HapticEffect | false };

const Toasts = createContext<(message: ToastMessage) => void>(() => undefined);

/** Shows a toast (brand v5's P14): `const toast = useToast(); toast({ text: "Archived", action: { label: "Undo", … } })`. */
export const useToast = () => useContext(Toasts);

// How long a toast stays: long enough to read, longer when it offers something to do.
const STAYS = 3200;
const STAYS_WITH_ACTION = 5600;

// It rises 12 pt as it comes, on the brand's curve, and sinks 8 pt, faster, as it goes. Glass may not fade (`glass.tsx`), so
// on glass it only moves; the stand-in surface fades too. Reanimated skips both under Reduce Motion.
const RISE = new Keyframe({ 0: { transform: [{ translateY: TRAVEL.rise }] }, 100: { transform: [{ translateY: 0 }], easing: EASE } }).duration(DURATION.base);
const RISE_FADED = new Keyframe({ 0: { opacity: 0, transform: [{ translateY: TRAVEL.rise }] }, 100: { opacity: 1, transform: [{ translateY: 0 }], easing: EASE } }).duration(DURATION.base);
const SINK = new Keyframe({ 0: { opacity: 1, transform: [{ translateY: 0 }] }, 100: { opacity: 0, transform: [{ translateY: TRAVEL.sink }] } }).duration(DURATION.exit);

/**
 * The app's toasts, one at a time at the foot of the screen above the home indicator: a new one takes the place of
 * the last. VoiceOver reads each as it comes. It goes by itself, or when its action is used.
 */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [shown, setShown] = useState<(ToastMessage & { id: number }) | null>(null);
  const next = useRef(0);
  const show = useCallback((message: ToastMessage) => {
    if (message.haptic !== false) haptic(message.haptic ?? "tap");
    next.current += 1;
    setShown({ ...message, id: next.current });
    AccessibilityInfo.announceForAccessibility(message.action ? `${message.text}. ${message.action.label} is available.` : message.text);
  }, []);
  useEffect(() => {
    if (!shown) return;
    const timer = setTimeout(() => setShown((now) => (now?.id === shown.id ? null : now)), shown.stays ?? (shown.action ? STAYS_WITH_ACTION : STAYS));
    return () => clearTimeout(timer);
  }, [shown]);
  return (
    <Toasts.Provider value={show}>
      {children}
      {shown ? <ToastView key={shown.id} message={shown} onDone={() => setShown(null)} /> : null}
    </Toasts.Provider>
  );
}

function ToastView({ message, onDone }: { message: ToastMessage; onDone: () => void }) {
  const theme = useTheme();
  const glass = useGlass();
  const insets = useSafeAreaInsets();
  return (
    <View pointerEvents="box-none" style={[StyleSheet.absoluteFill, styles.frame, { paddingBottom: Math.max(insets.bottom, 12) + 64 }]}>
      <Reanimated.View entering={glass ? RISE : RISE_FADED} exiting={SINK} accessibilityLiveRegion="polite">
        <Glass style={styles.toast}>
          {message.icon ? <Icon name={message.icon} size={16} color={theme.muted} /> : null}
          <Text style={[styles.text, { color: theme.foreground }]} numberOfLines={2}>
            {message.text}
          </Text>
          {message.action ? (
            <Pressable
              onPress={() => {
                haptic("tap");
                onDone();
                message.action?.onPress();
              }}
              accessibilityRole="button"
              hitSlop={10}
              style={({ pressed }) => [styles.action, pressed && styles.pressed]}
            >
              <Text style={[styles.actionText, { color: theme.foreground }]}>{message.action.label}</Text>
            </Pressable>
          ) : null}
        </Glass>
      </Reanimated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { justifyContent: "flex-end", alignItems: "center", paddingHorizontal: 16 },
  toast: { flexDirection: "row", alignItems: "center", gap: 10, minHeight: 48, paddingLeft: 16, paddingRight: 8, borderRadius: 24, borderCurve: "continuous", maxWidth: 420 },
  text: { ...text.secondary, fontFamily: font.regular, flexShrink: 1, paddingVertical: 12 },
  action: { minHeight: 36, paddingHorizontal: 12, justifyContent: "center" },
  actionText: { ...text.secondary, fontFamily: font.medium },
  pressed: { opacity: 0.5 },
});
