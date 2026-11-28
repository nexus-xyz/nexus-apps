import type { LibraryOrder } from "@nexus/kit";
import * as Clipboard from "expo-clipboard";
import { router, useFocusEffect } from "expo-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, FlatList, Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { ago } from "@/ago";
import { useArtifacts, useAuth, useStored } from "@/auth";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { KIND_LABEL, kindOf, ofType, OWNERS, TYPES, type Kind, type Owner, type TypeFilter } from "@/library";
import { LargeTitle } from "@/list";
import { PressScale } from "@/press-scale";
import { absoluteLink, audienceLabel } from "@/share";
import { ShareSheet } from "@/share-sheet";
import { useToast } from "@/toast";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { font, text, useTheme, type Theme } from "@/theme";
import { ACCESSIBILITY_FONT_SCALE, isAccessibilitySize } from "@/text-size";

const PAGE = 50;
type Tab = Owner;

/**
 * A tile of any list: your page opens at `page/[id]`, a shared page or chat as its artifact. `kind` is what it is and
 * how its thumbnail is drawn: a doc (Markdown) as its lines, a site (HTML or React) as a page, a chat as its messages.
 */
type Row = { id: string; title: string; meta: string; shared: boolean; kind: Kind; link?: string; excerpt?: string };
type Batch = { rows: Row[]; cursor?: string };

const EMPTY: Record<Tab, string> = {
  mine: "No pages yet. Ask a chat for a page, or run nexus pages publish.",
  shared_with_me: "Nothing shared with you yet.",
  shared_by_me: "You haven't shared anything yet.",
};

