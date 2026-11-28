import { Redirect, router, useLocalSearchParams } from "expo-router";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, Text, Pressable } from "react-native";
import { useAuth } from "@/auth";
import { DEVELOPMENT_GATEWAY } from "@/development";
import { Footnote, LargeTitle, Page, Section } from "@/list";
import { text, useTheme } from "@/theme";

/** The QR contains a one-use Gateway device grant, never an API key or a bundled credential. */
export default function ConnectDevelopment() {
  const { code } = useLocalSearchParams<{ code?: string }>();
  const { connectDevelopment } = useAuth();
  const theme = useTheme();
  const [error, setError] = useState<string>();
  const pending = useRef<{ code: string; task: Promise<void> } | undefined>(undefined);
  useEffect(() => {
    if (!DEVELOPMENT_GATEWAY || !code) return;
    let active = true;
    if (pending.current?.code !== code) pending.current = { code, task: connectDevelopment(code) };
    pending.current.task.then(
      () => active && router.replace("/session/new"),
      (reason: unknown) => active && setError(reason instanceof Error ? reason.message : "The connection failed."),
    );
    return () => { active = false; };
  }, [code, connectDevelopment]);
  if (!DEVELOPMENT_GATEWAY) return <Redirect href="/sign-in" />;
  return (
    <Page>
      <LargeTitle>Connect to your computer</LargeTitle>
      <Section title="Local development">
        <Text style={[text.body, { color: theme.foreground }]}>{error ?? (code ? "Connecting to Gateway…" : "Scan the connection QR from your computer.")}</Text>
        {code && !error ? <ActivityIndicator color={theme.primary} /> : null}
      </Section>
      <Pressable accessibilityRole="button" onPress={() => router.push("/connect")} style={{ minHeight: 44, justifyContent: "center" }}><Text style={[text.body, { color: theme.foreground }]}>Scan computer QR</Text></Pressable>
      <Footnote>{DEVELOPMENT_GATEWAY}. This preview keeps a separate sign-in and pairing key.</Footnote>
    </Page>
  );
}
