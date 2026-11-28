import { AGENT_LAUNCHERS, type AgentSessionSummary } from "@nexus/kit";

/**
 * The coding agents' sessions as the phone's Agents screen shows them (brand v5's P4 and P18): running first, the rest
 * under Done, each with how long it has run or ran. Agents are named with kit's `agentLabel`, from its agent catalog.
 */

/** The commands that start a coding agent, as a sentence lists them ("nexus claude, nexus codex or nexus agy"). */
export function launcherList(launchers: readonly string[] = AGENT_LAUNCHERS): string {
  return launchers.length > 1 ? `${launchers.slice(0, -1).join(", ")} or ${launchers.at(-1)}` : (launchers[0] ?? "");
}

export const STATUS: Record<AgentSessionSummary["status"], string> = {
  running: "Running",
  succeeded: "Done",
  failed: "Failed",
  interrupted: "Interrupted",
  lost: "Lost",
};

/** Running ones, newest first; then the rest, newest first. */
export function splitRuns(sessions: AgentSessionSummary[]) {
  const newest = (a: AgentSessionSummary, b: AgentSessionSummary) => Date.parse(b.startedAt) - Date.parse(a.startedAt);
  return {
    running: sessions.filter((s) => s.status === "running").sort(newest),
    done: sessions.filter((s) => s.status !== "running").sort(newest),
  };
}

/** The screen's line under its title: how many, and how many run now ("5 sessions · 2 running"). */
export function runsLine(sessions: AgentSessionSummary[]): string {
  const running = sessions.filter((s) => s.status === "running").length;
  return [`${sessions.length} ${sessions.length === 1 ? "session" : "sessions"}`, running ? `${running} running` : undefined].filter(Boolean).join(" · ");
}

/**
 * How long, as a clock: "04:12" under an hour, "1:02:07" past it. A running session counts to `now`; a finished one
 * shows what it took (`durationMs`, else its start to its end). Empty when neither is known.
 */
export function elapsed(session: Pick<AgentSessionSummary, "status" | "startedAt" | "endedAt" | "durationMs">, now = Date.now()): string {
  const start = Date.parse(session.startedAt);
  const ms =
    session.status === "running" ? now - start : (session.durationMs ?? (session.endedAt ? Date.parse(session.endedAt) - start : Number.NaN));
  if (!Number.isFinite(ms)) return "";
  const total = Math.max(Math.floor(ms / 1000), 0);
  const [h, m, s] = [Math.floor(total / 3600), Math.floor((total % 3600) / 60), total % 60];
  const two = (n: number) => String(n).padStart(2, "0");
  return h ? `${h}:${two(m)}:${two(s)}` : `${two(m)}:${two(s)}`;
}

/** A path's last folder, as the boards name where an agent works ("agents/spike-sort" → "spike-sort"). */
export const folder = (cwd: string | undefined) => cwd?.replace(/\/+$/, "").split("/").pop() || undefined;

/** The row's second line: what it is doing or how it ended, and where. */
export function runLine(session: Pick<AgentSessionSummary, "status" | "cwd" | "exitCode">): string {
  const where = folder(session.cwd);
  const how = session.status === "failed" && session.exitCode !== undefined ? `Failed · exit ${session.exitCode}` : STATUS[session.status];
  return where ? `${how} in ${where}` : how;
}