export default function Pages() {
  const artifacts = useArtifacts();
  const theme = useTheme();
  // Two columns between the 16 pt margins, 12 pt apart; set, so a last tile alone keeps a column's width. At the
  // accessibility text sizes one column, where a title has more than a word's room.
  const { width, fontScale } = useWindowDimensions();
  const columns = isAccessibilitySize(fontScale) ? 1 : 2;
  const tileWidth = columns === 1 ? width - 32 : (width - 32 - 12) / 2;
  const { ui } = useAuth();
  const toast = useToast();
  const [tab, setTab] = useState<Tab>("mine");
  const [type, setType] = useState<TypeFilter>("all");
  const sortChoice = useStored<LibraryOrder>("choice", "artifacts:order");
  const [order, setOrder] = useState<LibraryOrder>(() => sortChoice.load() ?? "recent");
  const [chooseOrder, setChooseOrder] = useState(false);
  const [typed, setTyped] = useState("");
  const [query, setQuery] = useState("");
  const [picked, setPicked] = useState<Row>();
  const [sharing, setSharing] = useState<Row>();
  useEffect(() => {
    const timer = setTimeout(() => setQuery(typed.trim()), 220);
    return () => clearTimeout(timer);
  }, [typed]);
  const [items, setItems] = useState<Row[]>([]);
  const [cursor, setCursor] = useState<string | undefined>();
  const [loading, setLoading] = useState(true);
  // The pull's spinner, only for a pull: a reload on coming back would leave iOS's RefreshControl spinning.
  const [pulled, setPulled] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The pills filter what is loaded: the Gateway lists every kind together.
  const shownItems = ofType(query ? items.filter((row) => row.excerpt !== undefined || row.title.toLowerCase().includes(query.toLowerCase())) : items, type);
  const paging = useRef(false);
  // Changing scope, query, or order invalidates any outstanding page.
  const view = tab + "|" + order + "|" + query;
  const shown = useRef(view);
  shown.current = view;
  const cache = useStored<Row[]>("cache", "pages:" + tab + (order === "recent" ? "" : ":" + order));
  const listed = useRef<string | undefined>(undefined);
  const generation = useRef(0);
  const read = useCallback(
    async (which: Tab, after?: string): Promise<Batch> => {
      if (which === "mine") {
        const page = await artifacts.pages({ limit: PAGE, cursor: after, q: query || undefined, order, docText: query ? true : undefined });
        return {
          rows: page.pages.map((p) => ({
            id: p.id,
            excerpt: page.contentMatches?.[p.id],
            title: p.title,
            meta: [KIND_LABEL[kindOf(p.format)], ago(p.updatedAt)].join(" · "),
            shared: false,
            kind: kindOf(p.format),
          })),
          cursor: page.nextCursor,
        };
      }
      const page = await artifacts.artifacts({ scope: which, status: "active", limit: PAGE, cursor: after, q: query || undefined, order });
      return {
        rows: page.artifacts.map((a) => ({
          id: a.id,
          title: a.title,
          meta: [
            KIND_LABEL[a.kind === "page" ? kindOf(a.pageFormat) : "chat"],
            a.mine ? audienceLabel(a.audience) : a.author.name || a.author.email,
            ago(a.createdAt),
          ]
            .filter(Boolean)
            .join(" · "),
          shared: true,
          link: ui ? absoluteLink(a.publicUrl ?? a.url, ui) : a.publicUrl ?? a.url,
          kind: a.kind === "page" ? kindOf(a.pageFormat) : "chat",
        })),
        cursor: page.nextCursor,
      };
    },
    [artifacts, order, query, ui],
  );

  const load = useCallback(() => {
    const request = ++generation.current;
    setLoading(true);
    if (listed.current !== view) {
      listed.current = view;
      setItems(cache.load() ?? []);
      setCursor(undefined);
      setError(null);
    }
    read(tab)
      .then((batch) => {
        if (shown.current !== view || generation.current !== request) return;
        setItems(batch.rows);
        if (!query) cache.save(batch.rows);
        setCursor(batch.cursor);
        setError(null);
      })
      .catch((e: Error) => { if (shown.current === view && generation.current === request) setError(e.message); })
      .finally(() => {
        if (shown.current !== view || generation.current !== request) return;
        setPulled(false);
        setLoading(false);
      });
  }, [read, tab, view, query, cache]);
  useFocusEffect(load);

  const loadMore = () => {
    if (!cursor || loading || paging.current) return;
    const request = generation.current;
    paging.current = true;
    read(tab, cursor)
      .then((batch) => {
        if (shown.current !== view || generation.current !== request) return;
        setItems((a) => {
          const have = new Set(a.map((x) => x.id));
          return [...a, ...batch.rows.filter((n) => !have.has(n.id))];
        });
        setCursor(batch.cursor);
      })
      .catch((e: Error) => { if (shown.current === view && generation.current === request) setError(e.message); })
      .finally(() => { paging.current = false; });
  };

  const sortOptions: { order: LibraryOrder; label: string }[] = [
    { order: "recent", label: tab === "mine" ? "Last edited" : "Last shared" },
    { order: "name", label: "Name" },
    { order: "kind", label: "Kind" },
    ...(tab === "mine" ? [{ order: "created" as const, label: "Created" }] : []),
  ];
  const open = (row: Row) => {
    haptic("tap");
    setPicked(undefined);
    router.push(row.shared ? { pathname: "/artifact/[id]", params: { id: row.id } } : { pathname: "/page/[id]", params: { id: row.id } });
  };
  const copy = async (row: Row) => {
    if (!row.link) return;
    try {
      await Clipboard.setStringAsync(row.link);
      setPicked(undefined);
      haptic("success");
      toast({ text: "Link copied", icon: "copy" });
    } catch {
      haptic("error");
      toast({ text: "Couldn't copy the link" });
    }
  };
  const move = async (row: Row) => {
    haptic("tap");
    setPicked(undefined);
    try {
      await artifacts.setPageTrash(row.id, true);
      setItems((current) => current.filter((item) => item.id !== row.id));
      load();
      toast({
        text: "Moved to Trash",
        haptic: "success",
        action: { label: "Undo", onPress: () => {
          void artifacts.setPageTrash(row.id, false).then(load, (error: Error) => toast({ text: error.message, haptic: "error" }));
        } },
      });
    } catch (error) {
      toast({ text: error instanceof Error ? error.message : "Couldn't move to Trash", haptic: "error" });
    }
  };

  const chooseTab = (next: Tab) => {
    if (next === tab) return;
    haptic("selection");
    setCursor(undefined);
    setTab(next);
    if (next !== "mine" && order === "created") setOrder("recent");
  };

  const account = (
    <View accessibilityRole="tablist" style={[styles.scopes, { backgroundColor: theme.fill }]}>
      {OWNERS.map(({ owner, label }) => (
        <Pressable key={owner} accessibilityRole="button" accessibilityState={{ selected: tab === owner }}
          onPress={() => chooseTab(owner)} style={[styles.scope, tab === owner && { backgroundColor: theme.card }]}>
          <Text style={[styles.pillText, { color: tab === owner ? theme.foreground : theme.muted }]}>{label}</Text>
        </Pressable>
      ))}
    </View>
  );

  const search = (
    <View style={[styles.searchField, { backgroundColor: theme.fill }]}>
      <Icon name="search" size={18} color={theme.muted} />
      <TextInput accessibilityLabel={tab === "mine" ? "Search titles and Doc text" : "Search artifact names"}
        placeholder={tab === "mine" ? "Search titles and Doc text" : "Search artifact names"} placeholderTextColor={theme.muted}
        value={typed} onChangeText={setTyped} autoCorrect={false} returnKeyType="search" maxLength={200}
        style={[styles.searchInput, { color: theme.foreground }]} />
      {typed.length > 0 && <Pressable accessibilityRole="button" accessibilityLabel="Clear search" style={styles.clearSearch}
        onPress={() => { haptic("tap"); setTyped(""); setQuery(""); }}>
        <Icon name="close" size={16} color={theme.foreground} />
      </Pressable>}
    </View>
  );

  const pills = (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.pills} accessibilityRole="tablist">
      {TYPES.map((t) => (
        <Pressable
          key={t.type}
          style={styles.pillTarget}
          onPress={() => { if (type !== t.type) haptic("selection"); setType(t.type); }}
          // `tab` gets no trait on iOS, so VoiceOver would not say which one is picked.
          accessibilityRole="button"
          accessibilityState={{ selected: type === t.type }}
        >
          <View style={[styles.pill, type === t.type && { backgroundColor: theme.fill }]}>
            <Text
              style={[styles.pillText, { color: type === t.type ? theme.foreground : theme.muted }]}
              numberOfLines={1}
              maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}
            >
              {t.label}
            </Text>
          </View>
        </Pressable>
      ))}
    </ScrollView>
  );

  return (
    <>
      <FlatList
        contentInsetAdjustmentBehavior="automatic"
        data={shownItems}
        keyExtractor={(row) => row.id}
        // A FlatList cannot change its columns in place: a new key makes a new list.
        key={columns}
        keyboardShouldPersistTaps="handled"
        numColumns={columns}
        columnWrapperStyle={columns > 1 ? styles.columns : undefined}
        refreshing={pulled}
        onRefresh={() => {
          setPulled(true);
          load();
        }}
        onEndReached={loadMore}
        onEndReachedThreshold={0.5}
        contentContainerStyle={[styles.grid, shownItems.length ? null : styles.grow]}
        ListHeaderComponent={
          <>
            <LargeTitle>Artifacts</LargeTitle>
            {search}
            {account}
            {pills}
            {tab === "mine" && <Pressable accessibilityRole="button" style={styles.trash}
              onPress={() => { haptic("tap"); router.push("/artifact-trash"); }}>
              <Text style={[styles.pillText, { color: theme.foreground }]}>Trash</Text>
              <Icon name="chevron" color={theme.muted} size={14} />
            </Pressable>}
            <Pressable accessibilityRole="button" accessibilityLabel="Sort artifacts" style={styles.sort}
              onPress={() => { haptic("tap"); setChooseOrder(true); }}>
              <Text style={[styles.pillText, { color: theme.foreground }]}>{sortOptions.find((choice) => choice.order === order)?.label ?? "Last shared"}</Text>
              <Icon name="chevron" color={theme.muted} size={14} />
            </Pressable>
            {error && <>
              <Text style={[styles.note, { color: theme.danger }]}>{items.length ? "Showing saved artifacts." : error}</Text>
              <Pressable accessibilityRole="button" onPress={load} style={styles.sort}>
                <Text style={[styles.pillText, { color: theme.foreground }]}>Try again</Text>
              </Pressable>
            </>}
          </>
        }
        ListFooterComponent={cursor ? <ActivityIndicator style={styles.more} color={theme.muted} /> : undefined}
        ListEmptyComponent={
          loading ? (
            <ActivityIndicator style={styles.more} color={theme.muted} />
          ) : (
            <Text style={[styles.note, styles.center, { color: theme.muted }]}>
              {query ? "No artifacts match." : items.length ? `No ${TYPES.find((t) => t.type === type)!.label.toLowerCase()} here yet.` : EMPTY[tab]}
            </Text>
          )
        }
        renderItem={({ item }) => (
          <PressScale
            accessibilityRole="button"
            accessibilityLabel={`${item.title || "Untitled"}, ${item.meta}`}
            style={styles.tile}
            hostStyle={{ width: tileWidth }}
            onPress={() => open(item)}
            onLongPress={() => { haptic("selection"); setPicked(item); }}
            accessibilityActions={[{ name: "options", label: "Artifact actions" }]}
            onAccessibilityAction={(event) => { if (event.nativeEvent.actionName === "options") setPicked(item); }}
          >
            <Thumbnail title={item.title || "Untitled"} kind={item.kind} theme={theme} />
            <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={1}>
              {item.title || "Untitled"}
            </Text>
            <Text style={[styles.meta, { color: theme.tertiary }]} numberOfLines={1}>
              {item.meta}
            </Text>
            {item.excerpt !== undefined && (
              <Text style={[styles.meta, { color: theme.muted }]} numberOfLines={2}>{item.excerpt}</Text>
            )}
          </PressScale>
        )}
      />
      <Sheet visible={chooseOrder} onClose={() => setChooseOrder(false)}>
        <SheetTitle>Sort artifacts</SheetTitle>
        {sortOptions.map((choice) => (
          <SheetRow key={choice.order} label={choice.label} tone={choice.order === order ? "primary" : "normal"}
            onPress={() => {
              haptic("selection");
              setOrder(choice.order);
              sortChoice.save(choice.order);
              setChooseOrder(false);
            }} />
        ))}
      </Sheet>
      <Sheet visible={Boolean(picked)} onClose={() => setPicked(undefined)}>
        {picked && <>
          <SheetTitle>{picked.title || "Untitled"}</SheetTitle>
          <SheetRow label="Open" onPress={() => open(picked)} />
          {picked.shared ? picked.link && <SheetRow label="Copy link" onPress={() => void copy(picked)} />
            : <SheetRow label="Share…" onPress={() => { setPicked(undefined); setTimeout(() => setSharing(picked), 450); }} />}
          {!picked.shared && <SheetRow label="Move to Trash" tone="danger" onPress={() => void move(picked)} />}
          <SheetRow label="Close" onPress={() => setPicked(undefined)} />
        </>}
      </Sheet>
      <ShareSheet actions={sharing ? { title: sharing.title || "Untitled", shareable: { kind: "page", id: sharing.id } } : null}
        onClose={() => setSharing(undefined)} onError={(message) => toast({ text: message })} />

    </>
  );
}

