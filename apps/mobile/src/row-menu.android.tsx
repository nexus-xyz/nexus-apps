import { haptic } from "@/haptics";
import { useState, type ReactElement } from "react";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import type { MessageAction } from "./message-menu";

export type RowAction = MessageAction;

/**
 * Android's long-press menu for a drawer row: Material 3's sheet, titled with the row's name, one row per action (iOS
 * draws its context menu, `row-menu.tsx`). The row itself takes the long press: a Pressable around it never would,
 * since the row's own Pressable answers the touch first.
 */
export function RowMenu({ title, actions, children }: { title: string; actions: RowAction[]; children: (onLongPress: (() => void) | undefined) => ReactElement }) {
  const [open, setOpen] = useState(false);
  const show = () => {
    haptic("longPress");
    setOpen(true);
  };
  return (
    <>
      {children(actions.length ? show : undefined)}
      <Sheet visible={open} onClose={() => setOpen(false)}>
        <SheetTitle>{title}</SheetTitle>
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
