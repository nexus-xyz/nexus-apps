import { router, useNavigation } from "expo-router";
import { DrawerActions } from "expo-router/react-navigation";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, Alert, Keyboard, Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { ScreenKeyboardAvoidingView } from "./screen-keyboard";
import { availableBranches, projectEffortUnknown, projectRoutes, replacesProjectEffort, validSelection, type ComputerSession } from "./computer-session";
import { useAuth, useCore } from "./auth";
import { useComputerSession, useComputerState } from "./computers";
import { takeTransferredDraft } from "./connection-intent";
import { Composer } from "./conversation-composer";
import { ComposerSend, composerControls, INPUT_PAD } from "./composer-controls";
import { linesOf } from "./composer-text";
import { ProviderMark } from "./provider-mark";
import { TextInput } from "./controls";
import { HistorySuggestions } from "./history-suggestions";
import { NativeSessionHeader, SessionDetails } from "./native-session-header";
import { COMPUTER_EFFORT, RemoteChoice, RemoteEfforts, RemoteModels, RemoteRoutes, SettingsAction, SettingsChip, SettingsNote, effortLabel } from "./remote-settings-ui";
import { isComputerAgent } from "@nexus/kit/computer-sessions";
import { Icon } from "./icon";
import { agentName } from "./code-sessions";
import { failureTitle } from "./connection-errors";
import { greeting } from "./greeting";
import { haptic } from "./haptics";
import { Blank } from "./list";
import { NexusMark } from "./nexus-mark";
import { RunOn } from "./run-on";
import { Sheet, SheetTitle } from "./sheet";
import { SidebarButton, TopBar } from "./top-bar";
import { font, text, useTheme } from "./theme";

