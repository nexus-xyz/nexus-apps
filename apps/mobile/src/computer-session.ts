import { CoreError, type CoreClient, type E2eDevice } from "@nexus/kit";
import { deviceTrustMode, type DeviceTrustMode } from "@nexus/kit/device-trust";
import type { DeviceKey } from "@nexus/kit/e2e";
import type { ComputerAgent, ComputerAgentChoices, ComputerBranchList, ComputerEffort, ComputerHandoffChoices, ComputerOperation, ComputerPresence, ComputerProjectChoice, ComputerRequest, ComputerRequestEnvelope, ComputerScope, ComputerSelection, ComputerSessionChanges, ComputerSessionSettings, ExecutionOrigin } from "@nexus/kit/computer-types";
import { isComputerEffort, openComputerReply, sealComputerRequest, verifyComputerPresence } from "@nexus/kit/computer-sessions";
import { thisDevice } from "@nexus/kit/pairing";
import { followComputerEvents } from "@nexus/kit/synced-follow";
import { connectionSignal, RECONNECT_MS } from "./reconnect";

export interface ComputerSaved {
  draft: string;
  projectId?: string;
  branches?: ComputerBranchList;
  baseBranch?: string;
  selection?: ComputerSelection;
  edit?: { model: string | null; effort: ComputerEffort | null };
  editDirty?: boolean;
  pending?: { envelope: ComputerRequestEnvelope; kind: ComputerOperation["kind"] };
  created?: { threadId: string; origin: ExecutionOrigin };
}
export interface ComputerJournal {
  load(): Promise<ComputerSaved | undefined>;
  save(value: ComputerSaved): Promise<void>;
  remove(): Promise<void>;
}
/** How long a session started from the phone may wait for its first synced key before the composer explains why. */
export const CREATED_WAIT_MS = 30_000;
/** A paired computer as the composer names it: `off` is awake with Remote control turned off. */
export type ComputerReach = "awake" | "off" | "offline";
export interface ComputerState {
  draft: string;
  computers: ComputerPresence[];
  projects: ComputerProjectChoice[];
  branches?: ComputerBranchList;
  baseBranch?: string;
  handoff?: ComputerHandoffChoices;
  agents: ComputerAgentChoices[];
  selection?: ComputerSelection;
  settings?: ComputerSessionSettings;
  changes?: ComputerSessionChanges;
  edit?: { model: string | null; effort: ComputerEffort | null };
  editDirty?: boolean;
  /** The person chose the edit's level in the effort picker. Not saved: after a restart, a level set on the computer is kept until chosen again. */
  effortPicked?: boolean;
  /** The same for the new session's selection. */
  selectionEffortPicked?: boolean;
  projectId?: string;
  pending?: { envelope: ComputerRequestEnvelope; kind: ComputerOperation["kind"]; state: "sending" | "queued" | "claimed" | "unknown" | "uncertain" };
  error?: string;
  notice?: string;
  /** `stalled`: no synced key after {@link CREATED_WAIT_MS}; the computer may not sync this session. */
  created?: { threadId: string; origin: ExecutionOrigin; ready: boolean; stalled?: boolean };
  loaded: boolean;
  /** The saved draft did not open. The composer stays locked until the person discards it. */
  unreadable?: boolean;
  preparing: boolean;
}

