import type { UsageReport } from "@nexus/kit";
import { Stack, useLocalSearchParams } from "expo-router";
import { useEffect, useMemo, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useCore } from "@/auth";
import { chartDays, count, GROUPINGS, NOUNS, RANGES, rangeOf, reportFigures, seriesOf, sourceLabel, spentText, stackOf, type Grouping } from "@/analytics";
import { Empty, Footnote, Hero, Inset, LargeTitle, Loading, Page, Row, Section, Segmented, Share } from "@/list";
import { font, text, useTheme, type Theme } from "@/theme";

const CHART = 140;

/**
 * Analytics, as desktop's: model spend by team, person, model or product over 7, 30 or 90 days, from Gateway's
 * ledger. By team, spend rolls up the tree: pick a depth, tap a team with child teams to see them, or a team without
 * to see its people. Usage's teams open here on one (`under`, its child teams; `team`, its people). Its title is in the
 * list, as brand v5's P105·D sets an admin page.
 */
export default function Analytics() {
  const core = useCore();
  const theme = useTheme();
  const opened = useLocalSearchParams<{ team?: string; under?: string }>();
  const [by, setBy] = useState<Grouping>(opened.team ? "person" : "team");
  const [days, setDays] = useState<(typeof RANGES)[number]>(30);
  const [depth, setDepth] = useState("1");
  const [under, setUnder] = useState<string | undefined>(opened.under);
  const [team, setTeam] = useState<string | undefined>(opened.team);
  const [deepest, setDeepest] = useState(1);
  const [report, setReport] = useState<UsageReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    core.teams().then((r) => setDeepest(Math.max(1, ...r.teams.map((t) => t.depth))), () => undefined);
  }, [core]);

  useEffect(() => {
    let live = true;
    setError(null);
    core
      .usage({ groupBy: by, ...(team ? { team } : {}), ...(by === "team" ? (under ? { under } : depth !== "all" ? { depth } : {}) : {}), ...rangeOf(days) })
      .then((r) => live && setReport(r), (e: Error) => live && setError(e.message));
    return () => {
      live = false;
    };
  }, [core, by, days, depth, under, team, attempt]);

  const pickBy = (next: Grouping) => {
    setBy(next);
    if (next !== "team") setUnder(undefined);
  };

  return (
    <Page onRefresh={() => setAttempt((n) => n + 1)}>
      <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>Analytics</LargeTitle>
      <View style={styles.controls}>
        <Segmented label="Group by" options={GROUPINGS.map((g) => ({ id: g.id, label: g.label }))} value={by} onChange={pickBy} />
        <Segmented label="Range" options={RANGES.map((r) => ({ id: String(r), label: `${r} days` }))} value={String(days)} onChange={(v) => setDays(Number(v) as (typeof RANGES)[number])} />
        {by === "team" && !under && deepest > 1 ? (
          <Segmented
            label="Depth"
            options={[...Array.from({ length: Math.min(deepest, 4) }, (_, n) => ({ id: String(n + 1), label: n === 0 ? "Top" : String(n + 1) })), { id: "all", label: "Every team" }]}
            value={depth}
            onChange={setDepth}
          />
        ) : null}
        {by === "team" && under ? (
          <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.crumbs}>
            <Crumb theme={theme} label="All teams" onPress={() => setUnder(undefined)} />
            {(report?.under?.path ?? []).map((step) => (
              <Crumb key={step.id} theme={theme} label={step.name} onPress={() => setUnder(step.slug)} />
            ))}
            <Text style={[styles.strong, { color: theme.foreground }]}>› {report?.under?.name ?? under}</Text>
          </ScrollView>
        ) : null}
        {team ? (
          <Pressable style={[styles.filter, { backgroundColor: theme.userBubble }]} onPress={() => setTeam(undefined)} accessibilityRole="button" accessibilityLabel={`Team ${report?.team?.name ?? team}. Show every team`}>
            <Text style={[styles.body, { color: theme.foreground }]}>
              Team <Text style={styles.strong}>{report?.team?.name ?? team}</Text> ×
            </Text>
          </Pressable>
        ) : null}
      </View>

      {error && !report ? <Footnote tone="danger">{error}</Footnote> : null}
      {!report && !error ? <Loading /> : null}
      {report ? (
        <Report
          report={report}
          theme={theme}
          onTeam={(slug) => {
            setTeam(slug);
            setBy("person");
            setUnder(undefined);
          }}
          onDrill={setUnder}
        />
      ) : null}
    </Page>
  );
}

