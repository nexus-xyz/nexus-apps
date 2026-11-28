import { Switch as MaterialSwitch } from "@expo/ui/jetpack-compose";
import { semantics } from "@expo/ui/jetpack-compose/modifiers";
import { MaterialHost } from "@/material";
import type { SwitchProps } from "@/switch";
import { useTheme } from "@/theme";

/**
 * Material 3's switch in the theme's metals: a primary track when on, an outlined one when off. Off, the thumb and the
 * outline are the tertiary grey on the fill, so the switch reads on a dark card too.
 */
export function Switch({ value, onValueChange, disabled, accessibilityLabel }: SwitchProps) {
  const theme = useTheme();
  return (
    <MaterialHost matchContents>
      <MaterialSwitch
        value={value}
        onCheckedChange={onValueChange}
        enabled={!disabled}
        modifiers={accessibilityLabel ? [semantics({ contentDescription: accessibilityLabel })] : undefined}
        colors={{
          checkedTrackColor: theme.primary,
          checkedThumbColor: theme.onPrimary,
          checkedBorderColor: theme.primary,
          uncheckedTrackColor: theme.fill,
          uncheckedThumbColor: theme.tertiary,
          uncheckedBorderColor: theme.tertiary,
        }}
      />
    </MaterialHost>
  );
}