/** Reconnection refreshes descriptors and receipts without replaying actions or replacing uncertain request IDs. */
export class ComputerSession {
  private state: ComputerState = { draft: "", computers: [], projects: [], agents: [], loaded: false, preparing: false };
  private listeners = new Set<() => void>();
  private active = false;
  private timer?: ReturnType<typeof setTimeout>;
  private checking = false;
  private poll?: AbortController;
  /** The live follow of request states, while a request waits. */
  private live?: AbortController;
  private streaming = false;
  private reading = false;
  /** An announcement came during a receipt read: that read may hold the older state, so read once more after it. */
  private readAgain = false;
  private submitting = false;
  private directory: E2eDevice[] = [];
  /** The mode `directory` came with (`deviceTrustMode`). */
  private trust: DeviceTrustMode = "approvals";
  private serverTime = 0;
  private checkedAt = 0;
  private generation = 0;
  private ackUntil = 0;
  private ackReads = 0;
  private observedModel?: string | null;
  private refreshModel = false;
  private modelReadAfter = 0;
  private needsDescription = true;
  private createdSince = 0;
  constructor(private readonly options: { core: CoreClient; scope: ComputerScope; computerId: string; threadId?: string; sender: { deviceId: string; holder: DeviceKey }; journal: ComputerJournal }) {}
  get computerId() { return this.options.computerId; }
  snapshot = (): ComputerState => this.state;
  subscribe = (fn: () => void) => { this.listeners.add(fn); return () => { this.listeners.delete(fn); }; };
  private update(change: Partial<ComputerState>) { this.state = { ...this.state, ...change }; for (const fn of this.listeners) fn(); }
  private persist() { return this.options.journal.save({ draft: this.state.draft, projectId: this.state.projectId, branches: this.state.branches, baseBranch: this.state.baseBranch, selection: this.state.selection, edit: this.state.edit, editDirty: this.state.editDirty, ...(this.state.pending ? { pending: { envelope: this.state.pending.envelope, kind: this.state.pending.kind } } : {}), ...(this.state.created ? { created: { threadId: this.state.created.threadId, origin: this.state.created.origin } } : {}) }); }
  async restore() {
    try {
      const saved = await this.options.journal.load();
      const request = saved?.pending?.envelope;
      if (request && (request.computerId !== this.options.computerId || request.senderDeviceId !== this.options.sender.deviceId || (request.scope.gateway !== this.options.scope.gateway || request.scope.tenantId !== this.options.scope.tenantId || request.scope.personId !== this.options.scope.personId))) throw new Error("Saved request belongs to a different account or computer.");
      if (saved?.created) this.createdSince = Date.now();
      this.update({ ...(saved ? { draft: saved.draft, projectId: saved.projectId, branches: saved.branches, baseBranch: saved.baseBranch, selection: saved.selection, edit: saved.edit, editDirty: saved.editDirty ?? !!saved.edit, pending: saved.pending ? { ...saved.pending, state: "unknown" } : undefined, created: saved.created ? { ...saved.created, ready: false } : undefined } : {}), loaded: true });
    } catch (e) { this.update({ loaded: false, unreadable: true, error: `Couldn't open the saved draft: ${message(e)}` }); }
  }
  /**
   * Removes a saved draft that does not open, and unlocks the composer. A request it held may already have run, so the
   * person chooses this; it is never automatic.
   */
  async discardSaved() {
    if (!this.state.unreadable) return;
    try {
      await this.options.journal.remove();
      this.update({ loaded: true, unreadable: false, error: undefined, notice: undefined, draft: "", pending: undefined, created: undefined });
    } catch (e) { this.update({ error: `Couldn't discard the saved draft: ${message(e)}` }); }
  }
  setDraft(draft: string) { if (!this.state.loaded || this.submitting || this.state.pending?.kind === "startSession" || this.state.created) return; this.update({ draft }); void this.persist().catch((e) => this.update({ error: `Draft couldn't be saved: ${message(e)}` })); }
  selectProject(projectId: string) {
    if (this.state.pending || this.submitting || this.state.created) return;
    const project = this.state.projects.find((p) => p.id === projectId);
    const changed = projectId !== this.state.projectId;
    this.update({ projectId, ...(changed ? { branches: undefined, baseBranch: undefined } : {}),
      selection: project ? projectSelection(project, this.state.agents) : undefined, selectionEffortPicked: false, error: undefined, notice: undefined });
    this.saveDraft();
  }
  selectBaseBranch(name?: string) {
    if (this.state.pending || this.submitting || this.state.created) return;
    const project = this.state.projects.find((entry) => entry.id === this.state.projectId);
    if (name && !availableBranches(this.state, project)?.includes(name)) return;
    this.update({ baseBranch: name, error: undefined, notice: undefined });
    this.saveDraft();
  }
  async describeBranches() {
    const project = this.state.projects.find((entry) => entry.id === this.state.projectId);
    if (!project || this.options.threadId) return;
    await this.submit({ kind: "describeBranches", projectId: project.id, revision: project.revision });
  }
  select(selection: ComputerSelection, pickedEffort = false) {
    if (this.state.pending || this.submitting || this.state.created) return;
    this.update({ selection, selectionEffortPicked: stillPicked(this.state.selection?.effort, selection.effort, this.state.selectionEffortPicked, pickedEffort), error: undefined, notice: undefined });
    this.saveDraft();
  }
  edit(settings: { model: string | null; effort: ComputerEffort | null }, pickedEffort = false) {
    if (this.state.pending || this.submitting) return;
    this.update({ edit: settings, editDirty: true, effortPicked: stillPicked(this.state.edit?.effort, settings.effort, this.state.effortPicked, pickedEffort), error: undefined, notice: undefined });
    this.saveDraft();
  }
  /** Synced metadata only invalidates a descriptor; the execution computer still supplies every choice and label. */
  observeModel(model: string | null | undefined) {
    if (model === undefined || !this.options.threadId || model === this.observedModel) return;
    this.observedModel = model;
    this.refreshModel = !!(this.state.settings || this.submitting || this.state.pending) && model !== this.state.settings?.model;
    if (this.refreshModel) this.schedule();
  }
  private saveDraft() { void this.persist().catch((e) => this.update({ error: `Choices couldn't be saved: ${message(e)}` })); }
  now() { return this.serverTime ? this.serverTime + (Date.now() - this.checkedAt) : Date.now(); }
  awake(computer: ComputerPresence) { return this.reach(computer) === "awake"; }
  /** Offline (no fresh presence) and Remote control off are different fixes, so the composer names them apart. */
  reach(computer: ComputerPresence): ComputerReach { return computer.expiresAt <= this.now() ? "offline" : computer.enabled ? "awake" : "off"; }
  setActive(active: boolean) {
    this.active = active;
    clearTimeout(this.timer);
    if (!active) this.poll?.abort();
    this.follow();
    if (active) void this.check();
  }
  /**
   * While a request waits, follow your requests' states live (`GET /v1/computers/stream`) and read the
   * receipt the moment it changes. A slow poll stays as the backstop; without a live stream the 2 s poll goes on.
   */
  private follow() {
    const wanted = this.active && !!this.state.pending && this.state.pending.state !== "uncertain";
    if (wanted === !!this.live) return;
    if (!wanted) {
      this.live?.abort();
      this.live = undefined;
      this.streaming = false;
      return;
    }
    const controller = new AbortController();
    this.live = controller;
    const { core, computerId } = this.options;
    void followComputerEvents({
      stream: (on, signal, onOpen) => core.streamComputerEvents(on, signal, onOpen),
      onRequest: (event) => {
        if (event.computerId === computerId && event.id === this.state.pending?.envelope.id) void this.readReceipt();
      },
      onState: (state) => {
        if (controller.signal.aborted) return;
        this.streaming = state === "live";
        // An event sent while the stream was down is lost: read the receipt again once it is back.
        if (this.streaming) void this.readReceipt();
        this.schedule();
      },
      signal: controller.signal,
    });
  }
  /** Reads the pending request's receipt now, one read at a time. A call during a read asks for one more read after it. */
  private async readReceipt() {
    const pending = this.state.pending;
    if (!this.active || !pending || pending.state === "uncertain") return;
    if (this.reading) { this.readAgain = true; return; }
    this.reading = true;
    try { await this.show(await this.options.core.computerRequest(this.options.computerId, pending.envelope.id)); }
    catch { /* The poll reads it again and reports a failure. */ }
    finally {
      this.reading = false;
      this.schedule();
      if (this.readAgain) { this.readAgain = false; void this.readReceipt(); }
    }
  }
  private schedule() {
    clearTimeout(this.timer);
    this.follow();
    if (!this.active) return;
    let delay = 15_000;
    const computer = this.state.computers.find((each) => each.computerId === this.options.computerId);
    if (!computer || !this.awake(computer) || this.state.error) delay = RECONNECT_MS;
    // A stalled start keeps checking at the normal pace: it opens on its own if the history arrives later.
    if (this.state.pending) delay = this.streaming ? 15_000 : 2000;
    else if (this.state.created && !this.state.created.ready && !this.state.created.stalled) delay = 2000;
    else if (this.state.settings?.pending && this.ackReads < 5 && Date.now() < this.ackUntil) delay = 5000;
    else if (this.refreshModel) delay = Math.max(2000, this.modelReadAfter - Date.now());
    this.timer = setTimeout(() => void this.check(), delay);
  }
  async refresh(signal?: AbortSignal) {
    const { core, scope, sender } = this.options;
    const readSignal = connectionSignal(signal);
    const [listed, directory] = await Promise.all([core.computers(readSignal), core.e2eDevices(readSignal)]);
    const self = thisDevice(directory.devices, sender.holder);
    if (!self || self.id !== sender.deviceId || self.state !== "trusted") throw new Error("This phone must be paired with a trusted computer first.");
    // The mode of the list just read: with `gateway`, a computer Gateway lists as trusted is trusted.
    const trust = deviceTrustMode(directory);
    const computers: ComputerPresence[] = [];
    for (const presence of listed.computers) {
      try { computers.push(await verifyComputerPresence({ presence, scope, reader: { ...sender, trust }, devices: directory.devices })); }
      catch { /* An unverified computer is never a destination. */ }
    }
    if (signal?.aborted) return;
    this.directory = directory.devices;
    this.trust = trust;
    this.serverTime = listed.serverTime;
    this.checkedAt = Date.now();
    this.update({ computers });
    const computer = computers.find((each) => each.computerId === this.options.computerId);
    if (!computer || !this.awake(computer)) this.needsDescription = true;
  }
  private async check() {
    if (this.checking) return;
    this.checking = true;
    const controller = new AbortController();
    this.poll = controller;
    const generation = this.generation;
    try {
      await this.refresh(controller.signal);
      if (controller.signal.aborted || !this.active) return;
      if (this.state.created && !this.state.created.ready) {
        const created = this.state.created;
        try {
          const thread = await this.options.core.syncedThread(created.threadId);
          if (controller.signal.aborted || !this.active) return;
          if (Number.isSafeInteger(thread.lastSeq) && Number.isSafeInteger(thread.keyVersion) && thread.keyVersion !== null && thread.keyVersion > 0) this.update({ created: { ...created, ready: true }, notice: "Opening session…", error: undefined });
          else this.waitForHistory(created);
        } catch (e) {
          if (!(e instanceof CoreError && e.status === 404)) throw e;
          if (!controller.signal.aborted) this.waitForHistory(created);
        }
      }
      const pending = this.state.pending;
      if (pending && pending.state !== "uncertain") {
        try { await this.show(await this.options.core.computerRequest(this.options.computerId, pending.envelope.id, controller.signal)); }
        catch (e) {
          if (e instanceof CoreError && e.status === 404) this.update({ pending: { ...pending, state: "unknown" }, notice: "Delivery is unconfirmed. The action may already have run. Check again or retry the same request." });
          else throw e;
        }
      }
      if (controller.signal.aborted || !this.active) return;
      const computer = this.state.computers.find((each) => each.computerId === this.options.computerId);
      if (this.needsDescription && computer && this.awake(computer) && !this.state.pending && !this.submitting && !this.state.created && (!this.options.threadId || computer.capabilities.includes("settings"))) {
        await this.describe();
      } else if (this.refreshModel && !this.state.pending && !this.submitting && Date.now() >= this.modelReadAfter) {
        this.refreshModel = false;
        this.modelReadAfter = Date.now() + 5000;
        await this.describe();
      } else if (!this.state.pending && this.state.settings?.pending && this.ackReads < 5 && Date.now() < this.ackUntil) {
        this.ackReads++;
        await this.describe();
      }
      if (generation === this.generation) this.update({ error: undefined });
    } catch (e) { if (!controller.signal.aborted && generation === this.generation) { this.needsDescription = true; this.update({ error: message(e) }); } }
    finally { this.checking = false; this.schedule(); }
  }
  /** Keeps waiting for the started session's history; past {@link CREATED_WAIT_MS} the composer explains and offers a way on. */
  private waitForHistory(created: NonNullable<ComputerState["created"]>) {
    if (created.stalled || Date.now() - this.createdSince < CREATED_WAIT_MS) {
      if (!created.stalled) this.update({ notice: "Session started on the computer. Waiting for its encrypted history…" });
      return;
    }
    this.update({ created: { ...created, stalled: true }, notice: "The computer hasn't synced this session yet. Sync may be off on that computer, or your organization keeps session content on the computer." });
  }
  /** After a stalled start: the composer unlocks for a new session. The started session stays on the computer. */
  startAnother() {
    if (!this.state.created?.stalled) return;
    this.update({ created: undefined, draft: "", notice: undefined, error: undefined });
    void this.persist().catch((e) => this.update({ error: message(e) }));
  }
  /** After a stalled start: opens the started session as it is. Returns its thread id. */
  openAnyway(): string | undefined {
    const created = this.state.created;
    if (!created?.stalled) return;
    this.update({ created: undefined, draft: "", notice: undefined, error: undefined });
    void this.persist().catch((e) => this.update({ error: message(e) }));
    return created.threadId;
  }
  async describe() { if (this.state.pending || this.submitting) return; this.refreshModel = false; await this.submit(this.options.threadId ? { kind: "describeSession", threadId: this.options.threadId } : { kind: "describeProjects" }); }
  async readChanges() {
    if (this.options.threadId) await this.submit({ kind: "readSessionChanges", threadId: this.options.threadId });
  }
  async describeHandoff() {
    if (!this.options.threadId || this.state.pending || this.state.created || this.submitting) return;
    this.update({ handoff: undefined });
    await this.submit({ kind: "describeHandoff", threadId: this.options.threadId });
  }
  async startHandoff(text: string, agent: ComputerAgent, selection: ComputerSelection) {
    const options = this.state.handoff;
    if (!options || options.threadId !== this.options.threadId || !text.trim()) return;
    const route = options.agents.find((entry) => entry.agent === agent)?.routes.find((entry) =>
      entry.state === "usable" && entry.via === selection.via && entry.payer === selection.payer);
    if (!route || selection.model !== route.defaultModel || selection.effort !== null) return;
    await this.submit({ kind: "startSession", projectId: options.project.id, revision: options.project.revision, text: text.trim(),
      selection, handoff: { sourceThreadId: options.threadId, agent } });
  }
  async compact() {
    const { settings } = this.state;
    if (!this.options.threadId || !settings || (settings.agent !== "claude" && settings.agent !== "codex") || settings.pending) return;
    await this.submit({ kind: "compactSession", threadId: this.options.threadId });
  }
  async apply() {
    const { settings, edit } = this.state;
    if (!this.options.threadId || !settings?.editable || settings.pending || !edit || !validSettings(settings.models, edit) || replacesComputerEffort(settings, edit, this.state.effortPicked)) return;
    await this.submit({ kind: "updateSession", threadId: this.options.threadId, revision: settings.revision, settings: edit });
  }
  async start() {
    const project = this.state.projects.find((p) => p.id === this.state.projectId);
    if (!project || !this.state.draft.trim() || this.options.threadId) return;
    if (this.state.selection && replacesProjectEffort(project, this.state.selection, this.state.selectionEffortPicked)) return;
    if (this.state.selection && !validSelection(project, this.state.agents, this.state.selection)) {
      this.update({ error: "These choices changed. Choose an available payment route and model before starting." }); return;
    }
    const { baseBranch } = this.state;
    if (baseBranch && !availableBranches(this.state, project)?.includes(baseBranch)) {
      this.update({ error: "Refresh branches on this computer before starting." });
      return;
    }
    await this.submit({ kind: "startSession", projectId: project.id, revision: project.revision, text: this.state.draft.trim(),
      ...(this.state.selection ? { selection: this.state.selection } : {}), ...(baseBranch ? { baseBranch } : {}) });
  }
  private async submit(operation: Parameters<typeof sealComputerRequest>[0]["operation"]) {
    if (this.state.pending || this.state.created || this.submitting || !this.state.loaded) return;
    this.submitting = true;
    // Lock synchronously before the first await, including presence and directory reads.
    const generation = ++this.generation;
    this.update({ notice: "Checking computer…", error: undefined, preparing: true });
    try {
      await this.refresh();
      const computer = this.state.computers.find((c) => c.computerId === this.options.computerId);
      const reach = computer ? this.reach(computer) : "offline";
      if (reach === "off") throw new Error(`Turn on Remote control on ${computer?.name ?? "this computer"}.`);
      if (!computer || reach !== "awake") throw new Error("Waiting for the computer. Reconnects automatically when Nexus is available.");
      if ((operation.kind === "describeSession" || operation.kind === "describeHandoff" || operation.kind === "compactSession" || operation.kind === "updateSession" || operation.kind === "startSession" && operation.selection) && !computer.capabilities.includes("settings")) throw new Error("Update Nexus on this computer to choose models and payment routes.");
      const now = this.now();
      const envelope = await sealComputerRequest({ scope: this.options.scope, computer, operation, sender: { ...this.options.sender, trust: this.trust }, devices: this.directory, now, expiresAt: now + (operation.kind === "startSession" ? 15_000 : 30_000) });
      if (generation !== this.generation) return;
      this.update({ pending: { envelope, kind: operation.kind, state: "sending" }, notice: undefined });
      // Storage must succeed before any POST, so a lost acknowledgement never creates a second id after restart.
      await this.sendSame();
    } catch (e) { this.update({ error: message(e), notice: undefined }); }
    finally { this.submitting = false; this.update({ preparing: false }); }
  }
  async retry() {
    if (this.state.created) { await this.check(); return; }
    if (!this.state.pending || this.state.pending.state === "uncertain") return;
    // A previous POST may have arrived even when the phone saw an error. Query before repeating its exact envelope.
    try { await this.show(await this.options.core.computerRequest(this.options.computerId, this.state.pending.envelope.id)); }
    catch (e) { if (e instanceof CoreError && e.status === 404) await this.sendSame(); else this.update({ error: message(e) }); }
  }
  private async sendSame() {
    const pending = this.state.pending;
    if (!pending) return;
    try {
      await this.persist();
      await this.show(await this.options.core.submitComputerRequest(this.options.computerId, pending.envelope));
    }
    catch (e) {
      this.update({ pending: { ...pending, state: "unknown" }, error: message(e), notice: "Delivery is unconfirmed. The action may already have run. Retry checks this same request first." });
      // Even a retry's 4xx cannot prove what happened to its earlier uncertain delivery.
      try { await this.show(await this.options.core.computerRequest(this.options.computerId, pending.envelope.id)); } catch { /* Keep the original request tracked. */ }
    }
    this.schedule();
  }
  private async show(receipt: ComputerRequest) {
    const pending = this.state.pending;
    if (!pending || receipt.envelope.id !== pending.envelope.id) return;
    if (receipt.state === "expired") {
      // Gateway also expires a claimed request after its deadline, so expiry does not prove the request never ran.
      if (pending.kind === "describeBranches" || pending.kind === "describeHandoff") {
        const what = pending.kind === "describeBranches" ? "branches" : "available agents";
        this.update({ pending: undefined, notice: undefined, error: `Couldn’t read ${what}. Try again or update Nexus on this computer.` });
      } else {
        this.update({ pending: undefined, notice: pending.kind === "readSessionChanges" ? "The computer didn’t send its recorded changes in time. Try again." : "The computer did not confirm this request in time. It may have run. Check your history before starting again. Your draft is kept." });
      }
      await this.persist(); return;
    }
    if (receipt.state !== "replied" || !receipt.reply) {
      this.update({ pending: { ...pending, state: receipt.state === "claimed" ? "claimed" : "queued" }, notice: receipt.state === "claimed" ? "The computer is working on this request…" : "Waiting for the computer…" });
      return;
    }
    const result = await openComputerReply({ request: pending.envelope, reply: receipt.reply, holder: this.options.sender.holder, devices: this.directory, trust: this.trust });
    if (this.state.pending !== pending) return;
    if (pending.kind === "readSessionChanges" && result.kind !== "sessionChanges" && result.kind !== "refused" && result.kind !== "uncertain") {
      throw new Error("The computer did not reply with recorded changes.");
    }
    switch (result.kind) {
      case "projects":
      case "choicesChanged": {
        this.needsDescription = false;
        if (result.kind === "choicesChanged" && this.options.threadId && this.state.handoff) {
          this.update({ pending: undefined, handoff: undefined, error: "The project or payment choices changed. Load available agents again.", notice: undefined });
          break;
        }
        const selected = this.state.projectId ? result.projects.find((p) => p.id === this.state.projectId) : result.projects[0];
        const agents = result.agents ?? [];
        // A level set on the computer since the last reply is kept: an earlier selection would replace it.
        const dropped = !!selected && projectEffortUnknown(selected);
        this.update({ pending: undefined, projects: result.projects, agents, projectId: this.state.projectId ?? selected?.id, selection: dropped ? undefined : this.state.selection ?? (selected ? projectSelection(selected, agents) : undefined), ...(dropped ? { selectionEffortPicked: false } : {}), notice: result.kind === "choicesChanged" ? "Choices changed on the computer. Review your selection before starting again." : result.projects.length ? undefined : "No saved projects. Add a project on this computer." });
        break;
      }
      case "branches": {
        if (pending.kind !== "describeBranches" || this.state.projectId !== result.projectId) throw new Error("The computer replied for a different project.");
        const { projectId, revision, branches } = result;
        this.update({ pending: undefined, branches: { projectId, revision, branches }, notice: undefined, error: undefined });
        break;
      }
      case "handoffChoices":
        if (pending.kind !== "describeHandoff" || result.threadId !== this.options.threadId) throw new Error("The computer replied for a different session.");
        this.update({ pending: undefined, handoff: result, error: undefined, notice: undefined });
        break;
      case "sessionSettings":
      case "sessionUpdated":
      case "sessionChanged": {
        this.needsDescription = false;
        if (result.session.threadId !== this.options.threadId) throw new Error("The computer replied for a different session.");
        const session = result.session;
        const accepted = result.kind === "sessionUpdated";
        if (session.pending && (accepted || !this.state.settings?.pending)) { this.ackUntil = Date.now() + 35_000; this.ackReads = 0; }
        if (!session.pending) this.ackUntil = 0;
        const replaced = accepted || !this.state.editDirty;
        this.update({ pending: undefined, settings: session, edit: replaced ? requestedSettings(session) : this.state.edit ?? requestedSettings(session), editDirty: accepted ? false : this.state.editDirty ?? false, effortPicked: replaced ? false : this.state.effortPicked, notice: result.kind === "sessionChanged" ? "Settings changed on the computer. Your selection is kept; review it before applying." : session.pending ? "Saved on the computer. Waiting for the agent to apply the change…" : accepted ? "Settings applied." : undefined });
        break;
      }
      case "sessionChanges": {
        if (pending.kind !== "readSessionChanges" || result.threadId !== this.options.threadId) throw new Error("The computer replied for a different session.");
        const { threadId, files, patch, truncated } = result;
        this.update({ pending: undefined, changes: { threadId, files, patch, truncated }, notice: undefined, error: undefined });
        break;
      }
      case "compacted":
        if (pending.kind !== "compactSession" || result.threadId !== this.options.threadId) throw new Error("The computer replied for a different session.");
        this.update({ pending: undefined, error: undefined, notice: "The computer accepted compaction." });
        break;
      case "created": this.createdSince = Date.now(); this.update({ pending: undefined, created: { threadId: result.threadId, origin: result.origin, ready: false }, notice: "Session started on the computer. Waiting for its encrypted history…" }); break;
      case "refused":
        this.needsDescription = false;
        this.update({
          pending: undefined, error: result.message, notice: undefined,
          ...(result.code === "branch_changed" ? { branches: undefined } : {}),
        });
        break;
      case "uncertain":
        if (pending.kind === "describeBranches" || pending.kind === "describeHandoff") {
          const what = pending.kind === "describeBranches" ? "branches" : "available agents";
          this.update({ pending: undefined, notice: undefined, error: `Couldn’t read ${what}. Try again.` });
        } else {
          this.update({ pending: { ...pending, state: "uncertain" }, notice: "The computer could not confirm this request. It may already have run. Refresh the session before trying again." });
        }
        break;
    }
    await this.persist();
  }
  opened() {
    if (!this.state.created?.ready) return;
    this.update({ created: undefined, draft: "" });
    void this.persist().catch((e) => this.update({ error: message(e) }));
  }
  dismiss() {
    const pending = this.state.pending;
    if (!pending || (pending.state !== "uncertain" && this.now() < pending.envelope.expiresAt)) return;
    this.update({ pending: undefined, notice: pending.kind === "readSessionChanges" ? "Stopped checking for recorded changes." : "Stopped checking. The action may already have run. Check your history before starting again." });
    void this.persist().catch((e) => this.update({ error: message(e) }));
  }
}
export function availableBranches(state: Pick<ComputerState, "branches">, project?: ComputerProjectChoice): string[] | undefined {
  const list = state.branches;
  return list && project && list.projectId === project.id && list.revision === project.revision ? list.branches : undefined;
}

