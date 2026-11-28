import type { ChatSessionSummary } from "@nexus/kit";
import { useEffect, useMemo, useRef, useState, type ComponentRef, type RefObject } from "react";
import { ActivityIndicator, Keyboard, Platform, Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { ago, spokenAgo } from "@/ago";
import { useCore, useStored } from "@/auth";
import { TextInput } from "@/controls";
import { forgetTitle } from "@/code-reader";
import type { TitleState } from "@/code-sessions";
import { DRAWER_GROUPINGS, drawerAgents, filterDrawerSessions, groupDrawerSessions, loudest, projectComputer, rowState, stateWord, type DrawerGrouping, type ProjectGroup } from "@/drawer-groups";
import { EMPTY_DRAWER_ORGANIZATION, moveDrawerSession, organizeDrawerSessions, type DrawerOrganization } from "@/drawer-organization";
import { DrawerSectionGhost, DrawerSectionGrip, useDrawerSectionDrag } from "@/drawer-section-drag";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { ProviderMark } from "@/provider-mark";
import { NavIcon, NavRow, NavSection, SessionMark } from "@/nav-row";
import { RowMenu, type RowAction } from "@/row-menu";
import { sessionDetail, sessionKey, sessionTitle, useHistoryTitles, type UnifiedSession } from "@/session-history";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { SwipeActions, type SwipeAction } from "@/swipe-actions";
import { useToast } from "@/toast";
import { useSessionPins } from "@/use-session-pins";
import { ACCESSIBILITY_FONT_SCALE } from "@/text-size";
import { font, text, useTheme } from "@/theme";

/** A project shows this many sessions until Show more. */
const PROJECT_ROWS = 5;

const AGENT_MARKS = new Map<string, string>([
  ["claude", "anthropic/claude"], ["claude-code", "anthropic/claude"],
  ["codex", "openai/codex"], ["nexus", "nexus/auto"], ["gemini-cli", "google/gemini"],
]);

/**
 * The drawer's sessions, grouped as the desktop's sidebar groups them (`drawer-groups.ts`): Projects, each with its
 * sessions under it and a tap to fold it, then Recent, then See all. Each row wears its session's mark, and says
 * "Working" or "Needs you" in place of its age while its agent works or waits on you. A folded project shows its
 * loudest mark. Folds last while the app runs; the names they hold are decrypted, so they never reach the disk.
 */
export function DrawerSessions({
  sessions,
  drawerOpen,
  drawerScroll,
  head,
  foot,
  onDragLock,
  titles: recentTitles,
  error,
  loading,
  more,
  onRefresh,
  current,
  onOpen,
  onSeeAll,
  onSyncedDeleted,
  onNewProject,
  chatActions,
}: {
  sessions: readonly UnifiedSession[];
  drawerOpen: boolean;
  drawerScroll: RefObject<ComponentRef<typeof ScrollView> | null>;
  head: number;
  foot: number;
  onDragLock: (locked: boolean) => void;
  titles: Record<string, TitleState>;
  error?: string;
  loading: boolean;
  onRefresh: () => void;
  /** The history holds more than it loaded. */
  more: boolean;
  /** The id of the session on screen. */
  current: (session: UnifiedSession) => boolean;
  onOpen: (session: UnifiedSession) => void;
  onSeeAll: () => void;
  onSyncedDeleted: (id: string, title: string) => void;
  onNewProject: (computerId: string, projectName: string) => void;
  chatActions: (chat: ChatSessionSummary) => RowAction[];
}) {
  const theme = useTheme();
  const core = useCore();
  const currentCore = useRef<typeof core | null>(core);
  currentCore.current = core;
  useEffect(() => () => { if (currentCore.current === core) currentCore.current = null; }, [core]);
  const { height } = useWindowDimensions();
  const [folded, setFolded] = useState<ReadonlySet<string>>(() => new Set());
  const [opened, setOpened] = useState<ReadonlySet<string>>(() => new Set());
  const preference = useStored<DrawerGrouping>("choice", "session-grouping");
  const [grouping, setGrouping] = useState(() => preference.load() ?? "folder");
  const [panel, setPanel] = useState<"grouping" | "filter" | null>(null);
  const [query, setQuery] = useState("");
  const toast = useToast();
  const storage = useStored<DrawerOrganization>("choice", "session-organization");
  const [layout, setLayout] = useState(() => ({ storage, value: storage.load() ?? EMPTY_DRAWER_ORGANIZATION }));
  const organization = layout.storage === storage ? layout.value : storage.load() ?? EMPTY_DRAWER_ORGANIZATION;
  const save = (value: DrawerOrganization) => {
    storage.save(value);
    setLayout({ storage, value });
  };
  const [menu, setMenu] = useState<{ storage: typeof storage; session: UnifiedSession } | null>(null);
  const [sectionPicker, setSectionPicker] = useState<{ core: typeof core; storage: typeof storage; session: UnifiedSession } | null>(null);
  const [sectionName, setSectionName] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<{ storage: typeof storage; session: UnifiedSession } | null>(null);
  const [deleteError, setDeleteError] = useState("");
  const deletingRef = useRef(false);
  const [deleting, setDeleting] = useState(false);
  const deleteTarget = confirmation?.storage === storage ? confirmation.session : null;
  const selectedMenu = menu?.storage === storage ? menu.session : null;
  const moving = sectionPicker?.core === core && sectionPicker.storage === storage ? sectionPicker.session : null;
  const deleteCopy = async () => {
    if (!deleteTarget || deletingRef.current) return;
    deletingRef.current = true;
    setDeleting(true);
    setDeleteError("");
    try {
      await core.deleteSyncedThread(deleteTarget.id);
      if (currentCore.current !== core) return;
      forgetTitle(core, deleteTarget.id);
      const key = sessionKey(deleteTarget);
      if (pins.keys.includes(key)) {
        if (!pins.set("synced", deleteTarget.id, false)) {
          save({ ...organization, pinned: organization.pinned.filter((id) => id !== key) });
        }
      }
      onSyncedDeleted(deleteTarget.id, sessionTitle(deleteTarget, titles));
      setConfirmation(null);
    } catch (error) {
      if (currentCore.current === core) {
        haptic("error");
        setDeleteError(error instanceof Error ? error.message : "The synced copy couldn’t be deleted.");
      }
    } finally {
      deletingRef.current = false;
      setDeleting(false);
    }
  };
  const closeSectionPicker = () => { setSectionPicker(null); setSectionName(null); };
  const move = (session: UnifiedSession, name: string | null) => {
    const key = sessionKey(session);
    const previous = organization.sections.find(({ sessions }) => sessions.includes(key))?.name ?? null;
    const next = moveDrawerSession(organization, key, name);
    save(next);
    Keyboard.dismiss();
    closeSectionPicker();
    toast({ text: name ? `Moved to ${name}` : "Moved out of section", haptic: "success", action: {
      label: "Undo", onPress: () => { if (currentCore.current === core) save(moveDrawerSession(storage.load() ?? next, key, previous)); },
    } });
  };
  useEffect(() => {
    const saved = preference.load();
    setGrouping(DRAWER_GROUPINGS.find(({ key }) => key === saved)?.key ?? "folder");
    setQuery("");
  }, [preference]);
  const pins = useSessionPins(core, drawerOpen, organization.pinned);
  const olderPins = useMemo(() => {
    const recentKeys = new Set(sessions.map(sessionKey));
    return pins.rows.filter((row) => !recentKeys.has(sessionKey(row)));
  }, [pins.rows, sessions]);
  const pinTitles = useHistoryTitles(core, olderPins);
  const titles = { ...pinTitles, ...recentTitles };
  const pinnedKeys = new Set(pins.keys);
  const available = [...sessions, ...olderPins.filter((row) => pinnedKeys.has(sessionKey(row)))];
  const filtered = filterDrawerSessions(available, query);
  const organized = organizeDrawerSessions(filtered, { ...organization, pinned: [...pins.keys] });
  const pinnedRows = new Set(organized.pinned.map(sessionKey));
  const sections = organized.sections.filter(({ sessions }) => sessions.length);
  const targetNames = new Set(sections.map(({ name }) => name));
  const dragScope = useMemo(() => ({}), [core, storage]);
  const drag = useDrawerSectionDrag({
    scope: dragScope, enabled: drawerOpen && !panel && !selectedMenu && !moving,
    targetNames, scroll: drawerScroll, head, foot, onLock: onDragLock,
    onDrop: (key, name) => {
      const session = filtered.find((candidate) => candidate.source === "chat" && sessionKey(candidate) === key);
      if (!session || pinnedRows.has(key) || !targetNames.has(name)) return;
      move(session, name);
    },
  });
  const draggedKey = drag.dragging?.key;
  useEffect(() => {
    if (draggedKey && (!filtered.some((session) => sessionKey(session) === draggedKey) || pinnedRows.has(draggedKey))) {
      drag.cancel();
    }
  }, [draggedKey, filtered, pinnedRows, drag.cancel]);
  const groups = groupDrawerSessions(organized.remaining, titles, grouping);
  const agents = drawerAgents(available);
  const setting = DRAWER_GROUPINGS.find(({ key }) => key === grouping)?.label ?? "Folder";
  const activeFilter = !!query.replace(/^agent:\s*/i, "").trim();
  const toggle = (set: ReadonlySet<string>, name: string) => {
    const next = new Set(set);
    if (!next.delete(name)) next.add(name);
    return next;
  };

  const actions = (session: UnifiedSession): RowAction[] => {
    const key = sessionKey(session);
    const pinned = pins.keys.includes(key);
    const pin: RowAction = {
      key: "pin", label: pinned ? "Unpin" : "Pin",
      icon: { ios: pinned ? "pin.slash" : "pin", android: "push_pin" },
      onPress: () => {
        haptic("selection");
        if (pins.set(session.source, session.id, !pinned)) return;
        save({ ...organization, pinned: pinned ? organization.pinned.filter((id) => id !== key) : [key, ...organization.pinned] });
      },
    };
    const moveTo: RowAction = {
      key: "move", label: "Move to section", icon: { ios: "folder", android: "folder" },
      onPress: () => {
        haptic("tap");
        const selection = { core, storage, session };
        if (selectedMenu) setSectionPicker(selection);
        else setTimeout(() => setSectionPicker(selection), Platform.OS === "ios" ? 400 : 250);
      },
    };
    const rowActions = [pin, moveTo];
    if (session.source === "chat") return [...rowActions, ...chatActions(session.chat)];
    if (!pins.userId || session.thread.personId !== pins.userId) return rowActions;
    return [...rowActions, {
      key: "delete-synced", label: "Delete synced copy", icon: { ios: "trash", android: "delete" }, destructive: true,
      onPress: () => {
        haptic("tap");
        setDeleteError("");
        const next = { storage, session };
        if (selectedMenu) setConfirmation(next);
        else setTimeout(() => setConfirmation(next), Platform.OS === "ios" ? 400 : 250);
      },
    }];
  };

  const row = (session: UnifiedSession) => {
    const title = sessionTitle(session, titles);
    const key = sessionKey(session);
    const section = organization.sections.find(({ sessions }) => sessions.includes(key))?.name;
    const hasTarget = sections.some(({ name }) => name !== section);
    const canMove = session.source === "chat" && !pinnedRows.has(key) && !!organization.sections.length;
    const rowActions = actions(session);
    const selected = current(session);
    const state = rowState(session, titles);
    const word = stateWord(state);
    const draw = (onLongPress?: () => void) => (
      <NavRow
        leading={<SessionMark state={state} current={selected} />}
        label={title}
        meta={ago(session.updatedAt)}
        stateWord={word}
        selected={selected}
        onPress={() => onOpen(session)}
        onLongPress={onLongPress}
        accessibilityLabel={[title, sessionDetail(session, titles), word ?? spokenAgo(session.updatedAt)].join(", ")}
      />
    );
    return (
      <View key={key} style={[styles.rowWithActions, draggedKey === key && styles.dragSource]}>
        <View style={styles.rowBody}>
          <SwipeActions actions={rowActions.flatMap<SwipeAction>((action) => {
            if (action.key === "pin") return [{ ...action, icon: "pin", color: theme.fill, ink: theme.foreground }];
            if (action.key === "archive") return [{ ...action, icon: "archive", color: theme.fill, ink: theme.foreground }];
            return [];
          })}>
            {/* The drawer's colour on the row itself, so Pin and Archive stay hidden under it until the swipe. */}
            <View style={[styles.rowWithActions, { backgroundColor: theme.sidebar }]}>
              <View style={styles.rowBody}>
                <RowMenu title={title} actions={rowActions}>{draw}</RowMenu>
              </View>
              <Pressable accessibilityRole="button" accessibilityLabel={`Actions for ${title}`}
                style={({ pressed }) => [styles.rowAction, pressed && { backgroundColor: theme.sidebarRow }]}
                onPress={() => { haptic("tap"); setMenu({ storage, session }); }}>
                <Icon name="more" size={15} color={theme.muted} />
              </Pressable>
            </View>
          </SwipeActions>
        </View>
        {canMove ? <DrawerSectionGrip title={title} source={hasTarget ? { key, title, current: section } : undefined}
          controls={drag} onPicker={() => {
            if (draggedKey) return;
            haptic("tap");
            setSectionPicker({ core, storage, session });
          }} /> : null}
      </View>
    );
  };

  const project = (group: ProjectGroup) => {
    const key = group.key ?? group.name;
    const isFolded = folded.has(key);
    const computer = grouping === "folder" ? projectComputer(group, titles) : undefined;
    const shown = opened.has(key) ? group.sessions : group.sessions.slice(0, PROJECT_ROWS);
    const hidden = group.sessions.length - shown.length;
    const mark = isFolded ? loudest(group.sessions.map((session) => rowState(session, titles))) : null;
    return (
      <View key={`project:${key}`}>
        <View style={styles.rowWithActions}>
          <View style={styles.rowBody}><NavRow
          leading={<NavIcon name={grouping === "folder" ? "repositories" : grouping === "machine" ? "command" : "history"} />}
          label={<Text style={[styles.project, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>{group.name}</Text>}
          trailing={
            <View style={styles.trailing}>
              {mark ? <SessionMark state={mark} /> : null}
              <View style={isFolded ? styles.chevronFolded : null}>
                <Icon name="chevron" size={13} color={theme.tertiary} />
              </View>
            </View>
          }
          onPress={() => {
            haptic("selection");
            setFolded((set) => toggle(set, key));
          }}
          accessibilityLabel={[group.name, `${group.sessions.length} ${group.sessions.length === 1 ? "session" : "sessions"}`, mark ? stateWord(mark) : null].filter(Boolean).join(", ")}
          onLongPress={() => { haptic("longPress"); setPanel("grouping"); }}
          accessibilityHint={isFolded ? "Shows its sessions. Touch and hold to group." : "Hides its sessions. Touch and hold to group."}
          accessibilityState={{ expanded: !isFolded }}
          /></View>
          {computer ? <Pressable accessibilityRole="button" accessibilityLabel={`New session in ${group.name}`}
            style={({ pressed }) => [styles.rowAction, pressed && { backgroundColor: theme.sidebarRow }]}
            onPress={() => onNewProject(computer, group.name)}>
            <Icon name="plus" size={18} color={theme.foreground} />
          </Pressable> : null}
        </View>
        {isFolded ? null : shown.map(row)}
        {!isFolded && hidden > 0 ? (
          <NavRow leading={null} label={<Text style={[styles.more, { color: theme.muted }]}>{`Show ${hidden} more`}</Text>}
            onPress={() => { haptic("tap"); setOpened((set) => toggle(set, key)); }} />
        ) : null}
      </View>
    );
  };

  return (
    <View ref={drag.root} collapsable={false}>
      <NavSection>Sessions</NavSection>
      <NavRow leading={<NavIcon name="search" />} label={activeFilter ? query : "Filter by agent"}
        meta={activeFilter ? String(filtered.length) : undefined} onPress={() => { haptic("tap"); if (!query) setQuery("agent: "); setPanel("filter"); }} />
      <NavRow leading={<NavIcon name="sliders" />} label="Group and sort" meta={setting}
        onPress={() => { haptic("tap"); setPanel("grouping"); }} />
      {organized.pinned.length ? <><NavSection>Pinned</NavSection>{organized.pinned.map(row)}</> : null}
      {sections.map(({ name, sessions }) => (
        <View key={name} onLayout={({ nativeEvent }) => drag.layout(name, nativeEvent.layout)}>
          <NavSection>{name}</NavSection>
          {sessions.map(row)}
          {drag.hover === name ? <View pointerEvents="none" style={[styles.dropTarget, { borderColor: theme.primary }]} /> : null}
        </View>
      ))}
      {groups.projects.length && grouping === "folder" ? <NavSection>Projects</NavSection> : null}
      {groups.projects.map(project)}
      {groups.recent.length && grouping === "folder" ? <NavSection>Recent</NavSection> : null}
      {error ? (
        <>
          <Text style={[styles.note, { color: theme.muted }]} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE} accessibilityLiveRegion="polite">{error}</Text>
          <NavRow leading={<NavIcon name="restore" />} label="Retry" onPress={() => { haptic("tap"); onRefresh(); }} />
        </>
      ) : loading && !sessions.length ? <ActivityIndicator color={theme.muted} accessibilityLabel="Loading sessions" /> : null}
      {groups.recent.map(row)}
      {!filtered.length && !loading && !error ? <Text style={[styles.note, { color: theme.muted }]}>{activeFilter ? "No sessions match this agent." : "Your sessions show here."}</Text> : null}
      {activeFilter ? <NavRow leading={null} label="Clear filter" onPress={() => { haptic("tap"); setQuery(""); }} /> : null}
      {groups.more || more || sessions.length ? <NavRow leading={null} label="View all sessions" onPress={onSeeAll} /> : null}
      <Sheet visible={selectedMenu !== null || moving !== null || deleteTarget !== null} onClose={() => {
        if (deletingRef.current) return;
        setMenu(null);
        setConfirmation(null);
        closeSectionPicker();
      }}>
        <ScrollView style={{ maxHeight: height * 0.6 }} keyboardShouldPersistTaps="handled">
        {selectedMenu ? <>
          <SheetTitle>{sessionTitle(selectedMenu, titles)}</SheetTitle>
          {actions(selectedMenu).map((action) => (
            <SheetRow key={action.key} label={action.label} tone={action.destructive ? "danger" : "normal"}
              onPress={() => { setMenu(null); action.onPress(); }} />
          ))}
        </> : null}
        {moving ? sectionName === null ? <>
          <SheetTitle>Move to section</SheetTitle>
          {organization.sections.map(({ name }) => <SheetRow key={name} label={name} onPress={() => move(moving, name)} />)}
          {organization.sections.some(({ sessions }) => sessions.includes(sessionKey(moving))) ?
            <SheetRow label="Remove from section" onPress={() => move(moving, null)} /> : null}
          <SheetRow label="New section" onPress={() => { haptic("tap"); setSectionName(""); }} />
        </> : <>
          <SheetTitle>New section</SheetTitle>
          <TextInput value={sectionName} onChangeText={setSectionName} placeholder="Section name" accessibilityLabel="Section name"
            maxLength={60} returnKeyType="done" style={[styles.field, { backgroundColor: theme.fill, color: theme.foreground }]} />
          <SheetRow label="Create and move" tone={sectionName.trim() ? "primary" : "muted"} onPress={() => {
            const name = sectionName.trim();
            if (!name) return;
            const existing = organization.sections.find((section) => section.name.toLowerCase() === name.toLowerCase());
            move(moving, existing?.name ?? name);
          }} />
        </> : null}
          {deleteTarget ? <>
            <SheetTitle>{`Delete the synced copy of ${sessionTitle(deleteTarget, titles)}?`}</SheetTitle>
            <Text style={[styles.note, { color: theme.muted }]}>This permanently removes the encrypted history and keys stored in Gateway.</Text>
            {deleteError ? <Text style={[styles.note, { color: theme.danger }]} accessibilityLiveRegion="polite">{deleteError}</Text> : null}
            {deleting ? <SheetTitle>Deleting…</SheetTitle> : <>
              <SheetRow label="Delete synced copy" tone="danger" onPress={() => void deleteCopy()} />
              <SheetRow label="Cancel" onPress={() => setConfirmation(null)} />
            </>}
          </> : null}
        </ScrollView>
      </Sheet>
      <Sheet visible={panel !== null} onClose={() => setPanel(null)}>
        {panel === "grouping" ? (
          <>
            <SheetTitle>Group by</SheetTitle>
            {DRAWER_GROUPINGS.map(({ key, label }) => (
              <SheetRow key={key} label={label} tone={key === grouping ? "primary" : "normal"} onPress={() => {
                haptic("selection");
                setGrouping(key);
                preference.save(key);
                setFolded(new Set());
                setOpened(new Set());
                setPanel(null);
              }} />
            ))}
          </>
        ) : (
          <>
            <SheetTitle>Filter by agent</SheetTitle>
            <TextInput value={query} onChangeText={setQuery} placeholder="agent: Codex"
              accessibilityLabel="Agent filter" autoCapitalize="none" autoCorrect={false} returnKeyType="search"
              onSubmitEditing={() => { Keyboard.dismiss(); setPanel(null); }}
              style={[styles.field, { backgroundColor: theme.fill, color: theme.foreground }]} />
            <ScrollView style={{ maxHeight: height * 0.45 }} keyboardShouldPersistTaps="handled">
              {agents.map(({ id, label }) => {
                const mark = AGENT_MARKS.get(id);
                return <SheetRow key={id} label={label} leading={mark ? <ProviderMark model={mark} /> : undefined} onPress={() => {
                  haptic("selection");
                  setQuery(`agent: ${label}`);
                  Keyboard.dismiss();
                  setPanel(null);
                }} />;
              })}
            </ScrollView>
            <SheetRow label="Clear filter" onPress={() => { haptic("tap"); setQuery(""); setPanel(null); }} />
          </>
        )}
      </Sheet>
      {drag.dragging ? <DrawerSectionGhost title={drag.dragging.title} motion={drag.style} /> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  rowWithActions: { flexDirection: "row", alignItems: "center" },
  rowBody: { flex: 1, minWidth: 0 },
  dragSource: { opacity: 0.35 },
  dropTarget: { ...StyleSheet.absoluteFill, borderWidth: 2, borderRadius: 12 },
  rowAction: { width: 44, minHeight: 44, alignItems: "center", justifyContent: "center", borderRadius: 12 },
  field: { ...text.body, minHeight: 44, paddingHorizontal: 12, paddingVertical: 8, borderRadius: 12 },
  project: { ...text.body, fontFamily: font.medium },
  trailing: { flexDirection: "row", alignItems: "center", gap: 6 },
  chevronFolded: { transform: [{ rotate: "-90deg" }] },
  note: { ...text.sm, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 8 },
  more: { ...text.body, fontFamily: font.regular },
});
