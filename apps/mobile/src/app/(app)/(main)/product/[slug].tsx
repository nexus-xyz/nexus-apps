import type { ModuleManifest, RunsPage } from "@nexus/kit";
import { router, Stack, useFocusEffect, useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { ActivityIndicator, Pressable, RefreshControl, ScrollView, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { useCore } from "@/auth";
import { TextInput } from "@/controls";
import { usd } from "@/meta";
import { pressedFill, ripple } from "@/press";
import { duration, productName, repoName } from "@/products";
import { POLL_MS, Status } from "@/run-report";
import { nextCursor, pageQuery, pageRuns, withFirstPage } from "@/run-pages";
import { font, mono, text, useTheme, type Theme } from "@/theme";
import { isAccessibilitySize } from "@/text-size";

const VISIBLE_RUNS = 5;

/**
 * A product, as desktop's product page shows it: one capability is the screen, several are tabs; each is drawn from
 * its manifest's panel kind. Today there is one kind, `pages`: start a run, and the runs, each opening
 * its pages.
 */
export default function Product() {
  const core = useCore();
  const theme = useTheme();
  const { slug, capability } = useLocalSearchParams<{ slug: string; capability?: string }>();
  const [manifests, setManifests] = useState<ModuleManifest[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState(capability);

  useEffect(() => {
    core
      .modules()
      .then((r) => setManifests(r.modules.filter((m) => m.product.slug === slug)))
      .catch((e: Error) => setError(e.message));
  }, [core, slug]);

  const manifest = manifests?.find((m) => m.capability.slug === tab) ?? manifests?.[0];
  return (
    <>
      <Stack.Screen options={{ title: manifest ? productName(manifest.product) : "" }} />
      {error ? <Text style={[styles.note, { color: theme.danger }]}>{error}</Text> : null}
      {manifests === null && !error ? <ActivityIndicator style={styles.loading} color={theme.muted} /> : null}
      {manifests?.length === 0 ? <Text style={[styles.note, { color: theme.muted }]}>This product isn't registered in your organization.</Text> : null}
      {manifests && manifests.length > 1 ? (
        <View style={[styles.tabs, { backgroundColor: theme.userBubble }]} accessibilityRole="tablist">
          {manifests.map((m) => (
            <Pressable
              key={m.capability.slug}
              style={[styles.tab, m === manifest && { backgroundColor: theme.card }]}
              onPress={() => setTab(m.capability.slug)}
              accessibilityRole="tab"
              accessibilityState={{ selected: m === manifest }}
            >
              <Text style={[styles.tabText, { color: m === manifest ? theme.foreground : theme.muted }]} numberOfLines={1}>
                {m.capability.name.replace(/^Nexus\s+/, "")}
              </Text>
            </Pressable>
          ))}
        </View>
      ) : null}
      {manifest?.panel.kind === "pages" ? <PagesPanel key={manifest.capability.slug} manifest={manifest} theme={theme} /> : null}
    </>
  );
}

/** Panel kind `pages`: the run form from the manifest's inputs, and the runs (polled while one runs). */
function PagesPanel({ manifest, theme }: { manifest: ModuleManifest; theme: Theme }) {
  const core = useCore();
  const product = manifest.product.slug;
  const capability = manifest.capability.slug;
  const name = manifest.capability.name;
  const [pages, setPages] = useState<RunsPage[] | null>(null);
  const [newest, setNewest] = useState<RunsPage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreError, setMoreError] = useState<string | null>(null);
  const [all, setAll] = useState(false);
  const [canRun, setCanRun] = useState<boolean>();
  const [values, setValues] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  // At the accessibility text sizes a run's cost goes to the line under its repository, which keeps the row's width.
  const stacked = isAccessibilitySize(useWindowDimensions().fontScale);

  // A paged capability (Nexus Review) loads the newest page here, and older pages on "Load more runs".
  const load = useCallback(() => {
    core
      .moduleReports(product, capability, pageQuery(manifest))
      .then((first) => {
        setNewest(first);
        setPages((old) => withFirstPage(old, first));
        setError(null);
      })
      .catch((e: Error) => setError(e.message));
  }, [core, product, capability, manifest]);
  useFocusEffect(load);
  const runs = pages ? pageRuns(pages) : null;
  const cursor = nextCursor(pages);
  const loadMore = () => {
    if (!cursor || loadingMore) return;
    setLoadingMore(true);
    setMoreError(null);
    core
      .moduleReports(product, capability, pageQuery(manifest, cursor))
      .then((page) => setPages((old) => [...(old ?? []), page]))
      .catch((e: Error) => setMoreError(e.message))
      .finally(() => setLoadingMore(false));
  };
  // Polling reads only the newest page, as Nexus UI's does: an older page loaded once never refreshes, so a run it
  // showed as running would keep the poll alive after it finished.
  const running = newest?.runs.some((run) => run.status === "running");
  useEffect(() => {
    if (!running) return;
    const timer = setInterval(load, POLL_MS);
    return () => clearInterval(timer);
  }, [running, load]);

  useEffect(() => {
    core
      .me()
      .then((me) => setCanRun(me.permissions?.includes("products.use") ?? false))
      .catch(() => setCanRun(false));
  }, [core, manifest]);

  const openRun = (id: string) => router.push({ pathname: "/run/[id]", params: { id, product, capability } });
  const start = () => {
    setBusy(true);
    setFailure(null);
    core
      .moduleVerb(product, capability, "run", values)
      .then((run) => {
        load();
        openRun(run.id);
      })
      .catch((e: Error) => setFailure(e.message))
      .finally(() => setBusy(false));
  };
  const ready = manifest.inputs.every((input) => !input.required || values[input.name]?.trim());
  const shown = all ? runs : runs?.slice(0, VISIBLE_RUNS);

  return (
    <ScrollView contentContainerStyle={styles.screen} keyboardShouldPersistTaps="handled" refreshControl={<RefreshControl refreshing={false} onRefresh={load} />}>
      <View style={[styles.card, { backgroundColor: theme.card, borderColor: theme.border }]}>
        {canRun === false ? (
          <Text style={[styles.body, { color: theme.muted }]}>Only the owner can start a {name} run. Every run is below.</Text>
        ) : null}
        {manifest.inputs.map((input) => (
          <View key={input.name} style={styles.field}>
            <Text style={[styles.label, { color: theme.muted }]}>
              {input.label}
              {input.required ? "" : " · optional"}
            </Text>
            <TextInput
              style={[styles.input, { color: theme.foreground, backgroundColor: theme.userBubble }]}
              value={values[input.name] ?? ""}
              onChangeText={(value) => setValues((v) => ({ ...v, [input.name]: value }))}
              placeholder={input.placeholder}
              placeholderTextColor={theme.muted}
              accessibilityLabel={input.label}
              autoCapitalize="none"
              autoCorrect={false}
              keyboardType={input.name === "repo" ? "url" : "default"}
              editable={canRun !== false}
              returnKeyType="go"
              onSubmitEditing={() => canRun && ready && !busy && start()}
            />
          </View>
        ))}
        {manifest.inputs.some((input) => input.name === "repo") && canRun !== false ? (
          <Text style={[styles.hint, { color: theme.muted }]}>A public repository by its URL; a private one once it is linked through the Nexus GitHub App.</Text>
        ) : null}
        <Pressable
          style={[styles.run, { backgroundColor: theme.primary }, (!canRun || !ready || busy) && { opacity: 0.4 }]}
          onPress={start}
          disabled={!canRun || !ready || busy}
          accessibilityRole="button"
        >
          {busy ? <ActivityIndicator color={theme.onPrimary} /> : <Text style={[styles.runText, { color: theme.onPrimary }]}>Run {name}</Text>}
        </Pressable>
        {failure ? <Text style={[styles.body, { color: theme.danger }]}>{failure}</Text> : null}
      </View>

      <Text style={[styles.section, { color: theme.tertiary }]}>
        Runs{runs?.length ? ` · ${running ? "Running · " : ""}${runs.length}${cursor ? " so far" : ""}` : ""}
      </Text>
      {error && !runs ? <Text style={[styles.note, { color: theme.danger }]}>The runs did not load. {error}</Text> : null}
      {runs === null && !error ? <ActivityIndicator style={styles.loading} color={theme.muted} /> : null}
      {runs?.length === 0 ? (
        <Text style={[styles.note, { color: theme.muted }]}>{canRun === false ? `No runs yet. The owner starts the first.` : "No runs yet. Start the first above."}</Text>
      ) : null}
      {shown?.length ? (
        <View style={[styles.group, { backgroundColor: theme.card, borderColor: theme.border }]}>
          {shown.map((run, n) => (
            <Pressable
              accessibilityRole="button"
              key={run.id}
              android_ripple={ripple(theme)}
              style={({ pressed }) => [styles.row, n > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderColor: theme.border }, pressed && pressedFill(theme)]}
              onPress={() => openRun(run.id)}
              accessibilityLabel={`${run.status} run on ${repoName(Object.values(run.input).join(" "))}`}
            >
              <View style={[styles.rowHead, stacked && styles.wrap]}>
                <Status status={run.status} theme={theme} />
                <Text style={[styles.target, stacked && styles.fullWidth, { color: theme.foreground }]} numberOfLines={stacked ? 3 : 1}>
                  {repoName(Object.values(run.input).join(" "))}
                </Text>
                {stacked ? null : <Text style={[styles.money, { color: theme.muted }]}>{usd(run.costUsd)}</Text>}
              </View>
              {stacked ? <Text style={[styles.money, { color: theme.muted }]}>{usd(run.costUsd)}</Text> : null}
              <Text style={[styles.meta, { color: theme.muted }]} numberOfLines={stacked ? undefined : 1}>
                {new Date(run.startedAt).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })} ·{" "}
                {run.status === "running" ? "running" : duration(run.startedAt, run.finishedAt)}
              </Text>
            </Pressable>
          ))}
        </View>
      ) : null}
      {runs && runs.length > VISIBLE_RUNS ? (
        <Pressable onPress={() => setAll(!all)} style={styles.more} accessibilityRole="button">
          <Text style={[styles.body, { color: theme.primary }]}>{all ? "Show fewer" : `Show all ${runs.length} runs${cursor ? " so far" : ""}`}</Text>
        </Pressable>
      ) : null}
      {cursor && runs && (all || runs.length <= VISIBLE_RUNS) ? (
        <Pressable onPress={loadMore} disabled={loadingMore} style={styles.more} accessibilityRole="button">
          {loadingMore ? <ActivityIndicator color={theme.muted} /> : <Text style={[styles.body, { color: theme.primary }]}>Load more runs</Text>}
        </Pressable>
      ) : null}
      {moreError ? <Text style={[styles.note, { color: theme.danger }]}>The older runs did not load. {moreError}</Text> : null}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { padding: 16, gap: 12 },
  loading: { padding: 24 },
  note: { ...text.sm, fontFamily: font.regular, padding: 16 },
  tabs: { flexDirection: "row", margin: 16, marginBottom: 0, padding: 3, borderRadius: 999 },
  tab: { flex: 1, minHeight: 36, borderRadius: 999, alignItems: "center", justifyContent: "center", paddingHorizontal: 8 },
  tabText: { ...text.sm, fontFamily: font.medium },
  card: { borderRadius: 24, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, padding: 16, gap: 12 },
  field: { gap: 6 },
  label: { ...text.xs, fontFamily: font.medium },
  input: { ...text.base, fontFamily: font.regular, minHeight: 44, borderRadius: 12, paddingHorizontal: 12 },
  hint: { ...text.xs, fontFamily: font.regular },
  body: { ...text.sm, fontFamily: font.regular },
  // Its corners are half its least height: a pill at one line, a rounded card when its label wraps at the larger text
  // sizes, never a circle that cuts the label.
  run: { minHeight: 44, borderRadius: 22, alignItems: "center", justifyContent: "center", paddingHorizontal: 24, paddingVertical: 10 },
  runText: { ...text.base, fontFamily: font.medium },
  section: { ...text.xs, fontFamily: font.medium, marginLeft: 4, marginTop: 8 },
  group: { borderRadius: 24, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth, overflow: "hidden" },
  row: { paddingHorizontal: 16, paddingVertical: 12, gap: 4 },
  rowHead: { flexDirection: "row", alignItems: "center", gap: 10 },
  wrap: { flexWrap: "wrap" },
  fullWidth: { flexBasis: "100%" },
  target: { ...text.base, fontFamily: mono, fontSize: 14, flex: 1 },
  money: { fontFamily: mono, fontSize: 13 },
  meta: { ...text.xs, fontFamily: font.regular },
  more: { alignItems: "center", paddingVertical: 8 },
});
