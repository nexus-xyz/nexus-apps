import { StyleSheet, Text, View } from "react-native";
import Svg, { Path } from "react-native-svg";
import { NexusMark } from "@/nexus-mark";
import { provider, providerOf } from "@/providers";
import { font, useTheme } from "@/theme";

/**
 * A model's provider mark, as Nexus UI's ModelLogo: its mark in its brand colour (OpenAI's, which has none, in the
 * foreground's), the Nexus mark for `nexus/*` (Auto), as Nexus UI and the desktop app draw it, or a neutral initial chip.
 */
export function ProviderMark({ model, size = 16 }: { model: string; size?: number }) {
  const theme = useTheme();
  const p = provider(providerOf(model));
  if (p.id === "nexus") return <NexusMark size={size} />;
  if (p.path) {
    return (
      <Svg width={size} height={size} viewBox="0 0 24 24" accessibilityElementsHidden importantForAccessibility="no">
        <Path d={p.path} fill={p.hex ? `#${p.hex}` : theme.foreground} />
      </Svg>
    );
  }
  // xAI and others as a quiet initial.
  return (
    <View
      style={[styles.chip, { width: size, height: size, borderRadius: size / 4, backgroundColor: theme.userBubble }]}
      accessibilityElementsHidden
      importantForAccessibility="no"
    >
      <Text style={[styles.initial, { fontSize: size * 0.62, color: theme.muted }]}>{p.name.charAt(0)}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  chip: { alignItems: "center", justifyContent: "center" },
  initial: { fontFamily: font.medium, includeFontPadding: false },
});
