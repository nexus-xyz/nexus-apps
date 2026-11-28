import { useFocusEffect } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { useCore, useStored } from "./auth";
import { noBilling } from "./billing";
import { payNoticeOf, planOf, usageOf, type PayNotice, type Plan, type Usage } from "./plan";

type Core = ReturnType<typeof useCore>;
/**
 * `freeStartMaxUsd`: the free start pays for a model whose output costs at most this many dollars per million tokens
 * (`GET /v1/billing`'s `pricing.freeStartMaxOutputUsd`); absent for a member, or from an older Gateway.
 * `notice`: what the composer says about paying, from `GET /v1/spend`'s `nexusPays`. `freeStartPays`: the
 * free start pays the next call.
 */
type Known = { plan: Plan; usage?: Usage; freeStartMaxUsd?: number; notice?: PayNotice; freeStartPays?: boolean };

// One read per signed-in client, shared by the chat, Settings and the organization page, and kept a minute: a chat opened from
// the sidebar does not read it again.
const reads = new WeakMap<Core, { at: number; read: Promise<Known> }>();
const FRESH_MS = 60_000;

/** How the organization pays (`GET /v1/billing`, for owners) and the Nexus usage (`GET /v1/spend`). A billing load that fails fails it. */
async function read(core: Core): Promise<Known> {
  const [me, spend] = await Promise.all([core.me(), core.spend().catch(() => undefined)]);
  const billing = me.permissions?.includes("org.billing") ? await core.billing().catch((e: unknown) => (noBilling(e) ? null : Promise.reject(e))) : undefined;
  const max = Number(billing?.pricing?.freeStartMaxOutputUsd ?? NaN);
  const usage = spend ? usageOf(spend) : undefined;
  const notice = spend ? payNoticeOf(spend, Date.now()) : undefined;
  const freeStartPays = spend?.nexusPays ? spend.nexusPays.payer === "free_start" : Boolean(usage && !usage.usedUp);
  return {
    plan: planOf(me.tenant.kind === "personal", billing),
    usage,
    ...(notice ? { notice } : {}),
    freeStartPays,
    ...(Number.isFinite(max) ? { freeStartMaxUsd: max } : {}),
  };
}

/**
 * How the organization pays and its Nexus usage: as last time until Gateway answers, and read again when the
 * screen comes back into view. `refresh` reads now, as after a refused answer. `loaded` is false until there is an answer;
 * `error` is the last read's failure.
 */
export function usePlan(): Known & { loaded: boolean; error?: string; refresh: () => void } {
  const core = useCore();
  const cache = useStored<Known>("cache", "plan");
  const [known, setKnown] = useState<Known | undefined>(() => cache.load());
  const [error, setError] = useState<string | undefined>();
  // After a switch, the new organization's last answer.
  useEffect(() => setKnown(cache.load()), [cache]);
  const load = useCallback(
    (now = false) => {
      let entry = reads.get(core);
      if (now || !entry || Date.now() - entry.at > FRESH_MS) {
        entry = { at: Date.now(), read: read(core) };
        reads.set(core, entry);
      }
      const current = entry;
      current.read.then(
        (next) => {
          setKnown(next);
          setError(undefined);
          cache.save(next);
        },
        // A failed read keeps the last answer and is not kept for the next screen.
        (e: Error) => {
          setError(e.message);
          if (reads.get(core) === current) reads.delete(core);
        },
      );
    },
    [core, cache],
  );
  useFocusEffect(useCallback(() => load(), [load]));
  const refresh = useCallback(() => load(true), [load]);
  return {
    plan: known?.plan ?? { kind: "unknown" },
    usage: known?.usage,
    notice: known?.notice,
    freeStartPays: known?.freeStartPays,
    freeStartMaxUsd: known?.freeStartMaxUsd,
    loaded: !!known,
    error,
    refresh,
  };
}
