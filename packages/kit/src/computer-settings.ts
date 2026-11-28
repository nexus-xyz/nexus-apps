import type { ComputerAgent, ComputerAgentChoices, ComputerEffort, ComputerModelChoice, ComputerModelSettings, ComputerPayer, ComputerRouteChoices, ComputerSelection, ComputerSessionSettings, ComputerVia } from "./computer-types";
import { object, protocolError, text, uuid } from "./computer-protocol";

const EFFORTS: readonly ComputerEffort[] = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
const AGENTS: readonly ComputerAgent[] = ["claude", "codex", "nexus"];
/** Bounds a list a newer computer may extend with agents or levels this phone does not know. */
const MAX_FORWARD_ENTRIES = 32;
function label(value: unknown): string {
  const result = text(value, 1024);
  if (Array.from(result).length > 256) throw protocolError();
  return result;
}
const nullable = (value: unknown, maximum = 256): string | null => value === null ? null : text(value, maximum);
function via(value: unknown): ComputerVia {
  if (value !== "nexus" && value !== "subscription") throw protocolError();
  return value;
}
/** A choice this phone sends or saves holds only levels it offers, so an unknown level is an error here. */
function effort(value: unknown): ComputerEffort | null {
  if (value === null) return null;
  for (const choice of EFFORTS) if (value === choice) return choice;
  throw protocolError();
}
export const isComputerEffort = (value: string): value is ComputerEffort => EFFORTS.some((choice) => choice === value);
export const isComputerAgent = (value: string): value is ComputerAgent => AGENTS.some((choice) => choice === value);
/** A newer computer may report an agent, level, route, payer or state this phone does not know: it reads as `undefined`. */
function knownWord<T extends string>(value: unknown, words: readonly T[]): T | undefined {
  const word = text(value, 64);
  return words.find((choice) => choice === word);
}
const knownEffort = (value: unknown) => knownWord(value, EFFORTS);
const knownAgent = (value: unknown) => knownWord(value, AGENTS);
function payerKind(value: unknown): ComputerSelection["payer"] {
  if (value !== "plan" && value !== "apiKey" && value !== "gateway") throw protocolError();
  return value;
}
function routeMatches(source: ComputerVia, payer: ComputerSelection["payer"]) {
  if ((source === "nexus") !== (payer === "gateway")) throw protocolError();
}
export function readComputerPayer(value: unknown): ComputerPayer {
  const fields = object(value);
  const kind = fields.kind === "unknown" ? "unknown" : payerKind(fields.kind);
  return { kind, label: label(fields.label), detail: nullable(fields.detail, 2048) };
}
export function readComputerSelection(value: unknown): ComputerSelection {
  const fields = object(value);
  const selection = { ...readComputerModelSettings(fields), via: via(fields.via), payer: payerKind(fields.payer) };
  routeMatches(selection.via, selection.payer);
  return selection;
}
export function readComputerModelSettings(value: unknown): ComputerModelSettings {
  const fields = object(value);
  return { model: nullable(fields.model), effort: effort(fields.effort) };
}
function models(value: unknown): ComputerModelChoice[] {
  if (!Array.isArray(value) || value.length > 128) throw protocolError();
  const result = value.map((entry: unknown) => {
    const fields = object(entry);
    if (!Array.isArray(fields.efforts) || fields.efforts.length > MAX_FORWARD_ENTRIES) throw protocolError();
    const efforts = fields.efforts.map(knownEffort).filter((level): level is ComputerEffort => level !== undefined);
    if (new Set(efforts).size !== efforts.length) throw protocolError();
    return { id: text(fields.id, 256), name: label(fields.name), efforts };
  });
  if (new Set(result.map((model) => model.id)).size !== result.length) throw protocolError();
  return result;
}
function routes(value: unknown): ComputerRouteChoices[] {
  if (!Array.isArray(value) || value.length > MAX_FORWARD_ENTRIES) throw protocolError();
  // A route this phone does not know is left out: the phone cannot send it back as a selection.
  const result = value.flatMap((entry: unknown): ComputerRouteChoices[] => {
    const fields = object(entry);
    const source = knownWord(fields.via, ["nexus", "subscription"] as const);
    const payer = knownWord(fields.payer, ["plan", "apiKey", "gateway"] as const);
    const state = knownWord(fields.state, ["usable", "unavailable", "unknown"] as const);
    if (!source || !payer || !state) return [];
    routeMatches(source, payer);
    return [{ via: source, payer, state, label: label(fields.label), detail: nullable(fields.detail, 2048), models: models(fields.models), defaultModel: nullable(fields.defaultModel) }];
  });
  if (new Set(result.map((route) => `${route.via}:${route.payer}`)).size !== result.length) throw protocolError();
  return result;
}
export function readComputerAgents(value: unknown): ComputerAgentChoices[] {
  if (!Array.isArray(value) || value.length > MAX_FORWARD_ENTRIES) throw protocolError();
  // An agent this phone does not know is left out: the phone cannot offer its routes.
  // A project of that agent still starts, with the computer's own settings for it.
  const result = value.flatMap((entry: unknown): ComputerAgentChoices[] => {
    const fields = object(entry);
    const agent = knownAgent(fields.agent);
    return agent ? [{ agent, name: label(fields.name), routes: routes(fields.routes) }] : [];
  });
  if (new Set(result.map((entry) => entry.agent)).size !== result.length) throw protocolError();
  return result;
}
export function readComputerSessionSettings(value: unknown): ComputerSessionSettings {
  const fields = object(value);
  if (typeof fields.editable !== "boolean") throw protocolError();
  const agent = knownAgent(fields.agent);
  let pending: ComputerSessionSettings["pending"] = null;
  if (fields.pending !== null) {
    const requested = object(fields.pending);
    // A requested level this phone does not know is left out of `effort` and marked, so it never reads as default requested.
    const level = requested.effort === null ? null : requested.effort === undefined ? undefined : knownEffort(requested.effort);
    pending = {
      ...(requested.model !== undefined ? { model: nullable(requested.model) } : {}),
      ...(level !== undefined ? { effort: level } : {}),
      ...(level === undefined && requested.effort !== undefined ? { effortUnknown: true as const } : {}),
    };
  }
  const source = knownWord(fields.via, ["nexus", "subscription"] as const);
  const payer = readComputerPayer(fields.payer);
  if (source && payer.kind !== "unknown") routeMatches(source, payer.kind);
  const reason = nullable(fields.reason, 2048);
  const known = agent !== undefined && source !== undefined;
  // A current level this phone does not know reads as null, marked so that no update replaces it unasked.
  const current = fields.effort === null ? null : knownEffort(fields.effort);
  // A session of an agent or route this phone does not know still shows, read-only: the phone cannot judge its settings.
  return { threadId: uuid(fields.threadId), revision: text(fields.revision, 128), agent: agent ?? "unknown", via: source ?? "unknown",
    model: nullable(fields.model), effort: current ?? null, ...(current === undefined ? { effortUnknown: true as const } : {}),
    payer, pending, models: models(fields.models), editable: known && fields.editable,
    reason: known ? reason : "Update Nexus on this phone to change this session's settings." };
}
