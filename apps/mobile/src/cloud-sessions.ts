import { CoreError, type Session } from "@nexus/kit";

export type CloudSessions = { state: "ready"; sessions: Session[] } | { state: "disabled" } | { state: "error"; error: string };

export function cloudError(error: unknown): string {
  if (error instanceof CoreError) {
    if (error.code === "github_connection_required" || error.code === "credentials_required" || error.code === "connection_required") return "Connect your GitHub account to show you can read this repository.";
    if (error.code === "connection_expired") return "Reconnect your GitHub account, then try again.";
    if (error.code === "account_required") return "Choose the GitHub account that can read this repository.";
    if (error.code === "repo_setup_unavailable") return "The Nexus GitHub App cannot access this repository.";
    if (error.status === 404) return "This Gateway does not have Cloud sessions enabled.";
  }
  return error instanceof Error ? error.message : "Could not reach Cloud sessions.";
}

export function cloudStatus(session: Session): string {
  if (session.state === "stopped") return "Stopped";
  if (session.state === "failed" || session.machine?.state === "failed" || session.machine?.state === "gone") return "Failed";
  if (session.machine?.state === "sleeping" || session.machine?.state === "stopping") return "Sleeping";
  return session.state === "starting" ? "Starting" : "Running";
}
