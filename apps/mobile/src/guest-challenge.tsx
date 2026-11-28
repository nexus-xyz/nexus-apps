import { useCallback, useEffect, useRef, useState } from "react";
import { StyleSheet, View } from "react-native";
import { WebView } from "react-native-webview";
import { challengeStep, freshToken, GUEST_CHALLENGE_URL, GuestChatError } from "@/guest";

/**
 * Cloudflare Turnstile for guest chat (ADR 0019 amendment): Nexus UI's challenge page in a small web view.
 * The widget stays empty unless Cloudflare needs the person. The page hands over its token by a navigation the web
 * view cancels (`challengeStep`). `next()` resolves to a fresh one-use token; `prepare()` loads the next one ahead.
 * `off` turns true when the page answers 404 (Nexus UI has no guest site key), as Nexus UI's guest mode is off then.
 */
export function useGuestChallenge() {
  const kept = useRef<{ token: string; at: number } | null>(null);
  const waiting = useRef<{ resolve: (token: string) => void; reject: (error: Error) => void }[]>([]);
  const [loading, setLoading] = useState(false);
  const busy = useRef(false);
  // A new key is a new web view: one challenge per load.
  const [load, setLoad] = useState(0);
  const [off, setOff] = useState(false);

  const start = useCallback(() => {
    if (busy.current) return;
    busy.current = true;
    setLoad((n) => n + 1);
    setLoading(true);
  }, []);

  const prepare = useCallback(() => {
    if (!freshToken(kept.current, Date.now())) start();
  }, [start]);

  const settle = useCallback((outcome: { token: string } | { error: string; off?: true }) => {
    busy.current = false;
    setLoading(false);
    const waiters = waiting.current.splice(0);
    if ("error" in outcome) {
      waiters.forEach(({ reject }) => reject(new GuestChatError(outcome.off ? "off" : "turnstile", `The check did not finish (${outcome.error}).`)));
      return;
    }
    const first = waiters.shift();
    if (first) first.resolve(outcome.token);
    else kept.current = { token: outcome.token, at: Date.now() };
    // A token is good once: anyone else waiting needs the next load.
    if (waiters.length) {
      waiting.current.push(...waiters);
      start();
    }
  }, [start]);

  const next = useCallback(
    () =>
      new Promise<string>((resolve, reject) => {
        const token = freshToken(kept.current, Date.now());
        kept.current = null;
        if (token) return resolve(token);
        const waiter = { resolve, reject };
        waiting.current.push(waiter);
        start();
        setTimeout(() => {
          const at = waiting.current.indexOf(waiter);
          if (at < 0) return;
          waiting.current.splice(at, 1);
          reject(new GuestChatError("turnstile", "The check did not finish."));
        }, 15_000);
      }),
    [start],
  );

  useEffect(() => () => void waiting.current.splice(0).forEach(({ reject }) => reject(new GuestChatError("failed", "Closed."))), []);

  const element = loading ? (
    <View style={styles.box}>
      <WebView
        key={load}
        source={{ uri: GUEST_CHALLENGE_URL }}
        style={styles.web}
        // Every URL goes to the check below; Cloudflare's frames load, any other page does not.
        originWhitelist={["*"]}
        onShouldStartLoadWithRequest={(request) => {
          const step = challengeStep(request.url, request.isTopFrame !== false);
          if (typeof step === "object") {
            settle(step);
            return false;
          }
          return step === "load";
        }}
        onHttpError={({ nativeEvent }) => {
          if (nativeEvent.statusCode === 404 && nativeEvent.url.startsWith(GUEST_CHALLENGE_URL)) {
            setOff(true);
            settle({ error: "not_found", off: true });
          }
        }}
        onError={({ nativeEvent }) => settle({ error: nativeEvent.description || "network" })}
        javaScriptEnabled
        // Cloudflare asks web views to keep their storage and default user agent between challenges.
        domStorageEnabled
        allowFileAccess={false}
        setSupportMultipleWindows={false}
        allowsLinkPreview={false}
      />
    </View>
  ) : null;

  return { element, next, prepare, off };
}

const styles = StyleSheet.create({
  // Room for Turnstile's widget (300 by 65) when Cloudflare needs the person; empty otherwise.
  box: { height: 72, alignSelf: "center", width: 300, overflow: "hidden" },
  web: { backgroundColor: "transparent" },
});