export function ComputerComposer({ computerId, startProject }: { computerId: string; startProject?: string }) {
  const { session, error, cause } = useComputerSession(computerId);
  const navigation = useNavigation();
  const theme = useTheme();
  // A network error retries on its own; only a pairing problem needs Connect phone.
  if (!session) return <View style={[styles.screen, { backgroundColor: theme.screen }]}>
    {error ? <Blank title={failureTitle(cause ?? "failed")} body={cause === "network" ? `${error} Retrying automatically.` : error}
      action={cause === "pairing" ? { label: "Connect phone", onPress: () => router.push("/connect") } : undefined} /> : <ActivityIndicator style={{ flex: 1 }} />}
    <RunOn computerId={computerId} />
    <TopBar left={<SidebarButton onPress={() => navigation.dispatch(DrawerActions.openDrawer())} />} center={<Text style={[styles.heading, { color: theme.foreground }]}>New session</Text>} centered />
  </View>;
  return <Compose session={session} computerId={computerId} startProject={startProject} />;
}
function Compose({ session, computerId, startProject }: { session: ComputerSession; computerId: string; startProject?: string }) {
  const state = useComputerState(session);
  const { height } = useWindowDimensions();
  const core = useCore();
  const { name } = useAuth();
  const theme = useTheme();
  const navigation = useNavigation();
  const [focused, setFocused] = useState(false);
  const [lines, setLines] = useState(1);
  const [picker, setPicker] = useState<"summary" | "project" | "branch" | "model" | "effort" | "payer" | null>(null);
  const [pickerQuery, setPickerQuery] = useState("");
  useEffect(() => setPickerQuery(""), [picker]);
  const initialized = useRef<ComputerSession | undefined>(undefined);
  const [prefilling, setPrefilling] = useState(Boolean(startProject));
  const computer = state.computers.find((c) => c.computerId === computerId);
  const project = state.projects.find((p) => p.id === state.projectId);
  const reach = computer ? session.reach(computer) : "offline";
  const awake = reach === "awake";
  const reachText = awake ? "Awake" : reach === "off" ? "Remote control off" : "Offline";
  useEffect(() => {
    if (!state.loaded || initialized.current === session) return;
    initialized.current = session;
    const transferred = takeTransferredDraft(core);
    if (transferred !== undefined) session.setDraft(transferred);
    // Describing projects is a read operation. A restored pending request is checked, never submitted again here.
    if (!state.pending && !state.created) void session.describe();
  }, [state.loaded, session, core]);
  useEffect(() => { if (state.created?.ready) { const created = state.created; session.opened(); router.replace({ pathname: "/session/[id]", params: { id: created.threadId, source: "synced" } }); } }, [state.created, session]);
  useEffect(() => {
    const snapshot = session.snapshot();
    if (!prefilling || !snapshot.loaded || snapshot.preparing || snapshot.pending || snapshot.created || snapshot.unreadable) return;
    const matches = snapshot.projects.filter((project) => project.name.trim() === startProject);
    const [target] = matches;
    if (!target || matches.length !== 1) return;
    if (snapshot.projectId !== target.id) {
      if (snapshot.draft.trim()) return;
      session.selectProject(target.id);
    }
    if (session.snapshot().projectId === target.id) setPrefilling(false);
  }, [prefilling, session, startProject, state]);
  const discard = () => Alert.alert("Discard the saved draft?", "This phone couldn't open it. A request it held may already have run. Check your session history before starting again.", [{ text: "Keep it", style: "cancel" }, { text: "Discard", style: "destructive", onPress: () => void session.discardSaved() }]);
  const dismiss = () => Alert.alert("Stop checking this request?", "It may already have run. Check your session history before starting again. Your draft will stay here.", [{ text: "Keep checking", style: "cancel" }, { text: "Stop checking", onPress: () => session.dismiss() }]);
  const pending = state.pending;
  const busy = state.preparing || !!pending || !!state.created;
  const settings = project?.settings;
  const agent = settings ? agentName(settings.agent) : null;
  // A newer computer's agent: the session starts with the project's own settings on the computer.
  const unknownAgent = !!settings && !isComputerAgent(settings.agent);
  const supports = !!computer?.capabilities.includes("settings");
  const routes = projectRoutes(project, state.agents);
  const selection = state.selection;
  // The project's level is still the one set on the computer, which this phone cannot name.
  const blocked = !!project && !!selection && replacesProjectEffort(project, selection, state.selectionEffortPicked);
  const kept = !!project && projectEffortUnknown(project) && (!selection || blocked);
  const route = routes.find((r) => r.via === selection?.via && r.payer === selection.payer);
  const selectedModel = route?.models.find((m) => m.id === selection?.model);
  const branches = availableBranches(state, project);
  const branchBlocked = !!state.baseBranch && !branches?.includes(state.baseBranch);
  const chooseBranches = () => {
    Keyboard.dismiss();
    setPicker("branch");
    if (awake && !busy) void session.describeBranches();
  };
  const nameContains = (value: string) => value.toLowerCase().includes(pickerQuery.trim().toLowerCase());
  const shownProjects = state.projects.filter((entry) => nameContains(entry.name) || nameContains(entry.locationHint));
  const shownBranches = branches?.filter(nameContains);
  const choiceValid = !selection || !!project && validSelection(project, state.agents, selection);
  const disabled = prefilling || state.unreadable || !state.loaded || !awake || !project || !state.draft.trim() || busy || !choiceValid || blocked || branchBlocked;
  const unreachable = reach === "off" ? `Turn on Remote control on ${computer?.name ?? "this computer"}.` : "Waiting for this computer. Reconnects automatically when Nexus is available.";
  const notice = state.unreadable ? "This phone couldn't open its saved draft for this computer. Discard it to start again. A request it held may already have run." : state.error ?? state.notice ?? (!awake ? unreachable : branchBlocked ? "Review your starting branch before sending." : !choiceValid ? "Choices changed. Review Who pays and Model before starting." : blocked ? "This project’s reasoning effort is set on the computer. Choose a reasoning effort before starting." : null);
  const projectNotice = prefilling && !busy && state.projects.length
    ? state.projects.some((project) => project.name.trim() === startProject)
      ? `Confirm the project to start a session in ${startProject}.`
      : `${startProject} isn't in this computer's saved projects. Choose a project to continue.`
    : null;
  const payerKind = route?.payer ?? project?.payer?.kind;
  const payerText = payerKind === "plan" ? "Plan" : payerKind === "apiKey" ? "API key" : payerKind === "gateway" ? "Nexus" : "Who pays";
  return <ScreenKeyboardAvoidingView style={[styles.screen, { backgroundColor: theme.screen }]} behavior="padding">
    <NativeSessionHeader title="New session" origin={computer?.name ?? "Computer"} status={reachText} onSidebar={() => navigation.dispatch(DrawerActions.openDrawer())} onDetails={() => { Keyboard.dismiss(); setPicker("summary"); }} />
    <ScrollView contentContainerStyle={styles.body} keyboardShouldPersistTaps="handled">
      <View style={styles.mark}><NexusMark size={40} /></View>
      <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">{greeting(name)}</Text>
      <Text style={[styles.description, { color: theme.muted }]}>{computer?.name ?? "Your computer"} · {reachText}</Text>
      <HistorySuggestions />
    </ScrollView>
    <View>
      {notice || projectNotice ? <View style={styles.status}><Text accessibilityLiveRegion="polite" numberOfLines={2} style={[styles.notice, { color: state.error ? theme.danger : theme.muted }]}>{notice ?? projectNotice}</Text></View> : null}
      {state.unreadable ? <View style={styles.actions}><Pressable onPress={discard} style={styles.action} accessibilityRole="button"><Text style={[styles.label, { color: theme.foreground }]}>Discard saved draft</Text></Pressable></View> : null}
      {state.created && !state.created.ready ? <View style={styles.actions}>
        {state.created.stalled ? <>
          <Pressable onPress={() => { const id = session.openAnyway(); if (id) router.replace({ pathname: "/session/[id]", params: { id, source: "synced" } }); }} style={styles.action} accessibilityRole="button"><Text style={[styles.label, { color: theme.foreground }]}>Open session</Text></Pressable>
          <Pressable onPress={() => session.startAnother()} style={styles.action} accessibilityRole="button" accessibilityHint="The started session stays on the computer"><Text style={[styles.label, { color: theme.foreground }]}>Start another</Text></Pressable>
        </> : null}
        <Pressable onPress={() => void session.retry()} style={styles.action} accessibilityRole="button"><Text style={[styles.label, { color: state.created.stalled ? theme.muted : theme.foreground }]}>Check session</Text></Pressable>
      </View> : null}
      {pending ? <View style={styles.actions}>
        {pending.state !== "uncertain" ? <Pressable onPress={() => void session.retry()} style={styles.action} accessibilityRole="button"><Text style={[styles.label, { color: theme.foreground }]}>Check request</Text></Pressable> : null}
        {pending.state === "uncertain" || session.now() >= pending.envelope.expiresAt ? <Pressable onPress={dismiss} style={styles.action} accessibilityRole="button"><Text style={[styles.label, { color: theme.muted }]}>Stop checking</Text></Pressable> : null}
      </View> : null}
      <Composer theme={theme} halo={busy ? "working" : "waiting"} focused={focused} box={lines > 1}>
        <View style={styles.sourceRow}><View style={styles.destination}><RunOn computerId={computerId} draft={state.draft} disabled={busy} /></View><SettingsChip label="Project" value={prefilling ? (startProject ?? "Choose project") : (project?.name ?? "Choose project")} disabled={busy} onPress={() => setPicker("project")} /></View>
        <TextInput editable={state.loaded && !state.preparing && pending?.kind !== "startSession" && !state.created} value={state.draft} onChangeText={(value) => session.setDraft(value)} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
          onContentSizeChange={(event) => setLines(linesOf(event.nativeEvent.contentSize.height - INPUT_PAD))}
          multiline scrollEnabled textAlignVertical="top" placeholder={agent ? `Ask ${agent}…` : "Build anything"} placeholderTextColor={theme.muted} accessibilityLabel="Message for computer" style={[composerControls.input, { color: theme.foreground }]} />
        <View style={styles.toolbar}>
          <View style={styles.choices}>
            <Pressable onPress={() => { haptic("tap"); Keyboard.dismiss(); setPicker("summary"); }} disabled={busy} style={composerControls.target} accessibilityRole="button" accessibilityLabel="Session settings" accessibilityState={{ disabled: busy }}><Icon name="more" size={18} color={theme.muted} /></Pressable>
            <SettingsChip label="Model" value={selectedModel?.name ?? selection?.model ?? settings?.model ?? "Model"} leading={<ProviderMark model={selection?.model ?? settings?.model ?? "nexus"} size={14} />} disabled={busy} onPress={() => setPicker("model")} />
            <SettingsChip label="Who pays" value={route?.label ?? project?.payer?.label ?? "Who pays"} displayValue={payerText} leading={<Icon name="members" size={14} color={theme.muted} />} disabled={busy} onPress={() => setPicker("payer")} />
          </View>
          <ComposerSend theme={theme} onPress={() => { haptic("send"); void session.start(); }} disabled={disabled} loading={busy} label="Start session" />
        </View>
      </Composer>
    </View>
    <Sheet visible={picker !== null} onClose={() => setPicker(null)}><SheetTitle>{picker === "project" ? "Saved project" : picker === "branch" ? "Starting branch" : picker === "model" ? "Model" : picker === "effort" ? "Reasoning effort" : picker === "payer" ? "Who pays" : "New session settings"}</SheetTitle>
      <ScrollView key={picker} style={{ maxHeight: height * 0.7 }} keyboardShouldPersistTaps="handled">
      {picker === "summary" ? <><SessionDetails title="New session" origin={computer?.name ?? "Computer"} project={project?.name} agent={settings ? agentName(settings.agent) : undefined} />
        <RemoteChoice label="Project" detail={project?.name ?? "Choose a saved project"} disabled={busy} onPress={() => setPicker("project")} />
        <RemoteChoice label="Branch" detail={state.baseBranch ?? "Project default"} disabled={busy || !project} onPress={chooseBranches} />
        <RemoteChoice label="Model" detail={selectedModel?.name ?? selection?.model ?? settings?.model ?? "Computer default"} disabled={busy} onPress={() => setPicker("model")} />
        <RemoteChoice label="Reasoning effort" detail={kept ? COMPUTER_EFFORT : effortLabel(selection?.effort ?? null)} disabled={busy || (!selectedModel?.efforts.length && !blocked)} onPress={() => setPicker("effort")} />
        <RemoteChoice label="Who pays" detail={route?.label ?? project?.payer?.label ?? "Computer default"} disabled={busy} onPress={() => setPicker("payer")} />
        <RemoteChoice label="Agent setup" detail="Check access and find desktop setup" onPress={() => { setPicker(null); router.push("/agents"); }} />
        {settings ? <SettingsNote>{`Permissions: ${settings.permissionMode} · ${state.baseBranch || settings.workspace === "worktree" ? "Worktree" : "Project folder"}`}</SettingsNote> : null}
      </> : picker === "project" ? <>
        {state.projects.length ? <PickerSearch label="Search projects" value={pickerQuery} onChangeText={setPickerQuery} /> : null}
        {shownProjects.map((p) => <RemoteChoice key={p.id} label={p.name} detail={p.locationHint} selected={!prefilling && project?.id === p.id} disabled={busy} onPress={() => { session.selectProject(p.id); setPrefilling(false); setPicker(null); }} />)}
        {!state.projects.length ? <SettingsNote>Save a project in Nexus on this computer first.</SettingsNote>
          : !shownProjects.length ? <SettingsNote>No projects match.</SettingsNote> : null}
      </> : picker === "branch" ? <>
        <SettingsNote>{awake ? "A selected branch starts in a new worktree. Any edits in the current checkout stay there." : unreachable}</SettingsNote>
        <RemoteChoice label="Use project default" selected={!state.baseBranch} disabled={busy || !awake} onPress={() => { session.selectBaseBranch(); setPicker("summary"); }} />
        {branches?.length ? <PickerSearch label="Search branches" value={pickerQuery} onChangeText={setPickerQuery} /> : null}
        {shownBranches?.map((branch) => <RemoteChoice key={branch} label={branch} selected={state.baseBranch === branch}
          disabled={busy || !awake || !!state.error} onPress={() => { session.selectBaseBranch(branch); setPicker("summary"); }} />)}
        {busy ? <ActivityIndicator color={theme.muted} style={styles.branchLoading} accessibilityLabel="Reading branches" /> : null}
        {!busy && state.error ? <SettingsNote>{state.error}</SettingsNote> : null}
        {!busy && shownBranches?.length === 0 ? <SettingsNote>{branches?.length ? "No branches match." : "This project has no branches yet."}</SettingsNote> : null}
        <SettingsAction label={pending?.kind === "describeBranches" ? "Check request" : "Refresh branches"} disabled={!awake || state.preparing}
          onPress={() => { haptic("tap"); void (pending?.kind === "describeBranches" ? session.retry() : session.describeBranches()); }} />
      </> : !supports ? <SettingsNote>Update Nexus on this computer to choose models and payment routes. New sessions currently use its saved project defaults.</SettingsNote>
        : unknownAgent ? <SettingsNote>This project’s agent uses the computer’s settings.</SettingsNote>
        : picker === "payer" ? <RemoteRoutes routes={routes} selected={selection} disabled={busy || !awake} onPick={(chosen) => { session.select({ via: chosen.via, payer: chosen.payer, model: chosen.defaultModel, effort: null }); setPicker(null); }} />
          : !selection || !route ? <><SettingsNote>Choose who pays to see the models available for this project’s agent.</SettingsNote><SettingsAction label="Choose who pays" disabled={busy} onPress={() => setPicker("payer")} /></>
            : picker === "model" ? <RemoteModels models={route.models} selected={selection.model} disabled={busy || !awake || route.state !== "usable"} onPick={(model) => { session.select({ ...selection, model, effort: route.models.find((m) => m.id === model)?.efforts.some((effort) => effort === selection.effort) ? selection.effort : null }); setPicker(null); }} />
              : picker === "effort" ? <RemoteEfforts efforts={selectedModel?.efforts ?? []} selected={kept ? undefined : selection.effort} disabled={busy || !awake} onPick={(effort) => { session.select({ ...selection, effort }, true); setPicker("summary"); }} /> : null}
      {picker !== "summary" ? <SettingsAction label="All session settings" onPress={() => setPicker("summary")} /> : null}
      <SettingsAction label="Refresh from computer" disabled={busy || !awake} onPress={() => void session.describe()} />
      </ScrollView>
    </Sheet>
  </ScreenKeyboardAvoidingView>;
}
function PickerSearch({ label, value, onChangeText }: { label: string; value: string; onChangeText: (text: string) => void }) {
  const theme = useTheme();
  return (
    <View style={[styles.pickerSearch, { backgroundColor: theme.fill }]}>
      <Icon name="search" size={16} color={theme.muted} />
      <TextInput value={value} onChangeText={onChangeText} autoCorrect={false} autoCapitalize="none" accessibilityLabel={label}
        placeholder={label} placeholderTextColor={theme.muted} style={[styles.pickerInput, { color: theme.foreground }]} />
    </View>
  );
}

