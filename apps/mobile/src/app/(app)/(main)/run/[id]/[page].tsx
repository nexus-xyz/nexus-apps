import { router, Stack, useLocalSearchParams } from "expo-router";
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { Markdown } from "@/markdown";
import { pressedFill, ripple } from "@/press";
import { githubRepo, repoName } from "@/products";
import { useRunReport } from "@/run-report";
import { font, text, useTheme } from "@/theme";

/**
 * A run page, as desktop's reader shows it: the page in the Markdown renderer, its code citations opening the lines
 * on GitHub and relative file links the files at the commit the run read, then Previous and Next.
 */
export default function RunPage() {
  const theme = useTheme();
  const { id, page: slug, product, capability } = useLocalSearchParams<{ id: string; page: string; product: string; capability: string }>();
  const { report, error } = useRunReport(product, capability, id);

  const n = report ? Math.max(0, report.pages.findIndex((p) => p.slug === slug)) : 0;
  const page = report?.pages[n];
  const repo = report?.input.repo;
  const github = repo ? githubRepo(repo) : undefined;
  const blob = github && `${github}/blob/${report?.commit ?? "HEAD"}/`;
  const go = (to: number) => {
    const next = report?.pages[to];
    if (!next) return;
    router.setParams({ page: next.slug });
  };

  return (
    // Keyed by the page, so Previous and Next start at its top.
    <ScrollView key={page?.slug} contentContainerStyle={styles.screen}>
      <Stack.Screen options={{ title: page?.title ?? "" }} />
      {!report && !error ? <ActivityIndicator style={styles.loading} color={theme.muted} /> : null}
      {!report && error ? <Text style={[styles.body, { color: theme.danger }]}>The page did not load. {error}</Text> : null}
      {report && !page ? <Text style={[styles.body, { color: theme.muted }]}>This run has no pages yet.</Text> : null}
      {page ? (
        <>
          <Markdown source={page.markdown} cite={(c) => page.citations?.find((x) => x.n === c)?.href} linkBase={blob || undefined} />
          {report.pages.length > 1 ? (
            <View style={styles.pager}>
              {n > 0 ? <Pager label="Previous" title={report.pages[n - 1]!.title} onPress={() => go(n - 1)} /> : <View style={styles.flex} />}
              {n < report.pages.length - 1 ? <Pager label="Next" title={report.pages[n + 1]!.title} onPress={() => go(n + 1)} next /> : <View style={styles.flex} />}
            </View>
          ) : null}
          {blob && repo ? (
            <Text style={[styles.source, { color: theme.muted }]}>
              Written from {repoName(repo)}
              {report.finishedAt ? ` on ${new Date(report.finishedAt).toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" })}` : ""}.
            </Text>
          ) : null}
        </>
      ) : null}
    </ScrollView>
  );
}

function Pager({ label, title, onPress, next }: { label: string; title: string; onPress: () => void; next?: boolean }) {
  const theme = useTheme();
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.pagerLink, { borderColor: theme.border, backgroundColor: theme.card }, next && styles.next, pressed && pressedFill(theme)]}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={`${label}: ${title}`}
    >
      <Text style={[styles.pagerLabel, { color: theme.muted }]}>{label}</Text>
      <Text style={[styles.pagerTitle, { color: theme.foreground }, next && styles.right]} numberOfLines={2}>
        {title}
      </Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  screen: { padding: 20, paddingBottom: 40, gap: 16 },
  loading: { padding: 24 },
  body: { ...text.sm, fontFamily: font.regular },
  pager: { flexDirection: "row", gap: 10, marginTop: 8 },
  flex: { flex: 1 },
  pagerLink: { flex: 1, borderRadius: 16, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, padding: 12, gap: 2 },
  next: { alignItems: "flex-end" },
  right: { textAlign: "right" },
  pagerLabel: { ...text.xs, fontFamily: font.medium },
  pagerTitle: { ...text.sm, fontFamily: font.medium },
  source: { ...text.xs, fontFamily: font.regular },
});
