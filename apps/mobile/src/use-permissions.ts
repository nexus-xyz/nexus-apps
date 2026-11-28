import { useEffect, useState } from "react";
import { useCore, useStored } from "@/auth";

/**
 * What you may do in the organization you are in (`GET /v1/tenants/me`'s `permissions`), as last time until the
 * Gateway answers, so a page that hides what you may not open does not move its rows when the answer lands. Undefined
 * while nothing is known: a caller then shows nothing that needs a permission. `load` false reads the cache alone.
 */
export function usePermissions(load = true): readonly string[] | undefined {
  const core = useCore();
  const cache = useStored<string[]>("cache", "permissions");
  const [permissions, setPermissions] = useState<string[] | undefined>(() => cache.load());
  useEffect(() => {
    // The organization changed under it (a switch): its cache is another's, read it again.
    setPermissions(cache.load());
    if (!load) return;
    let active = true;
    core.me().then(
      (me) => {
        if (!active) return;
        const next = me.permissions ?? [];
        setPermissions(next);
        cache.save(next);
      },
      () => undefined,
    );
    return () => { active = false; };
  }, [core, cache, load]);
  return permissions;
}
