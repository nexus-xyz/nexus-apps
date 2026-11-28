import { ScrollView, useWindowDimensions } from "react-native";
import { TRANSCRIPT_VIEWS, type TranscriptView } from "./code-timeline";
import type { ComputerSession, ComputerState } from "./computer-session";
import { replacesComputerEffort, requestedSettings, validSettings } from "./computer-session";
import { haptic } from "./haptics";
import { SessionDetails } from "./native-session-header";
import { COMPUTER_EFFORT, RemoteChoice, RemoteEfforts, RemoteModels, SettingsAction, SettingsNote, effortLabel } from "./remote-settings-ui";
import { SessionHandoffView } from "./session-handoff-view";
import { Sheet, SheetTitle } from "./sheet";
import { SessionChanges, SessionTerminal } from "./session-workbench";
import type { TerminalCommand } from "./read-only-workbench";

export type SessionSettingsPage = "summary" | "handoff" | "model" | "effort" | "payer" | "transcript" | "changes" | "terminal";
export function SessionSettingsSheet({ page, onPage, onClose, session, state, error, title, origin, project, agent, transcriptView, onTranscriptView, workbench, terminal, running, handoff }: {
  page: SessionSettingsPage | null; onPage: (page: SessionSettingsPage) => void; onClose: () => void;
  session?: ComputerSession; state?: ComputerState; error?: string; title: string; origin: string; project?: string; agent?: string | null;
  transcriptView: TranscriptView; onTranscriptView: (view: TranscriptView) => void;
  workbench?: boolean; terminal?: readonly TerminalCommand[]; running?: boolean; handoff?: { prompt: string; onOpen: (threadId: string) => void };

}) {
  const { height } = useWindowDimensions();
  const settings = state?.settings;
  const edit = state?.edit;
  const displayed = settings?.pending ? requestedSettings(settings) : edit;
  const computer = state?.computers.find((c) => c.computerId === session?.computerId);
  const awake = !!computer && !!session?.awake(computer);
  const supports = !!computer?.capabilities.includes("settings");
  const busy = !!state?.pending || !!state?.preparing;
  const disabled = !awake || !supports || busy || !settings?.editable || !!settings.pending;
  const model = settings?.models.find((m) => m.id === displayed?.model);
  const blocked = !!settings && !!edit && replacesComputerEffort(settings, edit, state?.effortPicked);
  // Choosing Agent default over a level set on the computer is a change, though both read as null.
  const changed = !!settings && !!edit && (edit.model !== settings.model || edit.effort !== settings.effort || (!!settings.effortUnknown && !blocked));
  // The displayed level is one set or requested on the computer, which this phone cannot name.
  const kept = settings?.pending ? !!settings.pending.effortUnknown || (!!settings.effortUnknown && settings.pending.effort === undefined) : blocked;
  if (page === "changes" || page === "terminal") {
    return <Sheet visible onClose={onClose}>
      <SheetTitle>{page === "changes" ? "Changes" : "Terminal"}</SheetTitle>
      <ScrollView key={page} style={{ maxHeight: height * 0.7 }}>
        {page === "changes" ? <SessionChanges session={session} state={state} error={error} /> : <SessionTerminal commands={terminal ?? []} />}
        <SettingsAction label="Back to session settings" onPress={() => onPage("summary")} />
      </ScrollView>
    </Sheet>;
  }
  const status = error ?? state?.error ?? (busy ? state?.notice ?? "Checking settings…" : !session ? "Settings need a verified origin computer. Open this older session on its computer to update it." : !computer ? "Checking the computer…" : !supports ? "Update Nexus on this computer to change models from your phone." : !awake ? "The computer is offline. Open Nexus there to change settings." : settings?.reason ?? (edit && settings && !validSettings(settings.models, edit) ? "This model or effort is no longer available. Choose another before applying." : changed && blocked ? "This session's reasoning effort is set on the computer. Choose a reasoning effort before applying." : state?.notice));
  if (page === "transcript") return <Sheet visible onClose={onClose}><SheetTitle>Transcript view</SheetTitle>
    {TRANSCRIPT_VIEWS.map(({ id, label, detail }) => (
      <RemoteChoice key={id} label={label} detail={detail} selected={id === transcriptView} onPress={() => onTranscriptView(id)} />
    ))}
    <SettingsAction label="Back to session settings" onPress={() => onPage("summary")} />
  </Sheet>;
  return <Sheet visible={page !== null} onClose={onClose}><SheetTitle>{page === "handoff" ? "Continue in another agent" : page === "model" ? "Model" : page === "effort" ? "Reasoning effort" : page === "payer" ? "Who pays" : "Session settings"}</SheetTitle>
    <ScrollView key={page} style={{ maxHeight: height * 0.7 }} keyboardShouldPersistTaps="handled">
    {page === "summary" ? <>
      <SessionDetails title={title} origin={origin} project={project} agent={agent} />
      <RemoteChoice label="Transcript view" detail={TRANSCRIPT_VIEWS.find(({ id }) => id === transcriptView)?.label}
        onPress={() => onPage("transcript")} />
      {workbench ? <>
        <RemoteChoice label="Changes" detail="Recorded file changes" onPress={() => onPage("changes")} />
        <RemoteChoice label="Terminal" detail="Synced agent command output" onPress={() => onPage("terminal")} />
      </> : null}
      {settings && edit ? <>
        <RemoteChoice label={settings.pending ? "Requested model" : "Model"} detail={model?.name ?? displayed?.model ?? "Computer default"} disabled={disabled} onPress={() => onPage("model")} />
        <RemoteChoice label={settings.pending ? "Requested effort" : "Reasoning effort"} detail={kept ? COMPUTER_EFFORT : effortLabel(displayed?.effort ?? null)} disabled={disabled || (!model?.efforts.length && !kept)} onPress={() => onPage("effort")} />
        <RemoteChoice label="Who pays" detail={settings.payer.label} onPress={() => onPage("payer")} />
        {handoff ? <RemoteChoice label="Continue in another agent" disabled={!awake || !supports} onPress={() => {
          onPage("handoff");
          if (!state?.pending && !state?.created) void session?.describeHandoff();
        }} /> : null}
        {settings.agent === "claude" || settings.agent === "codex" ? (
          <RemoteChoice label={state?.pending?.kind === "compactSession" ? "Compacting…" : "Compact now"}
            detail="Summarize earlier turns and keep your draft" disabled={running || !awake || !supports || busy || !!settings.pending}
            onPress={() => void session?.compact()} />
        ) : null}
      </> : null}
    </> : page === "handoff" && session && state && handoff ? (
      <SessionHandoffView session={session} state={state} initialPrompt={handoff.prompt} awake={awake} onOpen={handoff.onOpen} />
    ) : page === "payer" ? <>
      <SettingsNote>{settings?.payer.label ?? "Payment route has not been reported by the computer."}{settings?.payer.detail ? `\n${settings.payer.detail}` : ""}</SettingsNote>
      <SettingsNote>This session keeps the payment route it started with. Choose a different route in a new session.</SettingsNote>
    </> : page === "model" && settings && edit ? <RemoteModels models={settings.models} selected={displayed?.model ?? null} disabled={disabled} onPick={(model) => { session?.edit({ model, effort: settings.models.find((m) => m.id === model)?.efforts.some((effort) => effort === edit.effort) ? edit.effort : null }); onPage("summary"); }} />
      : page === "effort" && edit ? <RemoteEfforts efforts={model?.efforts ?? []} selected={kept ? undefined : displayed?.effort ?? null} disabled={disabled} onPick={(effort) => { session?.edit({ ...edit, effort }, true); onPage("summary"); }} /> : null}
    {settings?.pending ? <SettingsNote>{`Effective: ${settings.models.find((m) => m.id === settings.model)?.name ?? settings.model ?? "Computer default"} · ${settings.effortUnknown ? COMPUTER_EFFORT : effortLabel(settings.effort)}\nWaiting for the agent to apply the saved change.`}</SettingsNote> : null}
    <SettingsNote danger={!!(error ?? state?.error)}>{status ?? " "}</SettingsNote>
    {page !== "summary" ? <SettingsAction label="Back to session settings" onPress={() => onPage("summary")} /> : null}
    {page !== "handoff" && changed && edit && settings && !settings.pending ? <SettingsAction label={busy ? "Saving…" : "Apply model settings"} disabled={disabled || blocked || !validSettings(settings.models, edit)} onPress={() => void session?.apply()} /> : null}
    {page !== "handoff" && session && !state?.pending ? <SettingsAction label="Refresh settings" disabled={busy || !awake || !supports} onPress={() => void session.describe()} /> : null}
    {state?.pending?.kind === "compactSession" && state.pending.state !== "uncertain" ? (
      <SettingsAction label="Check compaction" onPress={() => { haptic("tap"); void session?.retry(); }} />
    ) : null}
    {state?.pending && (state.pending.state === "uncertain" || session && session.now() >= state.pending.envelope.expiresAt) ? <SettingsAction label="Stop checking · change may already have run" onPress={() => session?.dismiss()} /> : null}
    </ScrollView>
  </Sheet>;
}
