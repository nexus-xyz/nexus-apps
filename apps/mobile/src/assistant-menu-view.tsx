import { router, type Href } from "expo-router";
import type { ComponentRef, RefObject } from "react";
import type { View } from "react-native";
import { pickRow, rowDisabled, type MenuAction, type MenuRow } from "@/assistant-menu";
import { Dropdown } from "@/dropdown";
import { Icon } from "@/icon";
import { useTheme } from "@/theme";

/**
 * The Assistant's menu (`assistant-menu.ts`), as the app's own menu (`dropdown.tsx`): it opens below the ⋯ in the
 * bar, each group a section, each row its icon. It is drawn in `container`, the screen, and is not a Modal, so it
 * never moves the keyboard and a sheet it opens (History) shows at once. The system's pull-down menu brought the
 * keyboard back up as it opened, with nothing focused, so the app draws its own.
 */
export function AssistantMenu({
  anchor,
  container,
  visible,
  rows,
  busy,
  onClose,
  onAction,
}: {
  anchor: RefObject<ComponentRef<typeof View> | null>;
  container: RefObject<ComponentRef<typeof View> | null>;
  visible: boolean;
  rows: MenuRow[][];
  busy: boolean;
  onClose: () => void;
  onAction: (action: MenuAction) => void;
}) {
  const theme = useTheme();
  const byLabel = new Map(rows.flat().map((row) => [row.label, row]));
  return (
    <Dropdown
      anchor={anchor}
      container={container}
      visible={visible}
      placement="below"
      width={250}
      sections={rows.map((group) => ({
        options: group.map((row) => ({
          value: row.label,
          label: row.label,
          disabled: rowDisabled(row, busy),
          leading: <Icon name={row.icon} size={19} color={theme.foreground} />,
        })),
      }))}
      onPick={(label) => {
        const row = byLabel.get(label);
        if (row) pickRow(row, onAction, (href) => router.push(href as Href));
      }}
      onClose={onClose}
    />
  );
}
