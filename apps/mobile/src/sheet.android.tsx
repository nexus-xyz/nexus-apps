import { ModalBottomSheet, RNHostView, type ModalBottomSheetRef } from "@expo/ui/jetpack-compose";
import { useEffect, useRef, useState } from "react";
import { StyleSheet, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useCloseOnBlur } from "@/screen-focus";
import { MaterialHost } from "@/material";
import { useTheme } from "@/theme";

export { SheetRow, SheetTitle } from "@/sheet-rows";

/**
 * Material 3's modal bottom sheet, the same API as iOS's (`sheet.tsx`): a drag handle, a swipe or the back button to
 * close it, and a scrim that dims the screen. One sheet changes its content step by step rather than opening a second.
 * Closed, it slides away showing what it showed open (callers clear their content as they close it).
 */
/** `solid` is iOS's (`sheet.tsx`): Material's sheet is always solid. */
export function Sheet({ visible, onClose, children }: { visible: boolean; onClose: () => void; solid?: boolean; children: React.ReactNode }) {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const sheet = useRef<ModalBottomSheetRef>(null);
  useCloseOnBlur(visible, onClose);
  const [mounted, setMounted] = useState(visible);
  const shown = useRef(children);
  if (visible) shown.current = children;
  useEffect(() => {
    if (visible) setMounted(true);
    // Unmounted once the slide has finished; a sheet the person swiped away has nothing left to slide.
    else if (sheet.current) void sheet.current.hide().finally(() => setMounted(false));
    else setMounted(false);
  }, [visible]);
  if (!mounted) return null;
  return (
    <MaterialHost matchContents style={styles.host}>
      <ModalBottomSheet
        ref={sheet}
        onDismissRequest={onClose}
        skipPartiallyExpanded
        containerColor={theme.card}
        contentColor={theme.foreground}
        scrimColor="rgba(0,0,0,0.32)"
      >
        <RNHostView matchContents={{ vertical: true }}>
          <View style={{ paddingBottom: insets.bottom + 8 }}>{shown.current}</View>
        </RNHostView>
      </ModalBottomSheet>
    </MaterialHost>
  );
}

// The sheet has its own window, so the host takes no room in the screen's layout. It must not be sized to zero: Compose
// does not compose a zero-sized host, and the sheet never opens.
const styles = StyleSheet.create({ host: { position: "absolute" } });
