import type { ChatSessionSummary } from "@nexus/kit";
import { Stack, router, useFocusEffect } from "expo-router";
import { useCallback, useRef, useState } from "react";
import { ActivityIndicator, FlatList, StyleSheet, View } from "react-native";
import { useCore, useStored } from "@/auth";
import { ChatMenu } from "@/chat-menu";
import { ChatRow } from "@/chat-row";
import { transientError } from "@/connection-errors";
import { haptic } from "@/haptics";
import { Blank, Footnote, LargeTitle, Loading } from "@/list";
import { useModels } from "@/models";
import { restoreQueuedChat, type QueuedUnarchive } from "@/queued-unarchive";
import { connectionSignal } from "@/reconnect";
import { useScreenFocused } from "@/screen-focus";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { useReachable } from "@/use-reachable";
import { useTheme } from "@/theme";

const PAGE = 50;
type Snapshot = { sessions: ChatSessionSummary[]; at: number };

/** Archived chats, most recently active first, paged like Chats; long press to restore or delete. */
export default function Archived() {
  const core = useCore();
  const theme = useTheme();
  const models = useModels(core);
  const cache = useStored<Snapshot>("cache", "archived-chats");
  const queuedStore = useStored<QueuedUnarchive[]>("draft", "archived-chat-restores");
  const [queued, setQueued] = useState(() => queuedStore.load() ?? []);
  const queue = useRef(queued);
  const focused = useScreenFocused();
  const [chats, setChats] = useState(() => cache.load()?.sessions ?? []);
  const [savedAt, setSavedAt] = useState(() => cache.load()?.at);
  const [offline, setOffline] = useState(false);
  const reading = useRef<AbortController | null>(null);
  const restoring = useRef(new Set<string>());
  const flushing = useRef(Promise.resolve());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [menu, setMenu] = useState<ChatSessionSummary | null>(null);
  const [queueMenu, setQueueMenu] = useState<ChatSessionSummary | null>(null);
  const [restoreError, setRestoreError] = useState<string | null>(null);
  const [cursor, setCursor] = useState<string | undefined>();
  const paging = useRef<AbortSignal | undefined>(undefined);

  const changeQueue = useCallback((next: QueuedUnarchive[]) => {
    if (next.length) queuedStore.save(next);
    else queuedStore.remove();
    queue.current = next;
    setQueued(next);
  }, [queuedStore]);

  const removeCached = useCallback((id: string) => {
    setChats((current) => current.filter((chat) => chat.id !== id));
    const kept = cache.load();
    if (kept) cache.save({ ...kept, sessions: kept.sessions.filter((chat) => chat.id !== id) });
  }, [cache]);

  const flush = useCallback((signal: AbortSignal) => {
    const current = flushing.current.then(async () => {
      for (const item of queue.current) {
        if (signal.aborted) return;
        if (!queue.current.includes(item) || restoring.current.has(item.id)) continue;
        restoring.current.add(item.id);
        try {
          const result = await restoreQueuedChat(core, item, connectionSignal(signal));
          if (signal.aborted) return;
          changeQueue(queue.current.filter((chat) => chat.id !== item.id));
          if (result === "restored") removeCached(item.id);
          else setRestoreError("This chat changed. Swipe it again to unarchive.");
        } catch (failure) {
          if (signal.aborted) return;
          if (transientError(failure)) {
            setOffline(true);
            return;
          }
          changeQueue(queue.current.filter((chat) => chat.id !== item.id));
          setRestoreError(failure instanceof Error ? failure.message : "Couldn't unarchive this chat.");
        } finally {
          restoring.current.delete(item.id);
        }
      }
    });
    flushing.current = current.catch(() => undefined);
    return current;
  }, [changeQueue, core, removeCached]);

  const load = useCallback(() => {
    reading.current?.abort();
    const request = new AbortController();
    reading.current = request;
    setLoading(true);
    core
      .chatSessions({ limit: PAGE, archived: "only" }, connectionSignal(request.signal))
      .then((page) => {
        if (request.signal.aborted) return;
        const at = Date.now();
        cache.save({ sessions: page.sessions, at });
        setChats(page.sessions);
        setSavedAt(at);
        setCursor(page.nextCursor);
        setError(null);
        setOffline(false);
        return flush(request.signal);
      })
      .catch((e: Error) => {
        if (request.signal.aborted) return;
        const offline = transientError(e);
        setError(e.message);
        setOffline(offline);
        if (offline) return;
        cache.remove();
        setChats([]);
        setSavedAt(undefined);
        setCursor(undefined);
      })
      .finally(() => { if (!request.signal.aborted) setLoading(false); });
  }, [cache, core, flush]);
  useFocusEffect(useCallback(() => {
    const next = queuedStore.load() ?? [];
    queue.current = next;
    setQueued(next);
    const kept = cache.load();
    setChats(kept?.sessions ?? []);
    setSavedAt(kept?.at);
    setCursor(undefined);
    load();
    return () => reading.current?.abort();
  }, [cache, load, queuedStore]));
  useReachable(focused && offline && queued.length > 0, (signal) => core.me(signal), load);

  const remove = (id: string) => {
    removeCached(id);
    load();
  };
  const enqueue = (chat: ChatSessionSummary) => {
    if (queue.current.some((item) => item.id === chat.id)) return;
    setRestoreError(null);
    changeQueue([...queue.current, { id: chat.id, updatedAt: chat.updatedAt }]);
  };
  const unarchive = async (chat: ChatSessionSummary) => {
    if (restoring.current.has(chat.id) || queue.current.some((item) => item.id === chat.id)) return;
    haptic("tap");
    setRestoreError(null);
    if (offline) {
      enqueue(chat);
      return;
    }
    const signal = reading.current?.signal;
    if (!signal || signal.aborted) return;
    restoring.current.add(chat.id);
    try {
      await core.updateChatSession(chat.id, { archived: false }, connectionSignal(signal));
      if (signal.aborted) return;
      remove(chat.id);
      haptic("success");
    } catch (failure) {
      if (signal.aborted) return;
      if (transientError(failure)) {
        enqueue(chat);
        setOffline(true);
      } else {
        setRestoreError(failure instanceof Error ? failure.message : "Couldn't unarchive this chat.");
        haptic("error");
      }
    } finally {
      restoring.current.delete(chat.id);
    }
  };
  const cancelRestore = (id: string) => {
    if (restoring.current.has(id)) {
      setQueueMenu(null);
      setRestoreError("The chat is being restored.");
      return;
    }
    haptic("tap");
    changeQueue(queue.current.filter((item) => item.id !== id));
    setQueueMenu(null);
  };
  const menuQueued = queueMenu !== null && queued.some((item) => item.id === queueMenu.id);

  /** The next page, once, when the list nears its end: none of your archived chats is out of reach. */
  const loadMore = () => {
    const signal = reading.current?.signal;
    if (!signal || signal.aborted || offline || loading || !cursor || paging.current === signal) return;
    paging.current = signal;
    core
      .chatSessions({ limit: PAGE, cursor, archived: "only" }, signal)
      .then((page) => {
        if (signal.aborted) return;
        setChats((c) => {
          const have = new Set(c.map((x) => x.id));
          return [...c, ...page.sessions.filter((n) => !have.has(n.id))];
        });
        setCursor(page.nextCursor);
      })
      .catch((e: Error) => {
        if (signal.aborted) return;
        setError(e.message);
        setOffline(transientError(e));
      })
      .finally(() => {
        if (paging.current === signal) paging.current = undefined;
      });
  };

  return (
    <>
      <FlatList
        style={{ backgroundColor: theme.grouped }}
        data={chats}
        keyExtractor={(c) => c.id}
        refreshing={loading && chats.length > 0}
        onRefresh={load}
        onEndReached={loadMore}
        onEndReachedThreshold={0.5}
        ListFooterComponent={cursor && !error ? <ActivityIndicator style={styles.more} color={theme.muted} /> : undefined}
        contentContainerStyle={styles.list}
        ListHeaderComponent={
          <View style={styles.header}>
            <LargeTitle>Archived</LargeTitle>
            {offline && savedAt ? <Footnote>Offline. Showing the list saved at {new Date(savedAt).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}.</Footnote> : null}
            {queued.length ? (
              <Footnote>{queued.length === 1 ? "One chat is waiting to unarchive." : `${queued.length} chats are waiting to unarchive.`}</Footnote>
            ) : null}
            {restoreError ? <Footnote tone="danger">{restoreError}</Footnote> : null}
            {error && chats.length > 0 && !offline ? <Footnote tone="danger">{error}</Footnote> : null}
          </View>
        }
        ListEmptyComponent={loading ? <Loading /> : error && !(offline && savedAt) ? (
          <Blank title="Couldn't load archived sessions" body={error} action={{ label: "Try again", onPress: load }} />
        ) : (
          <Blank title="No archived sessions" body="Archive a session from its menu to keep it here." />
        )}
        renderItem={({ item, index }) => {
          const waiting = queued.some((chat) => chat.id === item.id);
          return (
            <ChatRow
              chat={item}
              models={models}
              first={index === 0}
              last={index === chats.length - 1}
              onPress={() => router.dismissTo({ pathname: "/session/[id]", params: { id: item.id } })}
              onLongPress={() => (offline || waiting ? setQueueMenu(item) : setMenu(item))}
              onUnarchive={waiting ? undefined : () => void unarchive(item)}
            />
          );
        }}
      />
      <Stack.Screen options={{ headerTitle: "", headerStyle: { backgroundColor: theme.grouped }, contentStyle: { backgroundColor: theme.grouped } }} />
      <Sheet visible={queueMenu !== null} onClose={() => setQueueMenu(null)}>
        <SheetTitle>{queueMenu?.title || "Archived chat"}</SheetTitle>
        {menuQueued && queueMenu ? (
          <SheetRow label="Cancel queued unarchive" onPress={() => cancelRestore(queueMenu.id)} />
        ) : queueMenu ? (
          <SheetRow label={offline ? "Unarchive when online" : "Unarchive"} onPress={() => {
            setQueueMenu(null);
            void unarchive(queueMenu);
          }} />
        ) : null}
        <SheetRow label="Close" onPress={() => setQueueMenu(null)} tone="muted" />
      </Sheet>
      <ChatMenu
        chat={menu}
        onClose={() => setMenu(null)}
        onChanged={(chat) => setChats((c) => c.map((x) => (x.id === chat.id ? chat : x)))}
        onRemoved={remove}
        onError={setError}
      />
    </>
  );
}

const styles = StyleSheet.create({
  list: { paddingTop: 12, paddingBottom: 16 },
  header: { marginHorizontal: 16, paddingBottom: 12 },
  more: { paddingVertical: 20 },
});
