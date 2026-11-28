// One weight per import: a package's index requires all 18 of its files, and the app would ship each of them.
import { DMSans_400Regular } from "@expo-google-fonts/dm-sans/400Regular";
import { DMSans_400Regular_Italic } from "@expo-google-fonts/dm-sans/400Regular_Italic";
import { DMSans_500Medium } from "@expo-google-fonts/dm-sans/500Medium";
import { GeistMono_400Regular } from "@expo-google-fonts/geist-mono/400Regular";
import { isRunningInExpoGo } from "expo";
import { useFonts } from "expo-font";
import * as SplashScreen from "expo-splash-screen";
import * as SystemUI from "expo-system-ui";
import { DarkTheme, DefaultTheme, Stack, ThemeProvider, type ErrorBoundaryProps } from "expo-router";
import { useEffect, useMemo, useRef } from "react";
import { LogBox, Platform, StyleSheet, useColorScheme } from "react-native";
import { GestureHandlerRootView } from "react-native-gesture-handler";
// Included in Expo Go for SDK 58 and linked automatically in development builds.
import { KeyboardProvider } from "react-native-keyboard-controller";
import { connectionIntent } from "@/connection-intent";
import { AuthProvider, useAuth } from "@/auth";
import { installDeviceTrust } from "@/device-trust";
import { GUEST_ENABLED } from "@/guest";
import { DiagramRenderer } from "@/markdown";
import { ErrorBoundary as ReportingErrorBoundary, installReporting, useReporting } from "@/reporting";
import { restoreAppearance, useTheme } from "@/theme";

void SplashScreen.preventAutoHideAsync();
if (Platform.OS === "android" && !isRunningInExpoGo()) SplashScreen.setOptions({ duration: 0 });

// The sidebar's drawer (react-native-drawer-layout 4.2, inside Expo Router) passes dependency arrays to
// useAnimatedStyle, which Reanimated 4 ignores on native and warns about on every frame. Only that warning is hidden;
// the app's own code passes none. Drop this when the library stops passing them.
LogBox.ignoreLogs(["[Reanimated] Dependencies should only be used on the web"]);
// Expo's DOM components (expo 58) send their props as they mount, before the web view exists, and that call fails;
// Expo sends them again once the page is ready. The diagram renderer (`DiagramRenderer`) logs it each time it mounts.
LogBox.ignoreLogs(["Unable to find the 'DomWebView' view"]);
restoreAppearance();
installReporting();
// The phone's trust checks remember revocations and verified devices across launches.
installDeviceTrust();

// A render error shows Expo Router's error screen, with Retry, instead of taking the app down, and is reported.
export function ErrorBoundary(props: ErrorBoundaryProps) {
  useEffect(() => SplashScreen.hide(), []);
  return <ReportingErrorBoundary {...props} />;
}

function Screens() {
  const { core } = useAuth();
  const theme = useTheme();
  const revealed = useRef(false);
  useReporting(core);
  // The display cut is not in @expo-google-fonts: it is brand/v1.0's static file, copied with its license.
  const [fonts, fontError] = useFonts({
    DMSans_400Regular,
    DMSans_400Regular_Italic,
    DMSans_500Medium,
    DMSans_24pt_400Regular: require("../../assets/fonts/DMSans_24pt-Regular.ttf"),
    GeistMono_400Regular,
  });
  if (core === undefined || (!fonts && !fontError)) return null;
  // Signed in: the app, a sidebar over a stack of screens (`(app)/_layout.tsx`). Signed out: the sign-in screen.
  return (
    <Stack
      screenOptions={{ headerShown: false, contentStyle: { backgroundColor: theme.screen } }}
      screenListeners={({ route }) => ({
        focus: () => {
          // The index only redirects. Reveal the first destination after its view commits.
          if (route.name === "index" || revealed.current) return;
          revealed.current = true;
          requestAnimationFrame(SplashScreen.hide);
        },
      })}
    >
      <Stack.Screen name="index" />
      <Stack.Protected guard={!!core} redirectTo="/sign-in">
        <Stack.Screen name="(app)" options={{ animation: "none" }} />
        <Stack.Screen name="connect" />
      </Stack.Protected>
      <Stack.Protected guard={!core} redirectTo={connectionIntent() ? "/connect" : "/session/new"}>
        <Stack.Screen name="sign-in" options={{ animation: "none" }} />
      </Stack.Protected>
      {/* Signed out, with guest mode built in: the guest Home, before sign-in. */}
      <Stack.Protected guard={!core && GUEST_ENABLED}>
        <Stack.Screen name="guest" options={{ animation: "none" }} />
      </Stack.Protected>
      <Stack.Protected guard={__DEV__}>
        <Stack.Screen name="dev/code-control" />
        <Stack.Screen name="dev/connect" />
      </Stack.Protected>
    </Stack>
  );
}

export default function RootLayout() {
  const theme = useTheme();
  const dark = useColorScheme() === "dark";
  const navigationTheme = useMemo(() => {
    const base = dark ? DarkTheme : DefaultTheme;
    return {
      ...base,
      colors: { ...base.colors, background: theme.screen, card: theme.card, text: theme.foreground, primary: theme.primary, border: theme.border, notification: theme.danger },
    };
  }, [dark, theme]);
  // Android's window is Material's gray or white wherever the React root does not cover it, as during a transition or
  // a resize for the keyboard. It takes the screen color. iOS keeps its window: sheets show it behind them.
  useEffect(() => {
    if (Platform.OS === "android") void SystemUI.setBackgroundColorAsync(theme.screen);
  }, [theme.screen]);
  return (
    // The sidebar's swipe needs the gesture handler's root.
    <GestureHandlerRootView style={[styles.root, { backgroundColor: theme.screen }]}>
      <ThemeProvider value={navigationTheme}>
        <KeyboardProvider>
          <AuthProvider>
            <Screens />
            <DiagramRenderer />
          </AuthProvider>
        </KeyboardProvider>
      </ThemeProvider>
    </GestureHandlerRootView>
  );
}

const styles = StyleSheet.create({ root: { flex: 1 } });
