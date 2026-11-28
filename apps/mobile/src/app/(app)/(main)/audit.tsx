import type { AuditQuery, AuditResponse } from "@nexus/kit";
import { CHROME_FONT_SCALE } from "@/text-size";
import { Stack } from "expo-router";
import { useEffect, useState } from "react";
import { Platform, Pressable, StyleSheet, Text, View } from "react-native";
import { useCore } from "@/auth";
import { actionText, actorName, AREAS, byDay, keepEvents, RANGES, SCOPE, SURFACE, targetName } from "@/audit";
import { TextInput } from "@/controls";
import { Icon } from "@/icon";
import { Busy, Empty, Footnote, LargeTitle, Loading, Page, Row, Section } from "@/list";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { font, text, useTheme } from "@/theme";

const ANDROID = Platform.OS === "android";
const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

/**
 * Audit, as brand v5's P29·E draws it on the phone: the title in the list, Search (who made the change: an email or a
 * user id) beside the period, then the changes under their days, each saying what changed, when, who and from where. A
 * change opens its own page (`audit-event/[id]`) with the record before and after and Copy as JSON. The period and
 * the area are chosen in one sheet from the period's pill (P105·D: editors as sheets). Owners and admins see
 * everything, a team lead their teams', everyone else their own. The board's Denied mark is left out: Gateway's log
 * records changes, not refused calls.
 */
export default function Audit() {
  const core = useCore();
  const theme = useTheme();
  const [since, setSince] = useState<string>("7d");
  const [action, setAction] = useState<string>("");
  const [actor, setActor] = useState("");
  const [typed, setTyped] = useState("");
  const [choosing, setChoosing] = useState(false);
  const [first, setFirst] = useState<AuditResponse | null>(null);
  const [older, setOlder] = useState<AuditResponse[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const query: AuditQuery = { ...(since ? { since } : {}), ...(action ? { action } : {}), ...(actor ? { actor } : {}) };
  const key = JSON.stringify(query);

  useEffect(() => {
    let live = true;
    setFirst(null);
    setOlder([]);
    setError(null);
    core.audit({ ...query, limit: 50 }).then((r) => live && setFirst(r), (e: Error) => live && setError(e.message));
    return () => {
      live = false;
    };
  }, [core, key, attempt]);

  const events = [...(first?.events ?? []), ...older.flatMap((p) => p.events)];
  keepEvents(events);
  const cursor = (older.at(-1) ?? first)?.nextCursor;
  const more = () => {
    if (!cursor || loadingMore) return;
    setLoadingMore(true);
    core
      .audit({ ...query, limit: 50, cursor })
      .then((page) => setOlder((p) => [...p, page]), (e: Error) => setError(e.message))
      .finally(() => setLoadingMore(false));
  };
  const range = RANGES.find((r) => r.id === since)?.label ?? "Everything";
  const area = AREAS.find((a) => a.id === action);
  const pill = action && area ? `${range} · ${area.label}` : range;
  const days = byDay(events);
  const search = () => typed.trim() !== actor && setActor(typed.trim());

  return (
    <Page onRefresh={() => setAttempt((n) => n + 1)}>
      <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>Audit</LargeTitle>
      <View style={styles.filters}>
        <View style={[styles.search, { backgroundColor: theme.fill }]}>
          <Icon name="search" size={15} color={theme.tertiary} />
          <TextInput
            style={[styles.searchText, { color: theme.foreground }]}
            value={typed}
            onChangeText={setTyped}
            onSubmitEditing={search}
            onBlur={search}
            placeholder="Search by who: an email"
            placeholderTextColor={theme.tertiary}
            autoCapitalize="none"
            autoCorrect={false}
            keyboardType="email-address"
            returnKeyType="search"
            clearButtonMode="while-editing"
            accessibilityLabel="Who made the change"
          />
        </View>
        <Pressable style={[styles.pill, { backgroundColor: theme.fill }]} onPress={() => setChoosing(true)} accessibilityRole="button" accessibilityLabel={`Showing ${pill}. Change`} hitSlop={6}>
          <Text style={[styles.pillText, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE}>
            {pill}
          </Text>
        </Pressable>
      </View>

      {error && !first ? <Footnote tone="danger">{error}</Footnote> : null}
      {!first && !error ? <Loading /> : null}
      {first ? <Footnote>{SCOPE[first.scope]}</Footnote> : null}
      {first && events.length === 0 ? (
        <Section>
          <Empty>
            No changes match: nothing {since ? "in this period" : "yet"}
            {action || actor ? " for these filters" : ""}.
          </Empty>
        </Section>
      ) : null}
      {days.map((d, n) => (
        <Section key={d.title} title={d.title} footer={n === days.length - 1 && !cursor ? `${events.length} change${events.length === 1 ? "" : "s"}, the oldest included.` : undefined}>
          {d.events.map((event) => (
            <Row
              key={event.id}
              label={`${actionText(event.action)}: ${targetName(event)}`}
              detail={[time(event.at), actorName(event), SURFACE[event.surface]].join(" · ")}
              href={{ pathname: "/audit-event/[id]", params: { id: event.id } }}
            />
          ))}
        </Section>
      ))}
      {events.length && cursor ? (
        <Section>
          <Row label={loadingMore ? "Loading…" : "Show older"} tone="primary" center disabled={loadingMore} accessory={loadingMore ? <Busy /> : undefined} onPress={more} />
        </Section>
      ) : null}

      {/* The period and the area, in one sheet. */}
      <Sheet visible={choosing} onClose={() => setChoosing(false)}>
        <SheetTitle>When</SheetTitle>
        {RANGES.map((r) => (
          <SheetRow
            key={r.id || "all"}
            label={r.id === since ? `${r.label} ✓` : r.label}
            tone={r.id === since ? "primary" : "normal"}
            onPress={() => {
              setSince(r.id);
              setChoosing(false);
            }}
          />
        ))}
        <SheetTitle>What</SheetTitle>
        {AREAS.map((a) => (
          <SheetRow
            key={a.id || "all"}
            label={a.id === action ? `${a.label} ✓` : a.label}
            tone={a.id === action ? "primary" : "normal"}
            onPress={() => {
              setAction(a.id);
              setChoosing(false);
            }}
          />
        ))}
      </Sheet>
    </Page>
  );
}

// The board's bar: the search field and the period's pill, each 32 pt, on the fill.
const styles = StyleSheet.create({
  filters: { flexDirection: "row", alignItems: "center", gap: 8 },
  search: { flex: 1, flexDirection: "row", alignItems: "center", gap: 6, minHeight: 36, paddingHorizontal: 10, borderRadius: 10, borderCurve: "continuous" },
  searchText: { flex: 1, ...text.secondary, fontFamily: font.regular, paddingVertical: ANDROID ? 4 : 8 },
  pill: { minHeight: 36, maxWidth: "45%", paddingHorizontal: 12, borderRadius: 18, justifyContent: "center" },
  pillText: { ...text.sm, fontFamily: font.medium },
});
