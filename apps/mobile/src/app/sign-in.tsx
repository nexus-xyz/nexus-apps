import { useState } from "react";
import { Redirect } from "expo-router";
import { ActivityIndicator, Platform, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import Svg, { Path } from "react-native-svg";
import type { Provider } from "@/app-sign-in";
import { useAuth } from "@/auth";
import { DEVELOPMENT_GATEWAY } from "@/development";
import { SITE_PAGES } from "@/links";
import { open } from "@/markdown";
import { NexusMark } from "@/nexus-mark";
import { ripple } from "@/press";
import { font, text, useTheme, type Theme } from "@/theme";

/** Google's "G" in its own colours, as Google's sign-in button guidelines draw it on a light or a dark button. */
function GoogleMark({ size = 18 }: { size?: number }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 48 48" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z" />
      <Path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z" />
      <Path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z" />
      <Path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z" />
    </Svg>
  );
}

/** Apple's logo, as Apple's Sign in with Apple button draws it, in the button's text colour. */
function AppleLogo({ size = 18, color }: { size?: number; color: string }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 814 1000" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Path
        fill={color}
        d="M788.1 340.9c-5.8 4.5-108.2 62.2-108.2 190.5 0 148.4 130.3 200.9 134.2 202.2-.6 3.2-20.7 71.9-68.7 141.9-42.8 61.6-87.5 123.1-155.5 123.1s-85.5-39.5-164-39.5c-76.5 0-103.7 40.8-165.9 40.8s-105.6-57-155.5-127C46.7 790.7 0 663 0 541.8c0-194.4 126.4-297.5 250.8-297.5 66.1 0 121.2 43.4 162.7 43.4 39.5 0 101.1-46 176.3-46 28.5 0 130.9 2.6 198.3 99.2zm-234-181.5c31.1-36.9 53.1-88.1 53.1-139.3 0-7.1-.6-14.3-1.9-20.1-50.6 1.9-110.8 33.7-147.1 75.8-28.5 32.4-55.1 83.6-55.1 135.5 0 7.8 1.3 15.6 1.9 18.1 3.2.6 8.4 1.3 13.6 1.3 45.4 0 102.5-30.4 135.5-71.3z"
      />
    </Svg>
  );
}

const LABELS: Record<Provider, string> = {
  apple: "Continue with Apple",
  google: "Continue with Google",
  email: "Sign in with email",
};
const METHODS: Provider[] = Platform.OS === "ios" ? ["apple", "google", "email"] : ["google", "email"];

export default function SignIn() {
  const { signIn, signInStatus, notice } = useAuth();
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const [error, setError] = useState<string | null>(null);
  const running = signInStatus.kind !== "idle" ? signInStatus.provider : null;

  const run = async (provider: Provider) => {
    setError(null);
    try {
      await signIn(provider);
    } catch (error) {
      setError(error instanceof Error ? error.message : "Couldn't sign in. Try again.");
    }
  };

  const message = error ?? notice;
  const status = signInStatus.kind === "browser" ? "Opening sign-in…" : signInStatus.kind === "finishing" ? "Signing in to Nexus…" : message;

  if (DEVELOPMENT_GATEWAY) return <Redirect href="/dev/connect" />;

  return (
    <ScrollView
      style={[styles.viewport, { backgroundColor: theme.screen }]}
      contentContainerStyle={[styles.screen, { paddingTop: insets.top + 24, paddingBottom: insets.bottom + 24 }]}
      bounces={false}
    >
      <View style={styles.content}>
        <View style={styles.hero}>
          <View accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
            <NexusMark size={56} />
          </View>
          <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">
            Sign in to Nexus
          </Text>
        </View>
        <View style={styles.status}>
          {status && (
            <Text
              accessibilityLiveRegion="polite"
              accessibilityRole={message && !running ? "alert" : undefined}
              style={[styles.statusText, { color: message && !running ? theme.danger : theme.muted }]}
            >
              {status}
            </Text>
          )}
        </View>
        <View style={styles.actions}>
          {METHODS.map((provider, index) => (
            <SignInButton
              key={provider}
              provider={provider}
              primary={index === 0}
              active={running === provider}
              busy={running !== null}
              theme={theme}
              onPress={() => { void run(provider); }}
            />
          ))}
        </View>
        <Text style={[styles.consent, { color: theme.muted }]}>
          By continuing, you agree to the{" "}
          <Text accessibilityRole="link" style={[styles.underline, { color: theme.foreground }]} onPress={() => open(SITE_PAGES.terms)}>
            Customer Terms
          </Text>{" "}
          and{" "}
          <Text accessibilityRole="link" style={[styles.underline, { color: theme.foreground }]} onPress={() => open(SITE_PAGES.privacyPolicy)}>
            Privacy Policy
          </Text>
          .
        </Text>
      </View>
    </ScrollView>
  );
}

function SignInButton({ provider, primary, active, busy, theme, onPress }: {
  provider: Provider;
  primary: boolean;
  active: boolean;
  busy: boolean;
  theme: Theme;
  onPress: () => void;
}) {
  const label = LABELS[provider];
  const color = primary ? theme.onPrimary : theme.foreground;
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ busy: active, disabled: busy }}
      android_ripple={ripple(theme)}
      disabled={busy}
      onPress={onPress}
      style={({ pressed }) => [
        styles.button,
        !primary && styles.outline,
        { backgroundColor: primary ? theme.primary : theme.card, borderColor: theme.border },
        pressed && styles.pressed,
        busy && !active && styles.inactive,
      ]}
    >
      <View style={styles.buttonIcon} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
        {active ? (
          <ActivityIndicator color={color} />
        ) : provider === "apple" ? (
          <AppleLogo color={color} />
        ) : provider === "google" ? (
          <GoogleMark />
        ) : null}
      </View>
      <Text style={[styles.buttonText, { color }]}>{label}</Text>
      <View style={styles.buttonIcon} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  viewport: { flex: 1 },
  screen: { flexGrow: 1, alignItems: "center", paddingHorizontal: 24 },
  content: { flexGrow: 1, width: "100%", maxWidth: 400 },
  hero: { flexGrow: 1, alignItems: "center", justifyContent: "center", paddingVertical: 32, gap: 16 },
  title: { ...text.largeTitle, fontFamily: font.display, textAlign: "center" },
  status: { minHeight: 28, justifyContent: "center", marginBottom: 8 },
  statusText: { ...text.secondary, fontFamily: font.regular, textAlign: "center" },
  actions: { gap: 8 },
  button: { minHeight: 56, borderRadius: 28, flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 20, paddingVertical: 12, overflow: "hidden" },
  outline: { borderWidth: StyleSheet.hairlineWidth },
  buttonIcon: { width: 20, alignItems: "center" },
  buttonText: { ...text.body, flex: 1, fontFamily: font.medium, textAlign: "center" },
  pressed: { opacity: 0.85 },
  inactive: { opacity: 0.6 },
  consent: { ...text.caption, marginTop: 20, fontFamily: font.regular, textAlign: "center" },
  underline: { textDecorationLine: "underline" },
});
