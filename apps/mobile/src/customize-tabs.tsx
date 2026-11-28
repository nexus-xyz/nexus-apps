import { router, useLocalSearchParams } from "expo-router";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { haptic } from "@/haptics";
import { LargeTitle } from "@/list";
import { pressedFill, ripple } from "@/press";
import { font, text, useTheme } from "@/theme";

const tabs = [
  { path: "/integrations", label: "Connectors" },
  { path: "/skills", label: "Skills" },
  { path: "/model-catalog", label: "Models" },
  { path: "/agents", label: "Agents" },
] as const;
type Tab = (typeof tabs)[number]["path"];

export function useCustomize() {
  return useLocalSearchParams<{ customize?: string }>().customize === "1";
}

export function CustomizeTabs({ active }: { active: Tab }) {
  const theme = useTheme();
  return (
    <>
      <LargeTitle>Customize</LargeTitle>
      <View style={styles.tabs} accessibilityRole="tablist">
        {tabs.map(({ path, label }) => {
          const selected = path === active;
          return (
            <Pressable
              key={path}
              accessibilityRole="tab"
              accessibilityState={{ selected }}
              android_ripple={ripple(theme)}
              onPress={() => {
                if (selected) return;
                haptic("selection");
                router.replace({ pathname: path, params: { customize: "1" } });
              }}
              style={({ pressed }) => [styles.tab, selected && { backgroundColor: theme.fill }, pressed && pressedFill(theme)]}
            >
              <Text style={[styles.label, { color: selected ? theme.foreground : theme.muted }]}>{label}</Text>
            </Pressable>
          );
        })}
      </View>
    </>
  );
}

const styles = StyleSheet.create({
  tabs: { flexDirection: "row", flexWrap: "wrap", gap: 4 },
  tab: { minHeight: 44, justifyContent: "center", paddingHorizontal: 10, paddingVertical: 6, borderRadius: 22, overflow: "hidden" },
  label: { ...text.secondary, fontFamily: font.medium },
});
