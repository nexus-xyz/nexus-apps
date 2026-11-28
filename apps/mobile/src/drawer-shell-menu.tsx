import type { ComponentProps } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { Icon } from "@/icon";
import { pressedFill, ripple } from "@/press";
import { font, text, useTheme } from "@/theme";

export function DrawerMenuRow({ icon, label, detail, destructive, disabled, onPress }: {
  icon: ComponentProps<typeof Icon>["name"];
  label: string;
  detail?: string;
  destructive?: boolean;
  disabled?: boolean;
  onPress: () => void;
}) {
  const theme = useTheme();
  const ink = disabled ? theme.muted : destructive ? theme.danger : theme.foreground;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={detail ? `${label}, ${detail}` : label}
      android_ripple={ripple(theme)}
      onPress={onPress}
      disabled={disabled}
      accessibilityState={{ disabled }}
      style={({ pressed }) => [styles.row, pressed && pressedFill(theme)]}
    >
      <Icon name={icon} size={20} color={ink} />
      <View style={styles.copy}>
        <Text style={[styles.label, { color: ink }]}>{label}</Text>
        {detail ? <Text style={[styles.detail, { color: theme.muted }]}>{detail}</Text> : null}
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  row: { minHeight: 56, flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 20, paddingVertical: 8 },
  copy: { flex: 1, gap: 2 },
  label: { ...text.base, fontFamily: font.medium },
  detail: { ...text.caption, fontFamily: font.regular },
});
