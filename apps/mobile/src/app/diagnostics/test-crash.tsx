import { requireOptionalNativeModule } from "expo";
import { router, useLocalSearchParams } from "expo-router";
import { useEffect } from "react";
import { Alert, View } from "react-native";
import { errorReportsOn } from "@/reporting";
import { TEST_ERROR, testCrashKind, type TestCrashKind } from "@/test-crash";
import { useTheme } from "@/theme";

/** The crash module's `crash`. Missing in Expo Go, where only the JavaScript error can be tested. */
const native = requireOptionalNativeModule<{ crash(): void }>("NexusCrashReports");

function leave() {
  if (router.canGoBack()) router.back();
  else router.replace("/");
}

function crash(kind: TestCrashKind) {
  if (kind === "native") native?.crash();
  // Thrown outside React, so it reaches the global handler as a fatal error and not the error screen.
  else
    setTimeout(() => {
      throw new Error(TEST_ERROR);
    });
}

/**
 * `nexus://diagnostics/test-crash?kind=js|native` (`src/test-crash.ts`). No screen links here. It asks first, so a link
 * from someone else cannot close the app. Outside the sign-in guard: the report waits on the device for a sign-in.
 */
export default function TestCrash() {
  const { kind } = useLocalSearchParams<{ kind?: string }>();
  const theme = useTheme();

  useEffect(() => {
    const chosen = testCrashKind(kind);
    if (!chosen || (chosen === "native" && !native)) {
      Alert.alert("Can't test that here", "This build has no native crash test. Try kind=js.", [{ text: "OK", onPress: leave }], {
        onDismiss: leave,
      });
      return;
    }
    if (!errorReportsOn()) {
      Alert.alert("Reports are off", "Turn on Send crash and error reports in Settings, then open the link again.", [{ text: "OK", onPress: leave }], {
        onDismiss: leave,
      });
      return;
    }
    const what = chosen === "native" ? "a native crash" : "a JavaScript error";
    Alert.alert(
      "Crash the app?",
      `Nexus will close with ${what}, to test crash reporting. The report has no content and is sent the next time you open the app.`,
      [
        { text: "Cancel", style: "cancel", onPress: leave },
        { text: "Crash", style: "destructive", onPress: () => crash(chosen) },
      ],
      { cancelable: true, onDismiss: leave },
    );
  }, [kind]);

  return <View style={{ flex: 1, backgroundColor: theme.screen }} />;
}