function message(e: unknown) { if (e instanceof CoreError && e.status === 404) return "Update Nexus Gateway to connect computers from this phone."; return e instanceof Error ? e.message : String(e); }

const owners = new WeakMap<CoreClient, Map<string, Promise<ComputerSession>>>();
/** A route releases only its foreground lease; the same account and phone reacquire the unresolved request. */
export function computerSession(options: ConstructorParameters<typeof ComputerSession>[0]): Promise<ComputerSession> {
  let sessions = owners.get(options.core);
  if (!sessions) { sessions = new Map(); owners.set(options.core, sessions); }
  const key = JSON.stringify([options.scope.gateway, options.scope.tenantId, options.scope.personId, options.computerId, options.threadId, options.sender.deviceId,
    Array.from(options.sender.holder.encryptionKey), Array.from(options.sender.holder.signingKey)]);
  let session = sessions.get(key);
  if (!session) {
    const made = new ComputerSession(options);
    session = made.restore().then(() => made);
    sessions.set(key, session);
  }
  return session;
}

export function projectRoutes(project: ComputerProjectChoice | undefined, agents: readonly ComputerAgentChoices[]) { return agents.find((a) => a.agent === project?.settings.agent)?.routes ?? []; }
export function projectSelection(project: ComputerProjectChoice, agents: readonly ComputerAgentChoices[]): ComputerSelection | undefined {
  // A selection replaces the project's level, so a level this phone does not know goes out as no selection, and the computer keeps it.
  if (projectEffortUnknown(project)) return;
  const route = projectRoutes(project, agents).find((r) => r.via === project.settings.via && r.payer === project.payer?.kind && r.state === "usable");
  if (!route) return;
  const model = project.settings.model ?? route.defaultModel;
  const effort = route.models.find((m) => m.id === model)?.efforts.find((e) => e === project.settings.effort) ?? null;
  return { via: route.via, payer: route.payer, model, effort };
}
export function validSettings(models: readonly { id: string; efforts: ComputerEffort[] }[], edit: { model: string | null; effort: ComputerEffort | null }) {
  if (edit.model === null) return edit.effort === null;
  const model = models.find((m) => m.id === edit.model);
  return !!model && (edit.effort === null || model.efforts.includes(edit.effort));
}
export function validSelection(project: ComputerProjectChoice, agents: readonly ComputerAgentChoices[], selection: ComputerSelection) {
  const route = projectRoutes(project, agents).find((r) => r.via === selection.via && r.payer === selection.payer);
  return route?.state === "usable" && validSettings(route.models, selection);
}

/** An update always carries a level, and the computer stores it. Without a level the person chose, it would replace one set on the computer. */
export function replacesComputerEffort(session: ComputerSessionSettings, edit: { effort: ComputerEffort | null }, picked?: boolean) {
  return !!session.effortUnknown && edit.effort === null && !picked;
}
export function projectEffortUnknown(project: ComputerProjectChoice) {
  return project.settings.effort !== null && !isComputerEffort(project.settings.effort);
}
/** A selection replaces the project's level as `replacesComputerEffort` describes for an update. */
export function replacesProjectEffort(project: ComputerProjectChoice, selection: ComputerSelection, picked?: boolean) {
  return projectEffortUnknown(project) && selection.effort === null && !picked;
}
/** A level the person picked stays picked until a model change drops it to none. */
function stillPicked(before: ComputerEffort | null | undefined, after: ComputerEffort | null, picked: boolean | undefined, pickedNow: boolean) {
  const dropped = after === null && before !== null && before !== undefined;
  return pickedNow || (!!picked && !dropped);
}

export function requestedSettings(session: ComputerSessionSettings) {
  return { model: session.pending?.model !== undefined ? session.pending.model : session.model, effort: session.pending?.effort !== undefined ? session.pending.effort : session.effort };
}
