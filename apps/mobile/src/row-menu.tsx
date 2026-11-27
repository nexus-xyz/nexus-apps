import type { ReactElement } from "react";
import { MessageMenu, type MessageAction } from "@/message-menu";

export type RowAction = MessageAction;

/**
 * A drawer row's long-press menu (brand v5's P94·C): the row lifts over the blurred drawer and its actions list under
 * it, as desktop's context menu. iOS draws its own context menu (`message-menu.tsx`), so the row needs no long press
 * of its own; Android opens a sheet from the row's long press (`row-menu.android.tsx`).
 */
export function RowMenu({ actions, children }: { title: string; actions: RowAction[]; children: (onLongPress: (() => void) | undefined) => ReactElement }) {
  return <MessageMenu actions={actions}>{children(undefined)}</MessageMenu>;
}
