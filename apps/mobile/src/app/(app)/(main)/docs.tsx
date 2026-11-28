import { Stack, useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { RefreshControl, ScrollView, StyleSheet } from "react-native";
import { useAuth } from "@/auth";
import { docsLink, docsSource, splitTitle } from "@/docs";
import { Footnote, Loading } from "@/list";
import { Markdown } from "@/markdown";

/**
 * The docs, read in the app: each page is Nexus UI's Markdown of it (`/docs/<path>.md`) in the Markdown renderer.
 * A link to another docs page opens it here, with Back; the rest of the site opens in the in-app browser.
 */
export default function Docs() {
  const { ui } = useAuth();
  const { path } = useLocalSearchParams<{ path?: string }>();
  const [page, setPage] = useState<{ title?: string; body: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const reload = useCallback(() => setAttempt((n) => n + 1), []);
  const shown = useRef(path);

  useEffect(() => {
    if (!ui) return;
    // Another page in this screen starts empty, so an error is never shown over the last page's text; a failed
    // refresh of the same page keeps it under the error.
    if (shown.current !== path) {
      shown.current = path;
      setPage(null);
      setError(null);
    }
    const controller = new AbortController();
    fetch(docsSource(ui, path), { signal: controller.signal, headers: { accept: "text/markdown" } })
      .then(async (response) => {
        if (!response.ok) throw new Error(response.status === 404 ? "This page isn't in the docs." : `The docs answered ${response.status}.`);
        setPage(splitTitle(await response.text()));
        setError(null);
      })
      .catch((e: Error) => controller.signal.aborted || setError(e.message));
    return () => controller.abort();
  }, [ui, path, attempt]);

  return (
    <ScrollView contentContainerStyle={styles.screen} refreshControl={<RefreshControl refreshing={false} onRefresh={reload} />}>
      <Stack.Screen options={{ title: page?.title ?? "Docs" }} />
      {!page && !error ? <Loading /> : null}
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      {page && ui ? <Markdown source={page.body} resolveLink={(href) => docsLink(href, ui)} /> : null}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { padding: 20, paddingBottom: 40 },
});
