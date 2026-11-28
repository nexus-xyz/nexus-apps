import { segments, type SearchGroup, type SearchResult } from "@nexus/kit";
import type { Href } from "expo-router";
import { useEffect, useMemo, useRef, useState, type ComponentProps } from "react";
import { ActivityIndicator, KeyboardAvoidingView, Platform, Pressable, SectionList, StyleSheet, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { ago, spokenAgo } from "@/ago";
import { askNexusHref, askNexusLabel } from "@/assistant-handoff";
import { useAuth, useCore } from "@/auth";
import { TextInput } from "@/controls";
import { sessionDetail, sessionHref, sessionKey, sessionTitle, useSessionHistory, type UnifiedSession } from "@/session-history";
import type { TitleState } from "@/code-sessions";
import { rowState, stateWord } from "@/drawer-groups";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { open } from "@/markdown";
import { NavIcon, NavLabel, NavRow, NavSection, SessionMark } from "@/nav-row";
import { useOrgs } from "@/org-switcher";
import { orgLabel } from "@/orgs";
import { usePermissions } from "@/use-permissions";
import { findSettings, settingsFor } from "@/settings-map";
import { absoluteLink } from "@/share";
import { ACCESSIBILITY_FONT_SCALE } from "@/text-size";
import { font, text, useTheme, type Theme } from "@/theme";

const PAUSE_MS = 250;

/**
 * Where the drawer's other rows went: still found here by name (P94·A keeps the drawer short). The Assistant is also
 * at the top of the drawer, in a build with the agent API (`drawer-places.ts`).
 */
const PLACES: { label: string; icon: ComponentProps<typeof NavIcon>["name"]; href: Href; words: string[] }[] = [
  { label: "Connect computer", icon: "agents", href: "/connect", words: ["computer", "remote", "scan", "phone"] },
  { label: "Assistant", icon: "mark", href: "/agent", words: ["agent", "nexus", "slack"] },
  { label: "Agents", icon: "agents", href: "/agent-sessions", words: ["coding", "runs", "sessions"] },
];

type Item =
  | { kind: "ask"; label: string; query: string }
  | { kind: "recent"; session: UnifiedSession }
  | { kind: "result"; result: SearchResult }
  | { kind: "page"; label: string; icon: ComponentProps<typeof NavIcon>["name"]; href: Href; where?: string };
type Group = { title: string; data: Item[] };

const ENTITY_ICON: Partial<Record<SearchResult["entity"], ComponentProps<typeof Icon>["name"]>> = {
  artifact: "artifacts",
  member: "person",
  invitation: "mail",
  team: "members",
  model: "models",
  policy: "privacy",
};

/**
 * Search in the drawer or at its deep link: the field at the top with Cancel, then what matched in
 * groups, each row drawn as the drawer's. Sessions and artifacts, people, teams and models come from the Gateway
 * (`/v1/search`, as Nexus UI's palette), after a short pause, the previous request cancelled; the settings pages and the
 * drawer's other places are matched here as you type, each with where it lives ("Acme › Controls"). A session or an
 * artifact opens here; the rest open in Nexus UI until the phone has their pages.
 *
 * With words typed and the Assistant in this build, the first row hands them to Nexus (A21·B e): Ask Nexus “dinner”
 * opens the Assistant, which sends them (`assistant-handoff.ts`).
 */
export function SearchPanel({ initialQuery = "", recentSessions, recentTitles, recentError, onClose, onNavigate }: {
  initialQuery?: string;
  recentSessions?: UnifiedSession[];
  recentTitles?: Record<string, TitleState>;
  recentError?: string;
  onClose: () => void;
  onNavigate: (href: Href) => void;
}) {
  const core = useCore();
  const { ui, orgName, agent } = useAuth();
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const { current } = useOrgs(false);
  // Search finds no page you have no access to, as Settings shows none.
  const permissions = usePermissions();
  const history = useSessionHistory(core, 50);
  useEffect(() => { if (!recentSessions) history.refresh(); }, [core, recentSessions, history.refresh]);
  const recent = recentSessions ?? history.sessions;
  const titles = recentTitles ?? history.titles;
  const [query, setQuery] = useState(initialQuery);
  const [groups, setGroups] = useState<SearchGroup[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inFlight = useRef<AbortController | null>(null);

  useEffect(() => {
    const q = query.trim();
    inFlight.current?.abort();
    setError(null);
    setGroups(null);
    if (!q) {
      setBusy(false);
      return;
    }
    const controller = new AbortController();
    inFlight.current = controller;
    setBusy(true);
    const timer = setTimeout(() => {
      core
        .search({ q, limit: 8 }, controller.signal)
        .then((r) => {
          if (controller.signal.aborted) return;
          setGroups(r.groups);
          setError(null);
        })
        .catch((e: Error) => !controller.signal.aborted && setError(e.message))
        .finally(() => !controller.signal.aborted && setBusy(false));
    }, PAUSE_MS);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [query, core]);

  const account = current ? orgLabel(current) : (orgName ?? "");
  const personal = current ? current.kind === "personal" : orgName === "Personal";
  // The board's order: Sessions, Settings, Artifacts, then the rest the Gateway found, then the drawer's other places.
  const sections = useMemo<Group[]>(() => {
    const words = query.trim().toLowerCase();
    if (!words) {
      const sessions = recent;
      return sessions.length ? [{ title: "Recents", data: sessions.map((session) => ({ kind: "recent", session })) }] : [];
    }
    const remote = (groups ?? []).map((g): Group => ({ title: groupTitle(g), data: g.results.map((result) => ({ kind: "result", result })) }));
    const sessions = remote.filter((_, n) => groups?.[n]?.results[0]?.entity === "session");
    const native: Group = { title: "Computer sessions · opened titles", data: recent.filter((s) => s.source === "synced" && titles[s.id]?.state === "open" && words.split(/\s+/).every((w) => `${sessionTitle(s, titles)} ${sessionDetail(s, titles)}`.toLowerCase().includes(w))).map((session) => ({ kind: "recent", session })) };
    const rest = remote.filter((_, n) => groups?.[n]?.results[0]?.entity !== "session");
    const settings: Group = {
      title: "Settings",
      data: findSettings(words, settingsFor({ personal, hidden: [...(ui ? [] : ["/docs"]), ...(agent ? [] : ["/assistant-settings"])], permissions }), account, personal).map((p) => ({ kind: "page", label: p.label, icon: p.icon, href: p.href as Href, where: p.where })),
    };
    const places: Group = {
      title: "Go to",
      data: PLACES.filter((p) => words.split(/\s+/).every((w) => [p.label.toLowerCase(), ...p.words].join(" ").includes(w))).map((p) => ({ kind: "page", ...p })),
    };
    const label = agent ? askNexusLabel(query) : undefined;
    const ask: Group = { title: "Assistant", data: label ? [{ kind: "ask", label, query }] : [] };
    return [ask, ...sessions, native, settings, ...rest, places].filter((s) => s.data.length > 0);
  }, [groups, query, personal, account, ui, permissions, recentSessions, recent, titles, agent]);

  const go = (item: Item) => {
    if (item.kind === "ask") return onNavigate(askNexusHref(item.query));
    if (item.kind === "page") return onNavigate(item.href);
    if (item.kind === "recent") return onNavigate(sessionHref(item.session));
    const r = item.result;
    if (r.entity === "session") onNavigate({ pathname: "/session/[id]", params: { id: r.id } });
    else if (r.entity === "artifact") onNavigate({ pathname: "/artifact/[id]", params: { id: r.id } });
    else if (ui) {
      haptic("tap");
      open(absoluteLink(r.url, ui));
    }
  };

  const empty = busy ? null : query.trim() ? `Nothing matches “${query.trim()}”.` : "Search your sessions, settings and artifacts.";
  return (
    <KeyboardAvoidingView behavior={Platform.OS === "ios" ? "padding" : "height"} style={[styles.screen, { paddingTop: insets.top + 8 }]}>
      <View style={styles.head}>
        <View style={[styles.field, { backgroundColor: theme.fill }]}>
          <Icon name="search" size={16} color={theme.muted} />
          <TextInput
            style={[styles.input, { color: theme.foreground }]}
            value={query}
            onChangeText={setQuery}
            placeholder="Search sessions and more"
            placeholderTextColor={theme.muted}
            autoFocus
            autoCorrect={false}
            autoCapitalize="none"
            returnKeyType="search"
            clearButtonMode="while-editing"
            maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}
            accessibilityLabel="Search"
          />
          {busy ? <ActivityIndicator size="small" color={theme.muted} /> : null}
        </View>
        <Pressable onPress={() => { haptic("tap"); onClose(); }} style={({ pressed }) => [styles.cancel, { backgroundColor: theme.fill }, pressed && styles.pressed]} accessibilityRole="button" accessibilityLabel="Cancel search">
          <Icon name="close" size={17} color={theme.muted} />
        </Pressable>
      </View>
      <SectionList
        style={styles.list}
        contentContainerStyle={{ paddingBottom: insets.bottom + 16 }}
        sections={sections}
        keyExtractor={(item) => item.kind === "ask" ? "ask" : item.kind === "recent" ? `recent:${sessionKey(item.session)}` : item.kind === "page" ? `page:${String(item.href)}` : `${item.result.type}:${item.result.id}:${item.result.messageId ?? ""}`}
        keyboardShouldPersistTaps="handled"
        keyboardDismissMode="on-drag"
        stickySectionHeadersEnabled={false}
        ListHeaderComponent={<View>{error || recentError || (!recentSessions && history.error) ? <Text style={[styles.note, { color: theme.danger }]}>{error ?? recentError ?? history.error}</Text> : null}{query.trim() ? <Text style={[styles.note, { color: theme.muted }]}>Computer search covers loaded titles opened on this phone.</Text> : null}</View>}
        ListEmptyComponent={empty ? <Text style={[styles.note, styles.center, { color: theme.muted }]}>{empty}</Text> : undefined}
        renderSectionHeader={({ section }) => <NavSection>{section.title}</NavSection>}
        renderItem={({ item }) =>
          item.kind === "ask" ? (
            <NavRow leading={<NavIcon name="mark" />} label={item.label} onPress={() => go(item)} accessibilityHint="Opens the Assistant and sends it" />
          ) : item.kind === "recent" ? (
            <NavRow leading={<SessionMark state={rowState(item.session, titles)} />} label={sessionTitle(item.session, titles)} detail={sessionDetail(item.session, titles)} meta={ago(item.session.updatedAt)} stateWord={stateWord(rowState(item.session, titles))} onPress={() => go(item)} accessibilityLabel={`${sessionTitle(item.session, titles)}, ${sessionDetail(item.session, titles)}, ${spokenAgo(item.session.updatedAt)}`} />
          ) : item.kind === "page" ? (
            <NavRow
              leading={<NavIcon name={item.icon} />}
              label={item.label}
              meta={item.where}
              onPress={() => go(item)}
              accessibilityLabel={[item.label, item.where?.replace("›", ",")].filter(Boolean).join(", ")}
            />
          ) : (
            <NavRow
              leading={item.result.entity === "session" ? <SessionMark /> : <NavIcon name={ENTITY_ICON[item.result.entity] ?? "file"} />}
              label={
                <NavLabel>
                  <Marked text={item.result.title || "Untitled"} ranges={item.result.titleMatches} theme={theme} />
                </NavLabel>
              }
              // A session's row is its title, as the drawer's (P94·B); under it only where a message matched. Others say
              // what they are (an email, a team's path).
              detail={
                item.result.snippet ? (
                  <Marked text={item.result.snippet.text} ranges={item.result.snippet.matches} theme={theme} />
                ) : item.result.entity === "session" ? undefined : (
                  item.result.subtitle
                )
              }
              onPress={() => go(item)}
              accessibilityLabel={[item.result.title || "Untitled", item.result.snippet?.text ?? item.result.subtitle].filter(Boolean).join(", ")}
            />
          )
        }
      />
    </KeyboardAvoidingView>
  );
}

/** The board's words for the Gateway's groups: sessions, not chats. */
function groupTitle(group: SearchGroup): string {
  const entity = group.results[0]?.entity;
  if (entity === "session") return "Sessions";
  if (entity === "artifact") return "Artifacts";
  return group.label;
}

/** Matched parts in the primary colour and medium weight (Nexus UI marks them). */
function Marked({ text: value, ranges, theme }: { text: string; ranges: [number, number][]; theme: Theme }) {
  return (
    <>
      {segments(value, ranges).map((part, n) =>
        part.match ? (
          <Text key={n} style={[styles.match, { color: theme.primary }]}>
            {part.text}
          </Text>
        ) : (
          part.text
        ),
      )}
    </>
  );
}

// The search field and Cancel have 44 pt touch targets. Its rows
// are the drawer's, 8 pt in.
const styles = StyleSheet.create({
  screen: { flex: 1 },
  head: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 12 },
  field: { flex: 1, flexDirection: "row", alignItems: "center", gap: 8, minHeight: 44, paddingHorizontal: 12, borderRadius: 22, borderCurve: "continuous" },
  input: { ...text.body, fontFamily: font.regular, flex: 1, paddingVertical: Platform.OS === "android" ? 6 : 9 },
  cancel: { width: 44, height: 44, alignItems: "center", justifyContent: "center", borderRadius: 22 },
  pressed: { opacity: 0.5 },
  list: { flex: 1, paddingHorizontal: 8, marginTop: 4 },
  match: { fontFamily: font.medium },
  note: { ...text.sm, fontFamily: font.regular, padding: 20 },
  center: { textAlign: "center" },
});
