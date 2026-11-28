export const LAST_SESSION_PLACE = "last-session";

export type SessionPlace = { id: string; source: "chat" | "synced" };

export function launchSession(saved: SessionPlace | undefined) {
  if (!saved || typeof saved.id !== "string" || !/^[a-z0-9_-]{1,160}$/i.test(saved.id)) return "/session/new" as const;
  if (saved.source !== "chat" && saved.source !== "synced") return "/session/new" as const;
  return { pathname: "/session/[id]" as const, params: { id: saved.id, source: saved.source } };
}