function Report({ report, theme, onTeam, onDrill }: { report: UsageReport; theme: Theme; onTeam: (slug: string) => void; onDrill: (slug: string) => void }) {
  const { totals, groups } = report;
  const by = report.groupBy as Grouping;
  const label = (key: string, raw: string) => (by === "product" ? sourceLabel(key) : raw);
  const colors = [theme.primary, theme.muted, theme.chartMuted];
  const { series, restKeys } = useMemo(() => seriesOf(groups.map((g) => ({ key: g.key, label: label(g.key, g.label) })), colors, theme.border), [groups, theme]);
  const colorOf = (key: string) => (restKeys.has(key) ? theme.border : (series.find((s) => s.key === key)?.color ?? theme.border));
  const active = report.series.filter((day) => day.calls > 0).length;
  const figures = reportFigures(totals);

  return (
    <>
      <Section footer={report.scope === "member" ? "You see your usage and your teams'. Owners and admins see the whole organization." : undefined}>
        <Hero label="Spent" value={figures.spent} note={`${report.from} → ${report.to}`} />
        <Row label="Calls" detail={`${active} of ${report.series.length} days active`} value={count(totals.calls)} />
        <Row label="Tokens in / out" detail="As the providers reported them" value={`${count(totals.inputTokens)} / ${count(totals.outputTokens)}`} />
        <Row
          label="Average a call"
          detail={`${groups.length} ${NOUNS[by]?.[groups.length === 1 ? 0 : 1] ?? "groups"} with usage`}
          value={figures.average}
        />
      </Section>

      <Section title="Spend by day · UTC">
        {totals.calls === 0 ? <Empty>No usage in this range. Every metered call shows here.</Empty> : <Chart report={report} series={series} restKeys={restKeys} theme={theme} />}
      </Section>

      <Section
        title={`By ${GROUPINGS.find((g) => g.id === by)?.label.toLowerCase() ?? "group"}`}
        footer={by === "team" ? (report.depth ? "Each team includes the teams under it." : undefined) : "Each is its share of the range's spend."}
      >
        {groups.length === 0 ? <Empty>Nothing to break down yet.</Empty> : null}
        {groups.map((g) => {
          const tappable = by === "team" && g.slug && !g.deleted && !g.direct;
          return (
            <Row
              key={g.key}
              leading={<View style={[styles.swatch, { backgroundColor: colorOf(g.key) }]} />}
              label={label(g.key, g.label)}
              monoLabel={by === "model"}
              detail={[
                `${Math.round(g.share * 100)}%`,
                `${count(g.calls)} calls`,
                g.level && !g.direct ? g.level : undefined,
                g.deleted ? `deleted team ${g.slug}` : undefined,
                g.direct ? "the team itself, not the teams under it" : undefined,
              ]
                .filter(Boolean)
                .join(" · ")}
              value={spentText(g)}
              valueTone="foreground"
              chevron={!!tappable && g.hasChildren}
              onPress={tappable ? () => (g.hasChildren ? onDrill(g.slug!) : onTeam(g.slug!)) : undefined}
              accessibilityHint={tappable ? (g.hasChildren ? "Opens its child teams" : "This team, by person") : undefined}
            >
              <Share share={g.share} />
            </Row>
          );
        })}
        {groups.length ? <Row label={`Total · ${count(totals.calls)} calls`} value={figures.spent} valueTone="foreground" /> : null}
      </Section>
    </>
  );
}

/** Daily spend as stacked columns; tap a day for its figures (the latest is shown until then). */
function Chart({ report, series, restKeys, theme }: { report: UsageReport; series: { key: string; label: string; color: string }[]; restKeys: Set<string>; theme: Theme }) {
  const { days, max } = useMemo(() => chartDays(report.series), [report.series]);
  const [picked, setPicked] = useState<number>();
  const at = picked ?? days.length - 1;
  const focus = days[at];
  return (
    <Inset>
      {focus ? (
        <Text style={[styles.body, { color: theme.foreground }]}>
          <Text style={styles.strong}>{focus.day.day}</Text> · {focus.spent} · {count(focus.day.calls)} calls
        </Text>
      ) : null}
      <View style={[styles.plot, { height: CHART }]}>
        {days.map(({ day, spent, height, any, usd }, n) => {
          const stack = stackOf(day, series, restKeys);
          return (
            <Pressable key={day.day} style={styles.col} onPress={() => setPicked(n)} accessibilityLabel={`${day.day}: ${spent}, ${day.calls} calls`}>
              <View style={[styles.bar, { height: Math.max(height * CHART, any ? 2 : 0), opacity: n === at ? 1 : 0.75 }]}>
                {(stack.length ? stack : [{ key: "all", usd, color: theme.primary }]).map((part) => (
                  <View key={part.key} style={{ flex: part.usd, backgroundColor: part.color }} />
                ))}
              </View>
            </Pressable>
          );
        })}
      </View>
      <View style={styles.axis}>
        <Text style={[styles.meta, { color: theme.muted }]}>{days[0]?.day.day}</Text>
        <Text style={[styles.meta, { color: theme.muted }]}>max {max}</Text>
        <Text style={[styles.meta, { color: theme.muted }]}>{days[days.length - 1]?.day.day}</Text>
      </View>
      {series.length > 1 ? (
        <View style={styles.legend}>
          {series.map((s) => (
            <View key={s.key} style={styles.legendItem}>
              <View style={[styles.swatch, { backgroundColor: s.color }]} />
              <Text style={[styles.meta, { color: theme.muted }]} numberOfLines={1}>
                {s.label}
              </Text>
            </View>
          ))}
        </View>
      ) : null}
    </Inset>
  );
}

function Crumb({ theme, label, onPress }: { theme: Theme; label: string; onPress: () => void }) {
  return (
    <Pressable onPress={onPress} accessibilityRole="button" hitSlop={8}>
      <Text style={[styles.body, { color: theme.primary }]}>{label} ›</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  controls: { gap: 8 },
  crumbs: { gap: 8, alignItems: "center", paddingHorizontal: 4 },
  filter: { alignSelf: "flex-start", paddingHorizontal: 12, minHeight: 32, justifyContent: "center", borderRadius: 999 },
  body: { ...text.sm, fontFamily: font.regular },
  strong: { ...text.sm, fontFamily: font.medium },
  meta: { ...text.xs, fontFamily: font.regular },
  swatch: { width: 10, height: 10, borderRadius: 3 },
  plot: { flexDirection: "row", alignItems: "flex-end", gap: 2 },
  col: { flex: 1, height: "100%", justifyContent: "flex-end" },
  bar: { borderTopLeftRadius: 2, borderTopRightRadius: 2, overflow: "hidden", flexDirection: "column-reverse" },
  axis: { flexDirection: "row", justifyContent: "space-between" },
  legend: { flexDirection: "row", flexWrap: "wrap", gap: 12 },
  legendItem: { flexDirection: "row", alignItems: "center", gap: 6, maxWidth: "100%" },
});
