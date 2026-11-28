import { Stack } from "expo-router";
import { useColorScheme } from "react-native";
import { useReducedMotion } from "react-native-reanimated";
import { useAuth } from "@/auth";
import { useTheme } from "@/theme";
import { useGatewayDevice } from "@/code-reader";
import { useShareInboxPrompt } from "@/use-share-inbox";

export const unstable_settings = { anchor: "(drawer)" };

/**
 * Pages slide in full screen over the current chat or open sidebar, as Claude's and ChatGPT's do, and Back returns to
 * that exact context. Not a page sheet: in one, a page's own sheets, its keyboard and its full-height content (a page,
 * Search) had a card's height and a second modal level to fight. What was shared to Nexus
 * from another app opens its review (`(main)/shared`) when the app comes to the front. When Gateway vouches for the
 * person's devices, this phone registers its key here, right after sign-in.
 */
export default function AppLayout() {
  const theme = useTheme();
  const dark = useColorScheme() === "dark";
  const reduceMotion = useReducedMotion();
  useShareInboxPrompt();
  useGatewayDevice(useAuth().core);
  return (
    <Stack screenOptions={{ statusBarStyle: dark ? "light" : "dark", headerShown: false, contentStyle: { backgroundColor: theme.screen } }}>
      <Stack.Screen name="(drawer)" />
      <Stack.Screen
        name="(main)"
        options={{
          presentation: "card",
          animation: reduceMotion ? "fade" : "default",
          gestureEnabled: true,
        }}
      />
    </Stack>
  );
}