const styles = StyleSheet.create({
  pickerSearch: { flexDirection: "row", alignItems: "center", gap: 8, minHeight: 44, marginHorizontal: 16, marginVertical: 6, paddingHorizontal: 12, borderRadius: 14 },
  pickerInput: { ...text.base, fontFamily: font.regular, flex: 1, minHeight: 44 },
  branchLoading: { paddingVertical: 12 },
  screen: { flex: 1 }, body: { flexGrow: 1, justifyContent: "center", paddingVertical: 32 }, title: { ...text.xl, fontFamily: font.display, textAlign: "center", marginHorizontal: 32 },
  mark: { alignSelf: "center", marginBottom: 18 },
  description: { ...text.body, fontFamily: font.regular, textAlign: "center", marginVertical: 12 }, heading: { ...text.body, fontFamily: font.medium }, note: { ...text.secondary, fontFamily: font.regular, padding: 20, textAlign: "center" },
  toolbar: composerControls.toolbar,
  label: { ...text.secondary, fontFamily: font.medium, flexShrink: 1 },
  sourceRow: { flexDirection: "row", alignItems: "center", paddingRight: 8 }, destination: { flex: 1, minWidth: 0 }, choices: composerControls.choices, status: { minHeight: 24, justifyContent: "center" }, notice: { ...text.secondary, fontFamily: font.regular, paddingHorizontal: 24, paddingVertical: 8 }, actions: { flexDirection: "row", paddingHorizontal: 16 }, action: { minHeight: 44, paddingHorizontal: 12, justifyContent: "center" },
});
