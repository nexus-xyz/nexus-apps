import type { NotificationItem } from "@nexus/kit";
import { Link, Stack, router, useFocusEffect } from "expo-router";
import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, FlatList, Pressable, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { useReducedMotion } from "react-native-reanimated";
import * as WebBrowser from "expo-web-browser";
import { ago, spokenAgo } from "@/ago";
import { useArtifacts, useAuth, useStored } from "@/auth";
import { Icon } from "@/icon";
import { itemLine, itemTarget, kindLook, unreadBadge } from "@/inbox";
import { collapseNext } from "@/motion";
import { pressedFill, ripple } from "@/press";
import { RowMenu, type RowAction } from "@/row-menu";
import { SwipeActions } from "@/swipe-actions";
import { font, text, useTheme, type Theme } from "@/theme";
import { isAccessibilitySize } from "@/text-size";

/**
 * The inbox, as brand v5's P35·D draws it on the phone: what is waiting for you, newest first, one row each with its
 * kind's symbol (in amber when it asks for attention), its words, where it happened and how long ago. Unread ones carry
 * a dot. A tap reads it and opens its source's page. Done takes it away: a swipe to the left on iPhone, and the row's
 * long-press menu on both phones. The design's Snooze waits on the Gateway, which has no snooze. ··· opens what
 * notifies you, and where.
 */
export default function Inbox() {
  const artifacts = useArtifacts();
  const { orgName, ui } = useAuth();
  const theme = useTheme();
  const reduce = useReducedMotion();
  // As it was last time, until the Gateway answers.
  const cache = useStored<NotificationItem[]>("cache", "inbox");
  const [items, setItems] = useState<NotificationItem[]>(() => cache.load() ?? []);
  const [cursor, setCursor] = useState<string | undefined>();
  const [loading, setLoading] = useState(true);
  const [pulling, setPulling] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const paging = useRef(false);
  // The drawer's count follows what is read and done here, so it is right the next time the drawer opens.
  const unreadCache = useStored<string>("cache", "inbox-unread");
  useEffect(() => {
    if (loading) return;
    const badge = unreadBadge(items.filter((x) => !x.readAt).length, !!cursor);
    if (badge) unreadCache.save(badge);
    else unreadCache.remove();
  }, [items, cursor, loading, unreadCache]);

  const load = useCallback(
    () =>
      artifacts
        .notifications()
        .then((page) => {
          setItems(page.items);
          setCursor(page.nextCursor);
          cache.save(page.items);
          setError(null);
        })
        .catch((e: Error) => setError(e.message))
        .finally(() => setLoading(false)),
    [artifacts, cache],
  );
  useFocusEffect(
    useCallback(() => {
      void load();
    }, [load]),
  );
  const loadMore = () => {
    if (!cursor || paging.current) return;
    paging.current = true;
    artifacts
      .notifications({ cursor })
      .then((page) => {
        setItems((now) => [...now, ...page.items.filter((n) => !now.some((x) => x.id === n.id))]);
        setCursor(page.nextCursor);
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => {
        paging.current = false;
      });
  };

  const open = (item: NotificationItem) => {
    if (!item.readAt) {
      setItems((now) => now.map((x) => (x.id === item.id ? { ...x, readAt: new Date().toISOString() } : x)));
      artifacts.markNotificationRead(item.id).catch(() => undefined);
    }
    const target = itemTarget(item);
    if (!target) return;
    // An approval opens Nexus UI's page in the browser, where the person confirms or denies it signed in as themselves.
    if (typeof target === "object" && "web" in target) {
      if (ui) void WebBrowser.openBrowserAsync(`${ui.replace(/\/$/, "")}${target.web}`).catch(() => undefined);
      return;
    }
    router.push(target);
  };
  const done = (item: NotificationItem) => {
    // The row fades while the rows under it close the gap; under Reduce Motion it just goes.
    collapseNext(reduce);
    setItems((now) => now.filter((x) => x.id !== item.id));
    artifacts.markNotificationDone(item.id).catch((e: Error) => {
      setError(e.message);
      void load();
    });
  };

  return (
    <>
      <Stack.Screen
        options={{
          headerRight: () => (
            <Link href="/notification-settings" asChild>
              <Pressable accessibilityRole="button" hitSlop={12} accessibilityLabel="Notification settings">
                <Icon name="more" color={theme.foreground} />
              </Pressable>
            </Link>
          ),
        }}
      />
      <FlatList
        data={items}
        keyExtractor={(n) => n.id}
        contentInsetAdjustmentBehavior="automatic"
        refreshing={pulling}
        onRefresh={() => {
          setPulling(true);
          void load().finally(() => setPulling(false));
        }}
        onEndReached={loadMore}
        onEndReachedThreshold={0.5}
        contentContainerStyle={items.length ? undefined : styles.grow}
        ListHeaderComponent={error ? <Text style={[styles.error, { color: theme.danger }]}>{error}</Text> : undefined}
        ListFooterComponent={cursor ? <ActivityIndicator style={styles.more} color={theme.muted} /> : undefined}
        ListEmptyComponent={loading ? <ActivityIndicator style={styles.more} color={theme.muted} /> : <CaughtUp theme={theme} org={orgName} />}
        renderItem={({ item }) => <InboxRow item={item} theme={theme} onPress={() => open(item)} onDone={() => done(item)} />}
      />
    </>
  );
}

/** Nothing waiting (P35·E): a check, then what lands here, so the empty screen says what the inbox is for. */
function CaughtUp({ theme, org }: { theme: Theme; org?: string }) {
  return (
    <View style={styles.empty}>
      <Icon name="picked" size={22} color={theme.tertiary} />
      <Text style={[styles.emptyTitle, { color: theme.foreground }]}>You’re all caught up</Text>
      <Text style={[styles.emptyDetail, { color: theme.muted }]}>
        {`Nothing waits on you${org ? ` in ${org}` : ""}. Requests, spending limits and billing changes land here when they come.`}
      </Text>
    </View>
  );
}

function InboxRow({ item, theme, onPress, onDone }: { item: NotificationItem; theme: Theme; onPress: () => void; onDone: () => void }) {
  const unread = !item.readAt;
  const look = kindLook(item);
  // At the accessibility text sizes the words take the lines they need: two would cut most of them.
  const big = isAccessibilitySize(useWindowDimensions().fontScale);
  const row = (onLongPress: (() => void) | undefined) => (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, { backgroundColor: theme.screen }, pressed && pressedFill(theme)]}
      onPress={onPress}
      onLongPress={onLongPress}
      accessibilityRole="button"
      accessibilityLabel={[unread ? "Unread" : undefined, item.subjectText, itemLine(item), spokenAgo(item.updatedAt)].filter(Boolean).join(", ")}
      accessibilityActions={[{ name: "done", label: "Done" }]}
      onAccessibilityAction={(e) => e.nativeEvent.actionName === "done" && onDone()}
    >
      <View style={styles.mark}>
        <Icon name={look.icon} size={17} color={look.attention ? theme.mark : theme.muted} />
      </View>
      {/* The hairline runs under the words only, as the board draws it: the symbols stand in a column of their own. */}
      <View style={[styles.body, { borderBottomColor: theme.border }]}>
        <View style={styles.text}>
          <Text style={[styles.title, { color: theme.foreground }]} numberOfLines={big ? undefined : 2}>
            {item.subjectText}
          </Text>
          <Text style={[styles.line, { color: theme.tertiary }]} numberOfLines={big ? undefined : 1}>
            {big ? `${itemLine(item)} · ${ago(item.updatedAt)}` : itemLine(item)}
          </Text>
        </View>
        <View style={styles.side}>
          {/* At the accessibility sizes the age joins the second line, so the words keep the row's width. */}
          {big ? null : <Text style={[styles.when, { color: theme.tertiary }]}>{ago(item.updatedAt)}</Text>}
          {/* Unread: a dot under the time, so the words keep one weight whether read or not. */}
          <View style={[styles.dot, { backgroundColor: unread ? theme.foreground : "transparent" }]} />
        </View>
      </View>
    </Pressable>
  );
  // Done on both phones: the long-press menu (iOS's context menu, Android's sheet) and the accessibility action; on
  // iPhone the swipe too. The system green under black: 10:1, where White would be 2:1.
  const open: RowAction = { key: "open", label: "Open", icon: { ios: "arrow.up.right", android: "open_in_new" }, onPress };
  const done: RowAction = { key: "done", label: "Done", icon: { ios: "checkmark", android: "check" }, onPress: onDone };
  return (
    <RowMenu title={item.subjectText} actions={itemTarget(item) ? [open, done] : [done]}>
      {(onLongPress) => <SwipeActions actions={[{ key: "done", label: "Done", icon: "check", color: theme.success, ink: "#000000", onPress: onDone }]}>{row(onLongPress)}</SwipeActions>}
    </RowMenu>
  );
}

