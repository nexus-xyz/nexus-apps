import { router, Stack, useLocalSearchParams } from "expo-router";
import { useState } from "react";
import { ActivityIndicator, Pressable, RefreshControl, ScrollView, StyleSheet, Text, View } from "react-native";
import { useCore } from "@/auth";
import { open } from "@/markdown";
import { usd } from "@/meta";
import { pressedFill, ripple } from "@/press";
import { duration, githubRepo, percent, repoName } from "@/products";
import { Status, useRunReport } from "@/run-report";
import { font, mono, text, useTheme } from "@/theme";

/**
 * A run, as desktop's run result shows it: its status, what it read (the repository at a commit), coverage, how long
 * it took and what it cost; why it failed, with Run it again; then its pages, each opening in the reader.
 */
export default function Run() {
  const core = useCore();
  const theme = useTheme();
  const { id, product, capability } = useLocalSearchParams<{ id: string; product: string; capability: string }>();
  const { report, error, reload } = useRunReport(product, capability, id);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState<string | null>(null);

  const target = report ? Object.values(report.input).join(" ") : "";
  const repo = report?.input.repo;
  const github = repo ? githubRepo(repo) : undefined;
  const retry = () => {
    if (!report) return;
    setRetrying(true);
    setRetryError(null);
    core
      .moduleVerb(product, capability, "run", report.input)
      .then((run) => router.replace({ pathname: "/run/[id]", params: { id: run.id, product, capability } }))
      .catch((e: Error) => setRetryError(e.message))
      .finally(() => setRetrying(false));
  };

  return (
    <ScrollView contentContainerStyle={styles.screen} refreshControl={<RefreshControl refreshing={false} onRefresh={reload} />}>
      <Stack.Screen options={{ title: report ? repoName(target) : "" }} />
      {!report && !error ? <ActivityIndicator style={styles.loading} color={theme.muted} /> : null}
      {!report && error ? (
        <Text style={[styles.body, { color: /^no run |not found/i.test(error) ? theme.muted : theme.danger }]}>
          {/^no run |not found/i.test(error) ? "This run isn't here. Its repository may have been unlinked." : `The run did not load. ${error}`}
        </Text>
      ) : null}
      {report ? (
        <>
          <View style={[styles.card, { backgroundColor: theme.card, borderColor: theme.border }]}>
            <View style={styles.head}>
              <Status status={report.status} theme={theme} />
              <Text style={[styles.money, { color: theme.muted }]}>
                {report.status === "running" ? `${usd(report.costUsd)} so far` : usd(report.costUsd)} · {duration(report.startedAt, report.finishedAt)}
              </Text>
            </View>
            <Text style={[styles.title, { color: theme.foreground }]}>
              {report.pages.length} {report.pages.length === 1 ? "page" : "pages"}
              {report.coverage !== undefined ? ` · ${percent(report.coverage)} coverage` : ""}
            </Text>
            <Pressable onPress={github ? () => open(report.commit ? `${github}/tree/${report.commit}` : github) : undefined} disabled={!github} accessibilityRole={github ? "link" : undefined} style={github ? styles.link : undefined}>
              <Text style={[styles.mono, { color: github ? theme.primary : theme.muted }]} numberOfLines={1}>
                {repoName(target)}
                {report.commit ? ` @ ${report.commit.slice(0, 7)}` : ""}
              </Text>
            </Pressable>
            {report.status === "running" ? (
              <View style={styles.running}>
                <ActivityIndicator color={theme.muted} />
                <Text style={[styles.body, { color: theme.muted, flex: 1 }]}>
                  Writing pages{report.pages.length ? `, ${report.pages.length} so far` : ""}. You can leave and come back.
                </Text>
              </View>
            ) : null}
            {report.error ? (
              <View style={[styles.problem, { backgroundColor: theme.dangerFaint }]}>
                <Text style={[styles.body, { color: theme.danger }]}>The run failed. {report.error}</Text>
                {report.status === "failed" ? (
                  <Pressable onPress={retry} disabled={retrying} accessibilityRole="button">
                    <Text style={[styles.strong, { color: theme.danger }]}>{retrying ? "Starting…" : "Run it again"}</Text>
                  </Pressable>
                ) : null}
                {retryError ? <Text style={[styles.body, { color: theme.danger }]}>The new run did not start. {retryError}</Text> : null}
              </View>
            ) : null}
          </View>

          {report.status !== "running" && report.pages.length === 0 && !report.error ? (
            <Text style={[styles.body, { color: theme.muted }]}>{report.status === "failed" ? "The run stopped before it wrote any page." : "The run finished without pages."}</Text>
          ) : null}
          {report.pages.length ? (
            <View style={[styles.group, { backgroundColor: theme.card, borderColor: theme.border }]}>
              {report.pages.map((page, n) => (
                <Pressable
                  accessibilityRole="button"
                  key={page.slug}
                  android_ripple={ripple(theme)}
                  style={({ pressed }) => [styles.row, n > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderColor: theme.border }, pressed && pressedFill(theme)]}
                  onPress={() => router.push({ pathname: "/run/[id]/[page]", params: { id, page: page.slug, product, capability } })}
                >
                  <Text style={[styles.n, { color: theme.muted }]}>{String(n + 1).padStart(2, "0")}</Text>
                  <Text style={[styles.pageTitle, { color: theme.foreground }]} numberOfLines={2}>
                    {page.title}
                  </Text>
                </Pressable>
              ))}
            </View>
          ) : null}
        </>
      ) : null}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { padding: 16, gap: 12 },
  loading: { padding: 24 },
  card: { borderRadius: 24, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, padding: 16, gap: 8 },
  // Wraps, so at the larger text sizes the cost goes under the status instead of past the card's edge.
  head: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", justifyContent: "space-between", gap: 8 },
  title: { ...text.lg, fontFamily: font.medium },
  mono: { fontFamily: mono, fontSize: 13, lineHeight: 18 },
  // A 44 pt target around the one-line repository link, its margin keeping the line where it was.
  link: { minHeight: 44, justifyContent: "center", marginVertical: -13 },
  money: { fontFamily: mono, fontSize: 13 },
  body: { ...text.sm, fontFamily: font.regular },
  strong: { ...text.sm, fontFamily: font.medium },
  running: { flexDirection: "row", alignItems: "center", gap: 10, paddingTop: 4 },
  problem: { borderRadius: 12, padding: 12, gap: 6 },
  group: { borderRadius: 24, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, overflow: "hidden" },
  row: { flexDirection: "row", alignItems: "center", gap: 12, paddingHorizontal: 16, minHeight: 52, paddingVertical: 10 },
  n: { fontFamily: mono, fontSize: 13 },
  pageTitle: { ...text.base, fontFamily: font.medium, flex: 1 },
});
