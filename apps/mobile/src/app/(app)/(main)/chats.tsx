import { Stack, router, useFocusEffect, useIsFocused, type Href } from "expo-router";
import { useCallback, useMemo, useState } from "react";
import { ActivityIndicator, Pressable, SectionList, StyleSheet, Text, View } from "react-native";
import { ago, spokenAgo } from "@/ago";
import { useCore, useStored } from "@/auth";
import { ChatMenu } from "@/chat-menu";
import { ChatRow } from "@/chat-row";
import type { DrawerOrganization } from "@/drawer-organization";
import { rowState, sessionSections, stateWord } from "@/drawer-groups";
import { haptic } from "@/haptics";
import { LargeTitle } from "@/list";
import { NavRow, SessionMark } from "@/nav-row";
import { useModels } from "@/models";
import { sessionDetail, sessionHref, sessionKey, sessionTitle, useSessionHistory, type UnifiedSession } from "@/session-history";
import { RowMenu } from "@/row-menu";
import { SearchPanel } from "@/search-panel";
import { SwipeActions } from "@/swipe-actions";
import { useToast } from "@/toast";
import { useSessionPins } from "@/use-session-pins";
import { font, text, useTheme } from "@/theme";
import type { ChatSessionSummary } from "@nexus/kit";

const FILTERS = [{ id: "all", label: "All" }, { id: "mine", label: "Mine" }, { id: "archived", label: "Archived" }] as const;

