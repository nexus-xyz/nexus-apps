import { CoreError, type CoreClient } from "@nexus/kit";
import type { ComputerPresence, ComputerScope } from "@nexus/kit/computer-types";
import { parseComputerRequestEnvelope, readComputerBranchList, readComputerSelection, readExecutionOrigin, verifyComputerPresence } from "@nexus/kit/computer-sessions";
import { thisDevice } from "@nexus/kit/pairing";
import { useFocusEffect } from "expo-router";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { AppState } from "react-native";
import { useAuth, useCore } from "./auth";
import { ensurePhoneDevice } from "./code-reader";
import { computerSession, type ComputerSession, type ComputerSaved } from "./computer-session";
import { computerJournal } from "./computer-journal";
import { failureCause, PairingError, type FailureCause } from "./connection-errors";
import { pendingApprovals, type PendingApproval } from "./pending-approvals";
import { connectionSignal, RECONNECT_MS } from "./reconnect";

/**
 * This phone as a sender to the person's computers. When Gateway vouches for the person's devices (`trust`
 * `gateway`), the phone registers here if it has not yet, and needs no scan, code or approval.
 */
export async function computerAccount(core: CoreClient, gateway: string) {
  const signal = connectionSignal();
  const [me, phone] = await Promise.all([core.me(signal), ensurePhoneDevice(core)]);
  const { list: directory, trust, holder } = phone;
  if (!me.user?.id) throw new PairingError("Sign in with your personal account to connect a computer.");
  if (trust === "gateway" && phone.self?.state === "revoked") throw new PairingError("This phone was removed from your devices. Set it up again to connect your computers.", trust);
  if (!holder) throw new PairingError("Scan Connect phone in Nexus on your computer first.");
  const device = thisDevice(directory.devices, holder);
  if (!device || device.state !== "trusted") throw new PairingError("This phone needs approval from a trusted computer.");
  const scope: ComputerScope = { gateway: gateway.replace(/\/+$/, ""), tenantId: me.tenant.id, personId: me.user.id };
  return { scope, sender: { deviceId: device.id, holder, trust }, devices: directory.devices, trust };
}
export function useComputers() {
  const core = useCore();
  const { gateway } = useAuth();
  const owner = useMemo(() => ({}), [core, gateway]);
  const current = useRef(owner);
  const reading = useRef<object | undefined>(undefined);
  current.current = owner;
  // `gateway`: Gateway vouches for the person's devices, so the computers come from presence alone.
  const [state, setState] = useState<{ owner: object; computers: ComputerPresence[]; pending: PendingApproval[]; serverTime: number; at: number; error?: string; needsPairing?: boolean; gateway?: boolean }>({ owner, computers: [], pending: [], serverTime: 0, at: 0 });
  const refresh = useCallback(async () => {
    if (!gateway || reading.current === owner) return;
    reading.current = owner;
    try {
      const account = await computerAccount(core, gateway);
      const trusts = account.trust === "gateway";
      // The computers waiting for this phone's approval come from the same directory read.
      const pending = await pendingApprovals(account.devices, account.sender.holder, Date.now(), account.trust);
      if (current.current === owner) setState((s) => ({ ...(s.owner === owner ? s : { owner, computers: [], serverTime: 0, at: 0 }), pending, gateway: trusts }));
      const list = await core.computers();
      const verified = await Promise.all(list.computers.map(async (presence) => {
        try { return await verifyComputerPresence({ presence, scope: account.scope, reader: account.sender, devices: account.devices }); } catch { return undefined; }
      }));
      if (current.current !== owner) return;
      setState({ owner, computers: verified.filter((c): c is ComputerPresence => !!c), pending, serverTime: list.serverTime, at: Date.now(), gateway: trusts });
    } catch (e) { if (current.current !== owner) return; setState((s) => ({ ...(s.owner === owner ? s : { owner, computers: [], pending: [], serverTime: 0, at: 0 }), ...(e instanceof PairingError ? { pending: [], gateway: e.trust === "gateway" } : {}), needsPairing: e instanceof PairingError, error: e instanceof CoreError && e.status === 404 ? "Update Nexus Gateway to connect computers from this phone." : e instanceof Error ? e.message : String(e) })); }
    finally { if (reading.current === owner) reading.current = undefined; }
  }, [core, gateway, owner]);
  useFocusEffect(useCallback(() => {
    void refresh();
    let timer = setInterval(() => { if (AppState.currentState === "active") void refresh(); }, 15_000);
    const subscription = AppState.addEventListener("change", (s) => { if (s === "active") void refresh(); });
    return () => { clearInterval(timer); subscription.remove(); };
  }, [refresh]));
  const visible = state.owner === owner ? state : { owner, computers: [], pending: [], serverTime: 0, at: 0, error: undefined, needsPairing: false, gateway: undefined };
  return { ...visible, refresh, awake: (computer: ComputerPresence) => computer.enabled && computer.expiresAt > visible.serverTime + (Date.now() - visible.at) };
}
function parseSaved(value: unknown): ComputerSaved | undefined {
  if (!value || typeof value !== "object" || !("draft" in value) || typeof value.draft !== "string") return;
  const saved: ComputerSaved = { draft: value.draft };
  if ("projectId" in value && value.projectId !== undefined) { if (typeof value.projectId !== "string") return; saved.projectId = value.projectId; }
  if ("branches" in value && value.branches !== undefined) saved.branches = readComputerBranchList(value.branches);
  if ("baseBranch" in value && value.baseBranch !== undefined) {
    if (typeof value.baseBranch !== "string" || !value.baseBranch || value.baseBranch.length > 256 || value.baseBranch.startsWith("-") || /\s/.test(value.baseBranch)) return;
    saved.baseBranch = value.baseBranch;
  }
  if ("selection" in value && value.selection !== undefined) saved.selection = readComputerSelection(value.selection);
  if ("edit" in value && value.edit !== undefined) {
    if (!value.edit || typeof value.edit !== "object") return;
    const parsed = readComputerSelection({ ...value.edit, via: "nexus", payer: "gateway" });
    saved.edit = { model: parsed.model, effort: parsed.effort };
  }
  if ("editDirty" in value && value.editDirty !== undefined) { if (typeof value.editDirty !== "boolean") return; saved.editDirty = value.editDirty; }
  let created: ComputerSaved["created"];
  if ("created" in value && value.created !== undefined) {
    const held = value.created;
    if (!held || typeof held !== "object" || !("threadId" in held) || typeof held.threadId !== "string" || !/^[0-9a-f-]{36}$/.test(held.threadId) || !("origin" in held)) return;
    created = { threadId: held.threadId, origin: readExecutionOrigin(held.origin) };
  }
  if (!("pending" in value) || value.pending === undefined) return { ...saved, created };
  const pending = value.pending;
  if (!pending || typeof pending !== "object" || !("kind" in pending) || !("envelope" in pending) || (pending.kind !== "describeProjects" && pending.kind !== "describeBranches" && pending.kind !== "describeHandoff" && pending.kind !== "startSession" && pending.kind !== "describeSession" && pending.kind !== "updateSession" && pending.kind !== "readSessionChanges" && pending.kind !== "compactSession")) return;
  if (created) return;
  return { ...saved, pending: { kind: pending.kind, envelope: parseComputerRequestEnvelope(pending.envelope) } };
}
async function acquire(core: CoreClient, gateway: string, computerId: string, threadId?: string) {
  const account = await computerAccount(core, gateway);
  const journal = computerJournal(`${JSON.stringify(account.scope)}:${computerId}${threadId ? `:session:${threadId}` : ""}`, account.sender.deviceId, account.sender.holder, parseSaved);
  return computerSession({ core, scope: account.scope, computerId, threadId, sender: account.sender, journal });
}
export function useComputerSession(computerId: string, threadId?: string) {
  const core = useCore();
  const { gateway } = useAuth();
  const owner = useMemo(() => ({}), [core, gateway, computerId, threadId]);
  const [held, setHeld] = useState<{ owner: object; session: ComputerSession }>();
  const session = held?.owner === owner ? held.session : undefined;
  const [failure, setFailure] = useState<{ owner: object; message: string; cause: FailureCause }>();
  useEffect(() => {
    let active = true;
    let resolved = false;
    let running = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const attempt = async () => {
      if (!active || resolved || running || !gateway || !computerId || AppState.currentState !== "active") return;
      running = true;
      try {
        const session = await acquire(core, gateway, computerId, threadId);
        if (!active) return;
        resolved = true;
        setFailure(undefined);
        setHeld({ owner, session });
      } catch (e) {
        if (active) setFailure({ owner, message: e instanceof Error ? e.message : String(e), cause: failureCause(e) });
      } finally {
        running = false;
        if (active && !resolved && AppState.currentState === "active") timer = setTimeout(() => void attempt(), RECONNECT_MS);
      }
    };
    void attempt();
    const subscription = AppState.addEventListener("change", (state) => { clearTimeout(timer); if (state === "active") void attempt(); });
    return () => { active = false; clearTimeout(timer); subscription.remove(); };
  }, [core, gateway, computerId, threadId, owner]);
  useFocusEffect(useCallback(() => {
    const activate = (active: boolean) => {
      session?.setActive(active);
      if (active && threadId && session?.snapshot().settings && !session.snapshot().pending) void session.describe();
    };
    activate(AppState.currentState === "active");
    const subscription = AppState.addEventListener("change", (state) => activate(state === "active"));
    return () => { subscription.remove(); session?.setActive(false); };
  }, [session, threadId]));
  const current = failure?.owner === owner ? failure : undefined;
  return { session, error: current?.message, cause: current?.cause };
}
export function useComputerState(session: ComputerSession) { return useSyncExternalStore(session.subscribe, session.snapshot, session.snapshot); }

const emptyState = { draft: "", computers: [], projects: [], agents: [], loaded: false, preparing: false } satisfies ReturnType<ComputerSession["snapshot"]>;
const emptySubscribe = () => () => {};
const emptySnapshot = () => emptyState;
export function useOptionalComputerState(session?: ComputerSession) { return useSyncExternalStore(session?.subscribe ?? emptySubscribe, session?.snapshot ?? emptySnapshot, session?.snapshot ?? emptySnapshot); }
