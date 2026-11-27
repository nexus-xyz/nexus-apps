import type { ChatSessionSummary } from "@nexus/kit";
import * as Clipboard from "expo-clipboard";
import { router, usePathname } from "expo-router";
import { useDrawerStatus, type DrawerContentComponentProps } from "expo-router/drawer";
import { useCallback, useEffect, useRef, useState, type ComponentRef } from "react";
import { Alert, AppState, Keyboard, Platform, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import Reanimated, { FadeIn, FadeOut, useReducedMotion, withTiming } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { AssistantNavRow } from "@/assistant-nav-row";
import { useArtifacts, useAuth, useCore, useStored } from "@/auth";
import { sessionHref, useSessionHistory } from "@/session-history";
import { ChatMenu } from "@/chat-menu";
import { keepProjectStart } from "@/connection-intent";
import { assistantPlace, drawerPlaces, errandHref, mobilePlaceVisibility, opensInDrawer, type DrawerPlace } from "@/drawer-places";
import { DrawerMenuRow } from "@/drawer-shell-menu";
import { DrawerSessions } from "@/drawer-sessions";
import { initials } from "@/greeting";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { unreadBadge } from "@/inbox";
import { Initial } from "@/list";
import { DURATION, timing } from "@/motion";
import { NavIcon, NavRow } from "@/nav-row";
import { OrgChoices } from "@/org-sheet";
import { useOrgs } from "@/org-switcher";
import { orgLabel } from "@/orgs";
import { ripple } from "@/press";
import type { RowAction } from "@/row-menu";
import { SearchPanel } from "@/search-panel";
import { Sheet, SheetTitle } from "@/sheet";
import { ACCESSIBILITY_FONT_SCALE } from "@/text-size";
import { font, text, useTheme, type Theme } from "@/theme";
import { useToast } from "@/toast";
import { EdgeFade } from "@/top-bar";
import { useHiddenMobilePlaces } from "@/use-hidden-mobile-places";
import { useSwitchOrg } from "@/use-switch-org";

/** The sessions the drawer loads, to group under Projects and Recent (`drawer-sessions.tsx`). */
const DRAWER_SESSIONS = 30;
const DRAWER_REFRESH_MS = 10_000;

/** iOS drops a sheet presented while its context menu is still closing: the menu's own sheet waits it out. */
const AFTER_CONTEXT_MENU_MS = Platform.OS === "ios" ? 400 : 0;

/**
 * The iOS Shell · Drawer starts with Assistant, New, Search, Artifacts, Customize and More. Its footer holds the
 * account and the Settings gear. Projects and Recent use the session groups in `drawer-sessions.tsx`.
 * The list scrolls under the head and the foot. All of it loads each time it opens, so a session just had is in the list.
 *
 * A chat has no project. An errand's row
 * has its own menu (`assistant-nav-row.tsx`).
 * VoiceOver reads the rows in the board's order (P94·E), each with its age, and offers the menu's actions in the rotor.
 */
export function Sidebar({ navigation }: { navigation: DrawerContentComponentProps["navigation"] }) {
  const core = useCore();
  const { name, orgName, uiLink, signOut, agent } = useAuth();
  const theme = useTheme();
  const toast = useToast();
  const insets = useSafeAreaInsets();
  const reduceMotion = useReducedMotion();
  const [panel, setPanel] = useState<"organizations" | "search" | "account" | "more" | null>(null);
  const openPanel = (next: NonNullable<typeof panel>) => {
    haptic("tap");
    setPanel(next);
  };
  const opened = useDrawerStatus() === "open";
  const drawerScroll = useRef<ComponentRef<typeof ScrollView>>(null);
  const [dragLocked, setDragLocked] = useState(false);
  useEffect(() => {
    if (!opened) setPanel(null);
  }, [opened]);
  const pathname = usePathname();
  const { orgs, current, error: orgError, unreachable, refresh: refreshOrganizations } = useOrgs(opened);
  const switcher = useSwitchOrg(current);
  const artifacts = useArtifacts();
  // What is unread in the inbox, read each time the drawer opens. The count from last time shows until then, so it does
  // not pop in while the drawer slides; none shows when it does not load.
  const unreadCache = useStored<string>("cache", "inbox-unread");
  const [unread, setUnread] = useState<string | undefined>(() => unreadCache.load());
  useEffect(() => {
    if (!opened) return;
    let active = true;
    // The drawer stays mounted: the inbox may have changed the count since.
    setUnread(unreadCache.load());
    artifacts.notifications({ unread: true }).then(
      (page) => {
        if (!active) return;
        const badge = unreadBadge(page.items.length, !!page.nextCursor);
        setUnread(badge);
        if (badge) unreadCache.save(badge);
        else unreadCache.remove();
      },
      () => { if (active) setUnread(undefined); },
    );
    return () => { active = false; };
  }, [opened, artifacts, unreadCache]);
  const history = useSessionHistory(core, DRAWER_SESSIONS);
  const [menu, setMenu] = useState<{ chat: ChatSessionSummary; start: "rename" | "delete" } | null>(null);
  // The head and foot float over the list; it scrolls under them.
  const [head, setHead] = useState(0);
  const [foot, setFoot] = useState(0);

  const load = history.refresh;
  const refresh = history.refreshQuiet;
  useEffect(() => {
    if (!opened) return;
    const tick = () => { if (AppState.currentState === "active") refresh(); };
    tick();
    const timer = setInterval(tick, DRAWER_REFRESH_MS);
    const foreground = AppState.addEventListener("change", tick);
    return () => { clearInterval(timer); foreground.remove(); };
  }, [opened, refresh]);

  const go = (to: Parameters<typeof router.push>[0], chat = false) => {
    Keyboard.dismiss();
    haptic("tap");
    if (chat) {
      navigation.closeDrawer();
      router.replace(to);
    } else {
      // Keep the drawer and its reading position under the page: Back returns to it.
      setPanel(null);
      router.push(to);
    }
  };
  const newSession = () => go({ pathname: "/session/[id]", params: { id: "new" } }, true);
  const onScreen = (chat: ChatSessionSummary) => pathname === `/session/${chat.id}`;
  const onNewSession = pathname === "/" || pathname === "/session/new";
  const appRowsHidden = panel !== null;
  const canSwitch = (orgs?.length ?? 0) > 1;
  const personal = current ? current.kind === "personal" : orgName === "Personal";
  const role = current?.role ?? (personal ? "owner" : undefined);
  const account = current ? orgLabel(current) : orgName || "Nexus";
  // Settings is a full-screen page, as Claude's and ChatGPT's are, on both platforms (`(main)/settings.tsx`).
  const openSettings = () => go("/settings");

  // P43's undo: archived at once, and a toast that puts it back.
  const archive = (chat: ChatSessionSummary) => {
    history.remove({ source: "chat", id: chat.id });
    if (onScreen(chat)) newSession();
    core.updateChatSession(chat.id, { archived: true }).then(
      () => {
        load();
        toast({
          text: `Archived “${chat.title || "Untitled"}”`,
          haptic: "success",
          icon: "archive",
          action: { label: "Undo", onPress: () => void core.updateChatSession(chat.id, { archived: false }).then(load, () => undefined) },
        });
      },
      (e: Error) => {
        void load();
        haptic("error");
        Alert.alert("Couldn't archive it", e.message);
      },
    );
  };
  const copyLink = (link: string) =>
    Clipboard.setStringAsync(link).then(
      () => toast({ text: "Link copied", icon: "link" }),
      (e: Error) => Alert.alert("Couldn't copy the link", e.message),
    );
  const openStep = (chat: ChatSessionSummary, start: "rename" | "delete") => setTimeout(() => setMenu({ chat, start }), AFTER_CONTEXT_MENU_MS);
  // Chat mutations use the Gateway session API.
  const actions = (chat: ChatSessionSummary): RowAction[] => {
    const link = uiLink?.(`/sessions/${chat.id}`);
    const rename: RowAction = { key: "rename", label: "Rename", icon: { ios: "pencil", android: "edit" }, onPress: () => openStep(chat, "rename") };
    const copy: RowAction | null = link ? { key: "copy", label: "Copy link", icon: { ios: "link", android: "link" }, onPress: () => void copyLink(link) } : null;
    const archiveIt: RowAction = { key: "archive", label: "Archive", icon: { ios: "archivebox", android: "archive" }, onPress: () => archive(chat) };
    const remove: RowAction = { key: "delete", label: "Delete", icon: { ios: "trash", android: "delete" }, destructive: true, onPress: () => openStep(chat, "delete") };
    return copy ? [rename, copy, archiveIt, remove] : [rename, archiveIt, remove];
  };

  const hiddenPlaces = useHiddenMobilePlaces(opened);
  const assistant = assistantPlace(agent);
  const assistantVisibility = mobilePlaceVisibility("assistant", hiddenPlaces, role);
  const placeRow = (place: DrawerPlace) => {
    const visibility = mobilePlaceVisibility(place.id, hiddenPlaces, role);
    if (visibility === "hidden") return null;
    const marked = visibility === "owner";
    const badge = place.id === "more" ? unread : undefined;
    const hint = place.label === "More" ? "Inbox, archived sessions and help" : undefined;
    return (
      <NavRow
        key={place.label}
        leading={<NavIcon name={place.icon} />}
        label={place.label}
        badge={badge}
        detail={marked ? "Hidden for members" : undefined}
        selected={pathname === (typeof place.href === "string" ? place.href : place.href.pathname)}
        onPress={() => place.label === "More" ? openPanel("more") : go(place.href)}
        accessibilityLabel={[place.label, marked ? "Hidden for members" : badge ? `${badge} unread in Inbox` : undefined].filter(Boolean).join(", ")}
        accessibilityHint={hint}
      />
    );
  };

  return (
    <View style={[styles.sidebar, { backgroundColor: theme.sidebar }]} accessibilityRole="menu">
      <ScrollView
        ref={drawerScroll}
        scrollEnabled={!dragLocked}
        pointerEvents={appRowsHidden ? "none" : "auto"}
        accessibilityElementsHidden={appRowsHidden}
        importantForAccessibility={appRowsHidden ? "no-hide-descendants" : "auto"}
        style={styles.scroll}
        // Clear of the head's fade at rest, so the first row's fill shows whole; the fade only covers what scrolls up.
        contentContainerStyle={{ paddingTop: head + HEAD_FADE, paddingBottom: foot + 8 }}
        scrollIndicatorInsets={{ top: head, bottom: foot }}
      >
        {/* The Assistant's row with its states (A21·c): unread, thinking, needs you (`assistant-nav-row.tsx`). */}
        {assistant && agent && assistantVisibility !== "hidden" ? (
          <AssistantNavRow
            agent={agent}
            opened={opened}
            label={assistant.label}
            hiddenForMembers={assistantVisibility === "owner"}
            selected={pathname === assistant.href}
            onPress={() => go(assistant.href, true)}
            current={pathname.startsWith("/errand/") ? decodeURIComponent(pathname.slice("/errand/".length)) : undefined}
            onErrand={(id) => go(errandHref(id), true)}
          />
        ) : null}
        <NavRow leading={<NavIcon name="compose" />} label={IOS ? "New" : "New session"} accessibilityLabel="New session" selected={onNewSession} onPress={newSession} />
        {IOS ? <NavRow leading={<NavIcon name="search" />} label="Search" onPress={() => openPanel("search")} /> : null}
        {drawerPlaces().map(placeRow)}

        <DrawerSessions
          sessions={history.sessions}
          drawerOpen={opened && !appRowsHidden}
          drawerScroll={drawerScroll}
          onDragLock={setDragLocked}
          head={head + HEAD_FADE}
          foot={foot}
          titles={history.titles}
          error={history.error}
          loading={history.loading}
          more={history.more}
          onRefresh={history.refresh}
          current={(session) => pathname === `/session/${session.id}`}
          onOpen={(session) => go(sessionHref(session), true)}
          onSeeAll={() => go("/chats")}
          onSyncedDeleted={(id, title) => {
            history.remove({ source: "synced", id });
            if (pathname === `/session/${id}`) newSession();
            history.refresh();
            toast({ text: `Deleted synced copy of “${title}”`, haptic: "success" });
          }}
          onNewProject={(computer, name) => {
            const projectStart = keepProjectStart(core, computer, name);
            go({ pathname: "/session/[id]", params: { id: "new", computer, projectStart } }, true);
          }}
          chatActions={actions}
        />
      </ScrollView>

      <View pointerEvents={appRowsHidden || IOS ? "none" : "auto"} accessibilityElementsHidden={appRowsHidden} importantForAccessibility={appRowsHidden ? "no-hide-descendants" : "auto"} style={[styles.head, { paddingTop: insets.top + (IOS ? 0 : 8) }]} onLayout={(e) => setHead(e.nativeEvent.layout.height)}>
        <EdgeFade color={theme.sidebar} edge="top" style={[StyleSheet.absoluteFill, { bottom: -HEAD_FADE }]} />
        {IOS ? null : <>
        <Pressable
          android_ripple={ripple(theme)}
          style={({ pressed }) => [styles.account, pressed && { backgroundColor: theme.sidebarRow }]}
          onPress={() => openPanel("organizations")}
          disabled={!canSwitch}
          accessibilityRole={canSwitch ? "button" : "header"}
          accessibilityLabel={canSwitch ? `Organization, ${account}` : account}
          accessibilityHint={canSwitch ? "Switches organization" : undefined}
          accessibilityState={{ disabled: !canSwitch, expanded: panel === "organizations" }}
        >
          <AccountMark personal={personal} account={account} logo={current?.imageUrl} person={name} theme={theme} />
          <Text style={[styles.accountName, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
            {account}
          </Text>
          {canSwitch ? <Icon name="chevron" size={13} color={theme.muted} /> : null}
        </Pressable>
        <Pressable
          style={({ pressed }) => [styles.iconButton, pressed && { backgroundColor: theme.sidebarRow }]}
          android_ripple={HEAD_RIPPLE(theme)}
          onPress={() => openPanel("search")}
          accessibilityRole="button"
          accessibilityLabel="Search"
          accessibilityHint="Sessions, settings and artifacts"
        >
          <Icon name="search" size={20} color={theme.foreground} />
        </Pressable>
        </>}
      </View>

      <View pointerEvents={appRowsHidden ? "none" : "auto"} accessibilityElementsHidden={appRowsHidden} importantForAccessibility={appRowsHidden ? "no-hide-descendants" : "auto"} style={[styles.foot, { paddingBottom: Math.max(insets.bottom, 12) }]} onLayout={(e) => setFoot(e.nativeEvent.layout.height)}>
        <EdgeFade color={theme.sidebar} edge="bottom" style={[StyleSheet.absoluteFill, { top: -28 }]} />
        <Pressable
          android_ripple={ripple(theme)}
          style={({ pressed }) => [styles.you, pressed && !ANDROID && { backgroundColor: theme.sidebarRow }]}
          onPress={() => openPanel("account")}
          accessibilityRole="button"
          accessibilityLabel={[name, IOS ? account : undefined, "Account menu"].filter(Boolean).join(", ")}
          accessibilityState={{ expanded: panel === "account" }}
        >
          <View style={[styles.avatar, { backgroundColor: theme.fill }]}>
            {/* Yours, never the account's letter: without a name, a person. */}
            {initials(name) ? (
              <Text style={[styles.avatarText, { color: theme.muted }]} allowFontScaling={false}>
                {initials(name)}
              </Text>
            ) : (
              <Icon name="person" size={15} color={theme.muted} />
            )}
          </View>
          <View style={styles.youText}>
            <Text style={[styles.youName, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
              {name || "Account"}
            </Text>
            {Platform.OS === "ios" ? <Text style={[styles.youOrg, { color: theme.muted }]} numberOfLines={1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>{account}</Text> : null}
          </View>
          <Icon name={Platform.OS === "ios" ? "updown" : "chevron"} size={12} color={theme.muted} />
        </Pressable>
        <Pressable
          android_ripple={ripple(theme)}
          style={({ pressed }) => [styles.iconButton, pressed && { backgroundColor: theme.sidebarRow }]}
          onPress={openSettings}
          accessibilityRole="button"
          accessibilityLabel="Settings"
        >
          <Icon name="settings" size={20} color={theme.foreground} />
        </Pressable>
      </View>

      {panel === "search" ? (
        <Reanimated.View
          style={[StyleSheet.absoluteFill, { backgroundColor: theme.sidebar, transformOrigin: "top right" }]}
          entering={reduceMotion ? FadeIn.duration(120) : searchEntering}
          exiting={FadeOut.duration(DURATION.exit)}
          accessibilityViewIsModal
        >
          <SearchPanel
            recentSessions={history.sessions} recentTitles={history.titles} recentError={history.error}
            onClose={() => {
              Keyboard.dismiss();
              setPanel(null);
            }}
            onNavigate={(href) => go(href, opensInDrawer(href))}
          />
        </Reanimated.View>
      ) : null}

      <ChatMenu
        chat={menu?.chat ?? null}
        start={menu?.start}
        onClose={() => setMenu(null)}
        onChanged={history.refresh}
        onRemoved={(id) => {
          history.remove({ source: "chat", id });
          // Deleted while on screen: a new session takes its place.
          if (pathname === `/session/${id}`) newSession();
        }}
        onError={(message) => Alert.alert("Couldn't do that", message)}
      />
      <Sheet visible={panel === "account" || panel === "organizations" || panel === "more"} onClose={() => setPanel(null)}>
        {panel === "more" ? (
          <>
            <SheetTitle>More</SheetTitle>
            <DrawerMenuRow icon="archive" label="Archived" onPress={() => go("/archived")} />
            <DrawerMenuRow icon="inbox" label="Inbox" detail={unread ? `${unread} unread` : undefined} onPress={() => go("/inbox")} />
            <DrawerMenuRow icon="info" label="Help and feedback" onPress={() => go("/settings-group/more")} />
          </>
        ) : panel === "organizations" ? (
          <OrgChoices
            orgs={orgs ?? []}
            visible
            onClose={() => setPanel((current) => current === "organizations" ? null : current)}
            switching={switcher.switching}
            problem={switcher.problem || orgError}
            loading={!orgs && !orgError}
            offline={unreachable}
            onEnrollmentClosed={refreshOrganizations}
            recent={switcher.recent}
            onPick={switcher.pick}
          />
        ) : (
          <>
            <SheetTitle>{`${name || "Account"} · ${account}`}</SheetTitle>
            <DrawerMenuRow icon="settings" label="Settings" onPress={openSettings} />
            <DrawerMenuRow icon="org" label="Switch organization" disabled={unreachable} detail={unreachable ? "Needs a connection" : account} onPress={() => openPanel("organizations")} />
            <DrawerMenuRow icon="logout" label="Sign out" destructive onPress={() => {
              haptic("tap");
              setPanel(null);
              void signOut().catch((e: Error) => {
                haptic("error");
                Alert.alert("Couldn't sign out", e.message);
              });
            }} />
          </>
        )}
      </Sheet>
    </View>
  );
}

/** The account's mark in the head: Personal is you (your initials, round); a company its logo, else its initial on a tile. */
function AccountMark({ personal, account, logo, person, theme }: { personal: boolean; account: string; logo?: string; person?: string; theme: Theme }) {
  if (personal && initials(person)) {
    return (
      <View style={[styles.mark, { backgroundColor: theme.fill }]}>
        <Text style={[styles.markText, { color: theme.muted }]} allowFontScaling={false}>
          {initials(person)}
        </Text>
      </View>
    );
  }
  return <Initial name={account} size={24} logo={personal ? undefined : logo} />;
}

const SEARCH_ENTER = timing(DURATION.base);
const SEARCH_FADE = timing(DURATION.fast);

const searchEntering = () => {
  "worklet";
  return {
    initialValues: { opacity: 0, transform: [{ translateX: 16 }, { translateY: 6 }, { scale: 0.96 }] },
    animations: {
      opacity: withTiming(1, SEARCH_FADE),
      transform: [{ translateX: withTiming(0, SEARCH_ENTER) }, { translateY: withTiming(0, SEARCH_ENTER) }, { scale: withTiming(1, SEARCH_ENTER) }],
    },
  };
};

const ANDROID = Platform.OS === "android";
const IOS = Platform.OS === "ios";
/** How far the head's fade reaches below it, over the list as it scrolls up. */
const HEAD_FADE = 8;
// An icon button's ripple: borderless, the 44 dp target's circle.
const HEAD_RIPPLE = (theme: Theme) => (ANDROID ? { color: theme.ripple, borderless: true, radius: 22 } : undefined);
// The organization owns its chevron; Search and Settings each have a separate 44 pt target.
const styles = StyleSheet.create({
  sidebar: { flex: 1 },
  scroll: { flex: 1, paddingHorizontal: 8 },
  head: { position: "absolute", top: 0, left: 0, right: 0, flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 12, paddingBottom: 8 },
  account: { flex: 1, minWidth: 0, flexDirection: "row", alignItems: "center", gap: 10, minHeight: 44, paddingHorizontal: 10, paddingVertical: 8, borderRadius: 12, borderCurve: "continuous" },
  accountName: { ...text.body, fontFamily: font.medium, flexShrink: 1 },
  mark: { width: 24, height: 24, borderRadius: 12, alignItems: "center", justifyContent: "center" },
  markText: { fontFamily: font.medium, fontSize: 10, letterSpacing: 0.2 },
  iconButton: { width: 44, height: 44, borderRadius: 12, borderCurve: "continuous", alignItems: "center", justifyContent: "center" },
  foot: { position: "absolute", left: 0, right: 0, bottom: 0, flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 12, paddingTop: 8 },
  you: {
    flex: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: 10,
    minHeight: 44,
    paddingHorizontal: 10,
    borderRadius: 12,
    borderCurve: "continuous",
    ...(ANDROID ? { borderRadius: 28, overflow: "hidden" as const } : null),
  },
  youText: { flex: 1, minWidth: 0 },
  youName: { ...text.body, fontFamily: font.medium },
  youOrg: { ...text.caption, fontFamily: font.regular },
  avatar: { width: 24, height: 24, borderRadius: 12, alignItems: "center", justifyContent: "center" },
  avatarText: { fontFamily: font.medium, fontSize: 10 },
});
