import { router, useLocalSearchParams } from "expo-router";
import { View } from "react-native";
import { opensInDrawer } from "@/drawer-places";
import { haptic } from "@/haptics";
import { SearchPanel } from "@/search-panel";
import { useTheme } from "@/theme";

/** Search links still open directly; the sidebar embeds the same search without navigating. */
export default function Search() {
  const { q } = useLocalSearchParams<{ q?: string }>();
  const theme = useTheme();
  return (
    <View style={{ flex: 1, backgroundColor: theme.screen }}>
      <SearchPanel initialQuery={q} onClose={() => router.back()} onNavigate={(href) => {
        haptic("tap");
        if (opensInDrawer(href)) router.dismissTo(href);
        else router.push(href);
      }} />
    </View>
  );
}
