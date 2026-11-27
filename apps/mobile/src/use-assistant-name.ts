import { useEffect, useSyncExternalStore } from "react";
import { assistantNameOf, known, nameOf, setName, subscribe } from "@/assistant-name";
import { useAuth, useStored } from "@/auth";

/** Accounts whose profile this run has asked for already. */
const asked = new Set<string>();

/**
 * The Assistant's name: the person's choice, Nexus by default (`assistant-name.ts`). The first reader in a
 * run reads the person's own agent profile once, through the sign-in's Gateway client, and keeps the name on the
 * device for the next start. A Gateway without profiles, or a failed read, leaves Nexus (rule 8).
 */
export function useAssistantName(): string {
  const { core, org } = useAuth();
  const kept = useStored<string>("cache", "assistant-name");
  const name = useSyncExternalStore(subscribe, () => nameOf(org));

  useEffect(() => {
    if (!org || asked.has(org)) return;
    asked.add(org);
    const last = kept.load();
    if (last && !known(org)) setName(org, last);
    const list = core && typeof core.listAgentProfiles === "function" ? core.listAgentProfiles() : undefined;
    list?.then(
      (profiles) => {
        const read = assistantNameOf(profiles.data);
        setName(org, read);
        kept.save(read);
      },
      () => {
        // A Gateway without profiles answers 404: the name stays as it was. Ask again on the next start.
        asked.delete(org);
      },
    );
  }, [core, org, kept]);

  return name;
}
