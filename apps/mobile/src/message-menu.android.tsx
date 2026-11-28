import { haptic } from "@/haptics";
import { useState, type ReactElement } from "react";
import { Pressable, type StyleProp, type ViewStyle } from "react-native";
import { Sheet, SheetRow } from "@/sheet";

export type { MessageAction } from "./message-menu";
import type { MessageAction } from "./message-menu";

/** Android's long-press menu for a message: Material 3's sheet, one row per action (iOS draws its context menu). */
export function MessageMenu({ actions, style, children }: { actions: MessageAction[]; style?: StyleProp<ViewStyle>; children: ReactElement }) {
  const [open, setOpen] = useState(false);
  if (!actions.length) return children;
  return (
    <>
      <Pressable
        style={style}
        onLongPress={() => {
          haptic("longPress");
          setOpen(true);
        }}
        accessibilityActions={[{ name: "longpress", label: actions.map((a) => a.label).join(", ") }]}
        onAccessibilityAction={(e) => e.nativeEvent.actionName === "longpress" && setOpen(true)}
      >
        {children}
      </Pressable>
      <Sheet visible={open} onClose={() => setOpen(false)}>
        {actions.map((a) => (
          <SheetRow
            key={a.key}
            label={a.label}
            tone={a.destructive ? "danger" : "normal"}
            onPress={() => {
              setOpen(false);
              a.onPress();
            }}
          />
        ))}
      </Sheet>
    </>
  );
}
