import type { OrgSummary } from "@nexus/kit";
import { useCallback, useRef, useState } from "react";
import { useAuth } from "@/auth";
import { haptic } from "@/haptics";
import { rememberOrg } from "@/org-picker";
import { orgLabel, orgSwitchKey } from "@/orgs";
import { load, save } from "@/store";
import { useToast } from "@/toast";

/** The organizations you were in, newest first: across organizations, so not `useStored`; sign-out forgets it. */
const RECENT = "recent:orgs";

export const loadRecentOrgs = () => load<string[]>(RECENT) ?? [];

/**
 * Switches organization from Settings' sheet and the Organizations page, the same way each time: a
 * selection tick, the row that was touched busy, then a toast that says where you are now, with Undo back to where you
 * were. Both organizations go to the front of the recent list, so the one you left heads the sheet's Recent list.
 */
export function useSwitchOrg(current: OrgSummary | undefined) {
  const { switchOrg } = useAuth();
  const toast = useToast();
  const [switching, setSwitching] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [recent, setRecent] = useState(loadRecentOrgs);
  const busy = useRef(false);

  const go = useCallback(
    async (org: OrgSummary, from: OrgSummary | undefined, undo: boolean): Promise<boolean> => {
      if (busy.current) return false;
      busy.current = true;
      haptic("selection");
      setSwitching(org.id);
      setProblem(null);
      try {
        await switchOrg(orgSwitchKey(org));
        const next = rememberOrg(from ? rememberOrg(loadRecentOrgs(), from.id) : loadRecentOrgs(), org.id);
        save(RECENT, next);
        setRecent(next);
        const back = undo || !from ? undefined : { label: "Undo", onPress: () => void go(from, org, true) };
        toast({ text: `${undo ? "Back" : "Now"} in ${orgLabel(org)}`, icon: "org", action: back, haptic: "success" });
        return true;
      } catch (e) {
        setProblem((e as Error).message);
        haptic("error");
        return false;
      } finally {
        busy.current = false;
        setSwitching(null);
      }
    },
    [switchOrg, toast],
  );

  /** Switches to `org`; true once you are in it. The one you are in already is a no-op. */
  const pick = useCallback((org: OrgSummary) => (org.current ? Promise.resolve(true) : go(org, current, false)), [go, current]);
  return { pick, switching, problem, recent };
}
