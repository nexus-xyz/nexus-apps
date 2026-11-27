import { Stack, router } from "expo-router";
import { Platform, Pressable, useColorScheme } from "react-native";
import { useReducedMotion } from "react-native-reanimated";
import { ToastProvider } from "@/toast";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { font, useTheme } from "@/theme";

export default function MainLayout() {
  const theme = useTheme();
  const dark = useColorScheme() === "dark";
  const reduceMotion = useReducedMotion();
  // The grouped lists' bar, on their grey. iOS's own large title draws only once collapsed when its stack is pushed over
  // another (iOS 26, react-native-screens 4.30), and this stack is pushed over the chat, so a list sets its large title
  // in its content instead (`LargeTitle`), and its bar keeps no title.
  const grouped = { headerStyle: { backgroundColor: theme.grouped }, contentStyle: { backgroundColor: theme.grouped } };
  const listTitle = { ...grouped, headerTitle: "" };
  return (
    // T3's stack: no large titles, no hairline under the header, a bare back chevron. Android keeps Material 3's top app
    // bar: the back arrow, and the title on the left in its 22 sp title size (DM Sans's display cut, 21 points or more).
    <ToastProvider>
      <Stack
        screenOptions={({ navigation, route }) => ({
          headerLeft: navigation.getState().routes[0]?.key === route.key
            ? () => (
                // The first page of the stack has no native Back: this one returns to the chat or sidebar under it.
                <Pressable onPress={() => { haptic("tap"); router.back(); }} style={{ width: 44, height: 44, alignItems: "center", justifyContent: "center" }} accessibilityRole="button" accessibilityLabel="Back">
                  <Icon name="back" size={18} color={theme.foreground} />
                </Pressable>
              )
            : undefined,
          headerUserInterfaceStyle: dark ? "dark" : "light",
          statusBarStyle: dark ? "light" : "dark",
          headerBackButtonDisplayMode: "minimal",
          headerShadowVisible: false,
          headerStyle: { backgroundColor: theme.screen },
          headerTintColor: theme.foreground,
          headerTitleStyle: Platform.OS === "android" ? { fontFamily: font.display, fontSize: 22 } : { fontFamily: font.medium, fontSize: 17 },
          contentStyle: { backgroundColor: theme.screen },
        })}
      >
        {/* The lists brand v5 gives a large title (P18, P20, P21): 34 pt DM Sans Medium. */}
        <Stack.Screen name="chats" options={{ title: "Sessions", headerTitle: "", ...grouped }} />
        <Stack.Screen name="archived" options={{ title: "Archived" }} />
        {/* Settings and the other entry pages share the first page's Back; drill-down pages keep native Back. */}
        <Stack.Screen name="settings" options={{ title: "Settings", ...listTitle }} />
        {/* The pages Settings drills into (P68, P69, P71). A row that holds several pages opens their list first
            (`settings-group/[key]`, titled with the row's name), as the desktop's Settings groups them. */}
        <Stack.Screen name="settings-group/[key]" options={{ title: "", ...grouped }} />
        <Stack.Screen name="privacy" options={{ title: "Privacy", ...grouped }} />
        <Stack.Screen name="assistant-settings" options={{ title: "Assistant", ...grouped }} />
        {/* The errands and their History, from the Assistant's menu (A12). */}
        <Stack.Screen name="errands" options={{ title: "Errands", ...grouped }} />
        {/* Projects (A14) and one project, from the Assistant's menu. */}
        <Stack.Screen name="assistant-projects" options={{ title: "Projects", ...grouped }} />
        <Stack.Screen name="assistant-project" options={{ title: "", ...grouped }} />
        <Stack.Screen name="about" options={{ title: "About", ...grouped }} />
        <Stack.Screen name="profile" options={{ title: "Profile" }} />
        <Stack.Screen name="appearance" options={{ title: "Appearance" }} />
        <Stack.Screen name="accounts" options={{ title: "Organizations" }} />
        <Stack.Screen name="referrals" options={{ title: "Referrals" }} />
        <Stack.Screen name="feedback" options={{ title: "Send feedback" }} />
        {/* The Assistant's Privacy (A11), from Account › Assistant: not the app's own Privacy above. */}
        <Stack.Screen name="assistant-privacy" options={{ title: "Privacy", ...grouped }} />
        {/* The Assistant's Memory (A16) and one of its files, from Account › Assistant. */}
        <Stack.Screen name="assistant-memory" options={{ title: "Memory", ...grouped }} />
        <Stack.Screen name="memory-file" options={{ title: "", ...grouped }} />
        {/* Discover (A15): from the empty chat and Account › Assistant. */}
        <Stack.Screen name="discover" options={{ title: "Discover", ...grouped }} />
        <Stack.Screen name="skills" options={{ title: "Skills" }} />
        <Stack.Screen name="skill/[id]" options={{ title: "" }} />
        <Stack.Screen name="policy" options={{ title: "Policy" }} />
        <Stack.Screen name="budgets" options={{ title: "Budgets" }} />
        {/* How often Nexus writes first (A10), from Account › Assistant. */}
        <Stack.Screen name="proactivity" options={{ title: "Proactivity", ...grouped }} />
        <Stack.Screen name="integrations" options={{ title: "Integrations" }} />
        <Stack.Screen name="computers" options={{ title: "Computers", ...grouped }} />
        <Stack.Screen name="computers/connect" options={{ title: "", animation: reduceMotion ? "none" : "default" }} />
        {/* Passwords and payments and its log (A8), from Account › Assistant. */}
        <Stack.Screen name="vault" options={{ title: "Passwords and payments", ...grouped }} />
        <Stack.Screen name="vault-activity" options={{ title: "Activity", ...grouped }} />
        {/* Spend is Usage, as desktop's map names it (P48). */}
        <Stack.Screen name="spend" options={{ title: "Usage" }} />
        <Stack.Screen name="artifact/[id]" options={{ title: "" }} />
        <Stack.Screen name="pages" options={{ title: "Artifacts", headerTitle: "", ...grouped }} />
        {/* A page owns the screen (P20): its bar is at its foot (`page-bar.tsx`). */}
        <Stack.Screen name="page/[id]" options={{ title: "", headerShown: false }} />
        {/* A site's overview and deploys (P20 Site), from its page's More. */}
        <Stack.Screen name="site/[id]/index" options={{ title: "Site", ...grouped }} />
        <Stack.Screen name="site/[id]/deploys" options={{ title: "Deploys", ...grouped }} />
        {/* Search draws its own field at the top, with Cancel (P94·B). */}
        <Stack.Screen name="search" options={{ title: "Search", headerShown: false }} />
        <Stack.Screen name="agents" options={{ title: "Agents and plans" }} />
        {/* Find and notify (P35, P21): the inbox, from the drawer, and what notifies you, from it and from Account. */}
        <Stack.Screen name="inbox" options={{ title: "Inbox" }} />
        <Stack.Screen name="notification-settings/index" options={{ title: "Notifications" }} />
        <Stack.Screen name="notification-settings/[group]" options={{ title: "" }} />
        {/* Agents at work (P4, P18): the sessions, opened from the drawer, and one session pushed from them. */}
        <Stack.Screen name="agent-sessions" options={{ title: "Agents", ...listTitle }} />
        <Stack.Screen name="agent-session/[id]" options={{ title: "", ...grouped }} />
        <Stack.Screen name="repositories" options={{ title: "Repositories" }} />
        {/* Billing (P40·C): its large title in the list, under Back with the screen it came from (Settings). */}
        <Stack.Screen name="billing" options={{ title: "Billing", headerTitle: "", headerBackButtonDisplayMode: "default", ...grouped }} />
        {/* Every receipt, from Billing's "All receipts". */}
        <Stack.Screen name="receipts" options={{ title: "Receipts", ...grouped }} />
        {/* Lower the usage limit, from Billing's row. */}
        <Stack.Screen name="usage-limit" options={{ title: "Usage limit", ...grouped }} />
        <Stack.Screen name="delete-account" options={{ title: "Delete account" }} />
        <Stack.Screen name="members" options={{ title: "Members" }} />
        <Stack.Screen name="models" options={{ title: "Models" }} />
        <Stack.Screen name="keys" options={{ title: "API keys" }} />
        <Stack.Screen name="audit" options={{ title: "Audit log" }} />
        <Stack.Screen name="teams" options={{ title: "Teams" }} />
        <Stack.Screen name="team/[slug]" options={{ title: "" }} />
        <Stack.Screen name="analytics" options={{ title: "Analytics" }} />
        <Stack.Screen name="gateway" options={{ title: "Nexus Gateway" }} />
        <Stack.Screen name="docs" options={{ title: "Docs" }} />
        <Stack.Screen name="product/[slug]" options={{ title: "" }} />
        <Stack.Screen name="run/[id]/index" options={{ title: "" }} />
        <Stack.Screen name="run/[id]/[page]" options={{ title: "" }} />
        {/* People and Controls on the admin page system (P105·D): each sets its large title in the list (`people-controls.tsx`). */}
        <Stack.Screen name="member/[id]" options={{ title: "" }} />
        <Stack.Screen name="not-joined" options={{ title: "Not joined yet" }} />
        <Stack.Screen name="team-people/[slug]" options={{ title: "People" }} />
        <Stack.Screen name="team-budget/[slug]" options={{ title: "" }} />
        <Stack.Screen name="model-catalog" options={{ title: "Catalogue" }} />
        <Stack.Screen name="policy-rule" options={{ title: "" }} />
        <Stack.Screen name="providers" options={{ title: "Providers" }} />
        <Stack.Screen name="provider/[id]" options={{ title: "" }} />
        {/* Tools, Activity and Admin pages and items (P105·D): each sets its title in the list. */}
        <Stack.Screen name="integration/[name]" options={{ title: "" }} />
        <Stack.Screen name="repository/[id]" options={{ title: "" }} />
        <Stack.Screen name="audit-event/[id]" options={{ title: "Change" }} />
        <Stack.Screen name="observability" options={{ title: "Observability" }} />
        <Stack.Screen name="general" options={{ title: "General" }} />
        <Stack.Screen name="data" options={{ title: "Data" }} />
        {/* What another app shared to Nexus: where it goes, before it reaches a composer. */}
        <Stack.Screen name="shared" options={{ title: "Shared with Nexus", ...grouped }} />
      </Stack>
    </ToastProvider>
  );
}
