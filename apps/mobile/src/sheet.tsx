import { useEffect, useRef, useState } from "react";
import { KeyboardAvoidingView, Modal, Platform, Pressable, StyleSheet, useWindowDimensions, View } from "react-native";
import Reanimated, { ReduceMotion, useAnimatedStyle, useReducedMotion, useSharedValue, withTiming } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { scheduleOnRN } from "react-native-worklets";
import { useCloseOnBlur } from "@/screen-focus";
import { Glass, useGlass } from "@/glass";
import { SHEET } from "@/motion";
import { useTheme } from "@/theme";

export { SheetRow, SheetTitle } from "@/sheet-rows";

// iOS's sheet curve: in fast, out faster, and done well before share-sheet.tsx's 450 ms wait to open a system sheet.
// Never skipped: under Reduce Motion the card does not travel (`slide`), so only the backdrop's fade is left.
const IN = { duration: SHEET.in, easing: SHEET.easing, reduceMotion: ReduceMotion.Never };
const OUT = { duration: SHEET.out, easing: SHEET.easing, reduceMotion: ReduceMotion.Never };

/**
 * iOS's bottom sheet; Android draws Material 3's (`sheet.android.tsx`). One sheet changes its content step by step
 * rather than opening a second: iOS drops a modal presented while another is closing. Its card is Liquid Glass where
 * iOS draws it. It rises from the bottom over a fading backdrop, and the card itself never fades
 * (glass under an opacity below 1 does not render). `solid` draws the card in the theme's own colour instead: over a
 * page, whose colours do not follow the theme, glass takes in the page's white and turns a muddy grey in dark mode.
 */
export function Sheet({ visible, onClose, solid, children }: { visible: boolean; onClose: () => void; solid?: boolean; children: React.ReactNode }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const glass = useGlass();
  const place = { marginBottom: Math.max(insets.bottom, 12) };
  const reduce = useReducedMotion();
  const { height: screen } = useWindowDimensions();
  // Mounted while it slides away, showing what it showed open (callers clear their content as they close it).
  const [mounted, setMounted] = useState(visible);
  const shown = useRef(children);
  if (visible) shown.current = children;
  useCloseOnBlur(visible, onClose);
  const progress = useSharedValue(0);
  const height = useSharedValue(screen);
  useEffect(() => {
    if (visible) {
      setMounted(true);
      progress.value = withTiming(1, IN);
    } else {
      progress.value = withTiming(0, OUT, (finished) => {
        if (finished) scheduleOnRN(setMounted, false);
      });
    }
  }, [visible, progress]);
  const dim = useAnimatedStyle(() => ({ opacity: progress.value }));
  const slide = useAnimatedStyle(() => ({ transform: [{ translateY: reduce ? 0 : (1 - progress.value) * height.value }] }));
  return (
    <Modal visible={mounted} transparent animationType="none" onRequestClose={onClose}>
      <KeyboardAvoidingView style={styles.frame} behavior={Platform.OS === "ios" ? "padding" : undefined}>
        <Reanimated.View style={[StyleSheet.absoluteFill, styles.dim, dim]}>
          <Pressable accessibilityRole="button" style={StyleSheet.absoluteFill} onPress={onClose} accessibilityLabel="Close" />
        </Reanimated.View>
        <Reanimated.View style={slide} onLayout={(e) => (height.value = e.nativeEvent.layout.height)}>
          {glass && !solid ? (
            <Glass style={[styles.card, place]}>{shown.current}</Glass>
          ) : (
            <View style={[styles.card, { backgroundColor: theme.card }, place]}>{shown.current}</View>
          )}
        </Reanimated.View>
      </KeyboardAvoidingView>
    </Modal>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, justifyContent: "flex-end", paddingHorizontal: 12 },
  dim: { backgroundColor: "rgba(0,0,0,0.35)" },
  card: { borderRadius: 28, borderCurve: "continuous", paddingVertical: 8, overflow: "hidden" },
});
