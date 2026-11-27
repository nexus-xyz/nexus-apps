import type { SiteFrameRoute } from "@nexus/kit";
import { useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Sheet } from "@/sheet";
import { SheetRow, SheetTitle } from "@/sheet-rows";
import { font, mono, text, useTheme } from "@/theme";

export function SiteRoutePicker({ routes, selected, onPick }: {
  routes: readonly SiteFrameRoute[];
  selected: SiteFrameRoute;
  onPick: (route: SiteFrameRoute) => void;
}) {
  const theme = useTheme();
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);

  return (
    <>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Pages, ${selected.title}, ${selected.path}`}
        accessibilityState={{ expanded: open }}
        onPress={() => { haptic("tap"); setOpen(true); }}
        style={({ pressed }) => [styles.pill, { backgroundColor: pressed ? theme.fill : theme.screen, borderColor: theme.border }]}
      >
        <Text numberOfLines={1} style={[styles.title, { color: theme.foreground }]}>{selected.title}</Text>
        <Text numberOfLines={1} style={[styles.path, { color: theme.muted }]}>{selected.path}</Text>
        <Icon name="chevron" size={14} color={theme.muted} />
      </Pressable>
      <Sheet visible={open} onClose={close} solid>
        <SheetTitle>Pages</SheetTitle>
        <ScrollView style={styles.routes}>
          {routes.map((route) => {
            const active = route.path === selected.path;
            return (
              <Pressable
                key={route.path}
                accessibilityRole="button"
                accessibilityState={{ selected: active }}
                accessibilityLabel={`${route.title}, ${route.path}`}
                onPress={() => { close(); onPick(route); }}
                style={({ pressed }) => [styles.row, (active || pressed) && { backgroundColor: theme.fill }]}
              >
                <View style={styles.rowCopy}>
                  <Text style={[styles.title, { color: theme.foreground }]}>{route.title}</Text>
                  <Text style={[styles.path, { color: theme.muted }]}>{route.path}</Text>
                </View>
                {active && <Icon name="check" color={theme.primary} size={18} />}
              </Pressable>
            );
          })}
        </ScrollView>
        <SheetRow label="Close" onPress={close} />
      </Sheet>
    </>
  );
}

const styles = StyleSheet.create({
  pill: {
    alignSelf: "flex-start", maxWidth: "94%", minHeight: 46, marginHorizontal: 16, marginVertical: 8,
    flexDirection: "row", alignItems: "center", paddingHorizontal: 14, gap: 8, borderRadius: 24, borderWidth: StyleSheet.hairlineWidth,
  },
  title: { ...text.base, fontFamily: font.medium, flexShrink: 1 },
  path: { ...text.sm, fontFamily: mono, flexShrink: 1 },
  routes: { maxHeight: 360 },
  row: { minHeight: 56, flexDirection: "row", alignItems: "center", paddingVertical: 8, paddingHorizontal: 20, gap: 12 },
  rowCopy: { flex: 1, gap: 2 },
});
