import { sessionKey, type UnifiedSession } from "@nexus/kit/session-history";

export type DrawerOrganization = {
  pinned: string[];
  sections: { name: string; sessions: string[] }[];
};

export const EMPTY_DRAWER_ORGANIZATION: DrawerOrganization = { pinned: [], sections: [] };

export function organizeDrawerSessions(sessions: readonly UnifiedSession[], organization: DrawerOrganization) {
  const available = new Map(sessions.map((session) => [sessionKey(session), session]));
  const take = (keys: string[]) => keys.flatMap((key) => {
    const session = available.get(key);
    if (!session) return [];
    available.delete(key);
    return [session];
  });
  const pinned = take(organization.pinned);
  const sections = organization.sections.map(({ name, sessions }) => ({ name, sessions: take(sessions) }));
  return { pinned, sections, remaining: [...available.values()] };
}

export function moveDrawerSession(organization: DrawerOrganization, key: string, destination: string | null): DrawerOrganization {
  const sections = organization.sections.map(({ name, sessions }) => {
    const others = sessions.filter((id) => id !== key);
    return { name, sessions: name === destination ? [key, ...others] : others };
  });
  if (destination && !sections.some(({ name }) => name === destination)) sections.push({ name: destination, sessions: [key] });
  return { ...organization, sections };
}
