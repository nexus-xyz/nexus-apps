import { Switch as NativeSwitch } from "react-native";
import { useTheme } from "@/theme";

/** What a call site may set on a switch: its colours are the theme's, never a prop. */
export type SwitchProps = { value: boolean; onValueChange?: (value: boolean) => void; disabled?: boolean; accessibilityLabel?: string };

/**
 * iOS's own switch, in the system green when on: the one place the phone keeps iOS's colour (brand v5's P21). Off, its
 * track is the boards' fill. Android draws Material 3's switch instead (`switch.android.tsx`).
 */
export function Switch(props: SwitchProps) {
  const theme = useTheme();
  return <NativeSwitch {...props} trackColor={{ false: theme.fill, true: theme.success }} ios_backgroundColor={theme.fill} thumbColor="#ffffff" />;
}