export default function Sessions() {
  const core = useCore();
  const theme = useTheme();
  const focused = useIsFocused();
  const toast = useToast();
  const layout = useStored<DrawerOrganization>("choice", "session-organization");
  const pins = useSessionPins(core, focused, layout.load()?.pinned ?? []);
  const models = useModels(core);
  const [scope, setScope] = useState<"all" | "mine">("all");
  const history = useSessionHistory(core, 50, scope === "all");
  const [menu, setMenu] = useState<ChatSessionSummary | null>(null);
  const [searching, setSearching] = useState(false);
  useFocusEffect(useCallback(() => { history.refresh(); }, [history.refresh]));
  const sections = useMemo(() => sessionSections(history.sessions, history.titles), [history.sessions, history.titles]);
  const open = (href: Href) => {
    haptic("tap");
    router.dismissTo(href);
  };
  const openMenu = (chat: ChatSessionSummary) => {
    haptic("longPress");
    setMenu(chat);
  };
  const pinAction = (item: Pick<UnifiedSession, "source" | "id">) => {
    if (!pins.available) return;
    const pinned = pins.keys.includes(sessionKey(item));
    return { label: pinned ? "Unpin" : "Pin", onPress: () => { haptic("selection"); pins.set(item.source, item.id, !pinned); } };
  };
  const archive = (chat: ChatSessionSummary) => {
    history.remove({ source: "chat", id: chat.id });
    const update = (archived: boolean) => core.updateChatSession(chat.id, { archived }).then(history.refresh);
    const failed = () => { history.refresh(); toast({ text: "Couldn’t change archive status.", haptic: "error" }); };
    void update(true).then(() => toast({
      text: `Archived “${chat.title || "Untitled"}”`, icon: "archive", haptic: "success",
      action: { label: "Undo", onPress: () => { void update(false).catch(failed); } },
    }), failed);
  };
  if (searching) return <SearchPanel recentSessions={history.sessions} recentTitles={history.titles} recentError={history.error} onClose={() => setSearching(false)} onNavigate={open} />;
  return <>
    <Stack.Screen options={{ headerRight: () => <Pressable onPress={() => { haptic("tap"); setSearching(true); }} accessibilityRole="button" accessibilityLabel="Search sessions" style={styles.search}><Text style={[styles.link, { color: theme.foreground }]}>Search</Text></Pressable> }} />
    <SectionList key={scope} sections={sections} keyExtractor={sessionKey} contentInsetAdjustmentBehavior="automatic"
      refreshing={history.loading && history.sessions.length > 0} onRefresh={history.refresh} onEndReached={() => { if (!history.error) history.loadMore(); }} onEndReachedThreshold={0.5}
      stickySectionHeadersEnabled={false} contentContainerStyle={styles.list}
      ListHeaderComponent={<View style={styles.head}>
        <LargeTitle>All sessions</LargeTitle>
        <View style={styles.filters}>
          {FILTERS.map(({ id, label }) => <Pressable key={id} accessibilityRole="button" accessibilityState={{ selected: scope === id }}
            style={[styles.filter, scope === id && { backgroundColor: theme.fill }]}
            onPress={() => {
              if (scope === id) return;
              haptic("selection");
              if (id === "archived") router.push("/archived");
              else setScope(id);
            }}>
            <Text style={[styles.link, { color: scope === id ? theme.foreground : theme.muted }]}>{label}</Text>
          </Pressable>)}
        </View>
        {history.error ? <Text style={[styles.note, { color: theme.danger }]}>{history.error}</Text> : null}
      </View>}
      ListFooterComponent={history.more ? (
        <Pressable
          disabled={history.loading}
          onPress={() => { haptic("tap"); history.loadMore(); }}
          style={styles.more}
          accessibilityRole="button"
          accessibilityLabel="Load earlier sessions"
        >
          {history.loading ? <ActivityIndicator color={theme.muted} /> : <Text style={[styles.link, { color: theme.muted }]}>Load earlier</Text>}
        </Pressable>
      ) : undefined}
      ListEmptyComponent={history.loading ? <ActivityIndicator color={theme.muted} /> : <Text style={[styles.note, { color: theme.muted }]}>Your Chat and computer sessions appear here.</Text>}
      renderSectionHeader={({ section }) => <Text accessibilityRole="header" style={[styles.shelf, { color: theme.tertiary }]}>{section.title}</Text>}
      renderItem={({ item, index, section }) => {
        const onOpen = () => open(sessionHref(item));
        const pin = pinAction(item);
        if (item.source === "chat") {
          return <ChatRow chat={item.chat} models={models} first={index === 0} last={index === section.data.length - 1}
            onPress={onOpen} onLongPress={() => openMenu(item.chat)} onArchive={() => archive(item.chat)} pin={pin} />;
        }
        const state = rowState(item, history.titles);
        const title = sessionTitle(item, history.titles);
        const detail = sessionDetail(item, history.titles);
        const status = stateWord(state);
        return <View style={[styles.native, { backgroundColor: theme.card }]}>
          <SwipeActions actions={pin ? [{ key: "pin", icon: "pin", color: theme.swipeMore, ...pin }] : []}>
            <RowMenu title={title} actions={pin ? [{ key: "pin", icon: { ios: "pin", android: "push_pin" }, ...pin }] : []}>
              {(onLongPress) => <NavRow leading={<SessionMark state={state} />} label={title} detail={detail} stateWord={status}
                onPress={onOpen} onLongPress={onLongPress} meta={ago(item.updatedAt)}
                accessibilityLabel={[title, detail, status ?? spokenAgo(item.updatedAt)].filter(Boolean).join(", ")}
                accessibilityActions={pin ? [{ name: "pin", label: pin.label }] : undefined}
                onAccessibilityAction={({ nativeEvent }) => { if (nativeEvent.actionName === "pin") pin?.onPress(); }} />}
            </RowMenu>
          </SwipeActions>
        </View>;
      }}
    />
    <ChatMenu chat={menu} pin={menu ? pinAction({ source: "chat", id: menu.id }) : undefined} onClose={() => setMenu(null)} onChanged={history.refresh} onRemoved={history.refresh} onError={history.refresh} />
  </>;
}
const styles = StyleSheet.create({
  filters: { flexDirection: "row", flexWrap: "wrap", gap: 8, marginBottom: 8 },
  filter: { minHeight: 44, minWidth: 68, paddingHorizontal: 14, borderRadius: 10, alignItems: "center", justifyContent: "center" },
  list: { paddingBottom: 88 }, head: { paddingHorizontal: 16 }, shelf: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 32, marginTop: 22, marginBottom: 7 },
  native: { marginHorizontal: 16, borderRadius: 12 }, search: { minWidth: 64, minHeight: 44, alignItems: "center", justifyContent: "center" },
  link: { ...text.body, fontFamily: font.medium }, more: { alignSelf: "center", padding: 24, minHeight: 44 }, note: { ...text.body, fontFamily: font.regular, padding: 20 },
});