const styles = StyleSheet.create({
  grow: { flexGrow: 1 },
  // The board's inbox rows (P35·D): the symbol in a 24 pt column, then the words 16 on 21 with where under them at 13,
  // and the age at the right in the text's own face. The symbol sits on the first line's middle.
  row: { flexDirection: "row", alignItems: "flex-start", gap: 12, paddingLeft: 16 },
  mark: { width: 24, height: 21, marginTop: 11, alignItems: "center", justifyContent: "center" },
  body: { flex: 1, flexDirection: "row", gap: 12, paddingVertical: 11, paddingRight: 16, borderBottomWidth: StyleSheet.hairlineWidth },
  text: { flex: 1, gap: 2 },
  title: { fontFamily: font.regular, fontSize: 16, lineHeight: 21 },
  line: { ...text.caption, fontFamily: font.regular },
  side: { alignItems: "flex-end", gap: 8, paddingTop: 2 },
  when: { ...text.caption, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
  dot: { width: 7, height: 7, borderRadius: 3.5 },
  more: { paddingVertical: 24 },
  error: { ...text.sm, fontFamily: font.regular, padding: 16 },
  empty: { flex: 1, alignItems: "center", justifyContent: "center", paddingHorizontal: 40, gap: 8 },
  emptyTitle: { ...text.xl, fontFamily: font.display, textAlign: "center" },
  emptyDetail: { ...text.secondary, fontFamily: font.regular, textAlign: "center" },
});
