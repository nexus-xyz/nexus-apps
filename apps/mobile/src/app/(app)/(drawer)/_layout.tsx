import { Drawer } from "expo-router/drawer";
import { Platform, StyleSheet, useWindowDimensions, View } from "react-native";
import { SidebarDepth } from "@/drawer-card";
import { Sidebar } from "@/sidebar";
import { ToastProvider } from "@/toast";
import { useTheme } from "@/theme";

/** The chat and its persistent sidebar stay beneath the app's full-screen pages. */
const ANDROID = Platform.OS === "android";

export default function DrawerLayout() {
  const theme = useTheme();
  const { width } = useWindowDimensions();
  return (
    <ToastProvider>
      {/*
        On iOS the sidebar's colour runs the full width under the card, as Claude's app: the drawer is 320 pt wide,
        and the card's rounded corners sit just past it. Without this they would show the chat's own colour.
      */}
      <View style={[styles.fill, !ANDROID && { backgroundColor: theme.sidebar }]}>
        <Drawer
          drawerContent={(props) => (
            <SidebarDepth>
              <Sidebar navigation={props.navigation} />
            </SidebarDepth>
          )}
          screenOptions={{
            headerShown: false,
            // On iOS the sidebar stays in place and the chat slides off it as a card (`drawer-card.tsx`), as Claude's.
            drawerType: ANDROID ? "front" : "back",
            drawerStyle: [
              { width: Math.min(width * 0.82, 320), backgroundColor: theme.sidebar },
              ANDROID && { borderTopRightRadius: 16, borderBottomRightRadius: 16, overflow: "hidden" },
            ],
            // The card shows the sidebar's colour through its rounded corners.
            sceneStyle: ANDROID ? undefined : { backgroundColor: "transparent" },
            // The chat stays in view at the edge; a tap on it closes the sidebar. On iOS the card fades its own content,
            // so the overlay only takes the tap: a coloured one would square off the card's corners.
            overlayColor: ANDROID ? "rgba(0,0,0,0.32)" : "transparent",
            swipeEdgeWidth: ANDROID ? 40 : width,
          }}
        >
          <Drawer.Screen name="(chat)" />
        </Drawer>
      </View>
    </ToastProvider>
  );
}

const styles = StyleSheet.create({ fill: { flex: 1 } });