/**
 * A tile's thumbnail, drawn rather than captured (the Gateway keeps no image of a page): a doc's title over its lines,
 * a page as a white sheet with a heading and a rule, a chat as its two sides. It only decorates, so it is hidden from
 * VoiceOver; the tile says what it is.
 */
function Thumbnail({ title, kind, theme }: { title: string; kind: Kind; theme: Theme }) {
  const line = (width: `${number}%`, key: number) => <View key={key} style={[styles.line, { width, backgroundColor: theme.fill }]} />;
  return (
    <View style={[styles.thumb, { backgroundColor: theme.card }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      {kind === "doc" ? (
        <>
          <Text style={[styles.thumbTitle, { color: theme.foreground }]} numberOfLines={2}>
            {title}
          </Text>
          {(["92%", "86%", "95%", "60%"] as const).map(line)}
        </>
      ) : kind === "sheet" ? (
        <View style={[styles.sheetGrid, { borderColor: theme.border }]}>
          {[0, 1, 2, 3].map((row) => (
            <View key={row} style={styles.sheetGridRow}>
              {[0, 1, 2].map((column) => (
                <View key={column} style={[styles.sheetGridCell, { borderColor: theme.border },
                  row === 0 && { backgroundColor: theme.fill }]} />
              ))}
            </View>
          ))}
        </View>
      ) : kind === "site" || kind === "board" ? (
        // The page's own White in both appearances, as a page renders, with the boards' greys on it.
        <View style={styles.sheet}>
          <Text style={[styles.thumbTitle, styles.sheetTitle]} numberOfLines={1}>
            {title}
          </Text>
          <View style={[styles.bar, styles.barShort]} />
          <View style={[styles.bar, styles.barMid, styles.barInk]} />
          <View style={[styles.bar, styles.barLong]} />
        </View>
      ) : (
        <View style={styles.bubbles}>
          <View style={[styles.bubble, styles.mine, { backgroundColor: theme.fill }]} />
          <View style={[styles.bubble, styles.theirs, { backgroundColor: theme.fill }]} />
          <View style={[styles.bubble, styles.mine, styles.short, { backgroundColor: theme.fill }]} />
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  trash: { minHeight: 44, flexDirection: "row", gap: 8, alignItems: "center", alignSelf: "flex-start", paddingHorizontal: 8 },
  grow: { flexGrow: 1 },
  grid: { paddingHorizontal: 16, paddingBottom: 32, rowGap: 20 },
  scopes: { flexDirection: "row", gap: 2, padding: 4, borderRadius: 14, marginTop: 8 },
  scope: { flex: 1, minHeight: 44, alignItems: "center", justifyContent: "center", borderRadius: 10, padding: 4 },
  searchField: { marginTop: 8, minHeight: 48, flexDirection: "row", alignItems: "center", gap: 8, paddingLeft: 14, borderRadius: 14 },
  searchInput: { ...text.body, fontFamily: font.regular, minHeight: 48, flex: 1 },
  clearSearch: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  sort: { minHeight: 44, flexDirection: "row", gap: 8, alignItems: "center", alignSelf: "flex-start", paddingHorizontal: 6 },
  columns: { gap: 12 },
  // The lists as the boards' type pills: 32 pt, the chosen one on the fill.
  // The pills wrap to a second line at the larger text sizes, and grow with their text.
  pills: { flexDirection: "row", flexWrap: "wrap", gap: 4, marginHorizontal: -4, marginTop: 2 },
  // A 44 pt target around the 32 pt pill, for TalkBack, VoiceOver and touch alike; the top margin gives back 6 pt. Not
  // the bottom: Android takes no touch outside the parent, so a negative margin there would cut the target.
  pillTarget: { minHeight: 44, justifyContent: "center", flexShrink: 1 },
  pill: { minHeight: 32, paddingHorizontal: 14, borderRadius: 16, alignItems: "center", justifyContent: "center", flexShrink: 1 },
  pillText: { ...text.secondary, fontFamily: font.regular },
  tile: { gap: 4 },
  thumb: { height: 112, paddingVertical: 14, paddingHorizontal: 16, borderRadius: 12, borderCurve: "continuous", overflow: "hidden", marginBottom: 4 },
  sheetGrid: { flex: 1, borderTopWidth: StyleSheet.hairlineWidth, borderLeftWidth: StyleSheet.hairlineWidth },
  sheetGridRow: { flex: 1, flexDirection: "row" },
  sheetGridCell: { flex: 1, borderBottomWidth: StyleSheet.hairlineWidth, borderRightWidth: StyleSheet.hairlineWidth },
  thumbTitle: { fontFamily: font.regular, fontSize: 11, lineHeight: 14 },
  line: { height: 4, marginTop: 6, borderRadius: 2 },
  // A hairline round the page, so its White still reads on the white card of light mode.
  sheet: { flex: 1, margin: -8, padding: 12, borderRadius: 6, backgroundColor: "#ffffff", borderWidth: StyleSheet.hairlineWidth, borderColor: "#dcdcdf" },
  sheetTitle: { color: "#1c1c1e" },
  bar: { height: 6, marginTop: 8, borderRadius: 2, backgroundColor: "#d1d1d6" },
  barInk: { backgroundColor: "#1c1c1e" },
  barShort: { width: "12%" },
  barMid: { width: "34%" },
  barLong: { width: "96%" },
  bubbles: { flex: 1, justifyContent: "center", gap: 6 },
  bubble: { height: 14, borderRadius: 7, width: "62%" },
  mine: { alignSelf: "flex-end" },
  theirs: { alignSelf: "flex-start", width: "80%" },
  short: { width: "40%" },
  title: { ...text.secondary, fontFamily: font.regular },
  meta: { ...text.caption, fontFamily: font.regular },
  note: { ...text.sm, fontFamily: font.regular, paddingVertical: 20 },
  center: { textAlign: "center" },
  more: { paddingVertical: 20 },
});
