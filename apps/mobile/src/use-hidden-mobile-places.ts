import type { MobilePlace } from "@nexus/kit";
import { useEffect, useState } from "react";
import { useCore, useStored } from "@/auth";
import { transientError } from "@/connection-errors";

export function useHiddenMobilePlaces(opened: boolean): readonly MobilePlace[] {
  const core = useCore();
  const cache = useStored<MobilePlace[]>("cache", "hidden-mobile-places");
  const [known, setKnown] = useState(() => ({ core, places: cache.load() ?? [] }));
  useEffect(() => {
    if (!opened) return;
    let active = true;
    void core.orgSettings().then(({ hiddenMobilePlaces = [] }) => {
      if (!active) return;
      cache.save(hiddenMobilePlaces);
      setKnown({ core, places: hiddenMobilePlaces });
    }, (error: unknown) => {
      if (!active || transientError(error)) return;
      cache.remove();
      setKnown({ core, places: [] });
    });
    return () => { active = false; };
  }, [core, cache, opened]);
  return known.core === core ? known.places : cache.load() ?? [];
}
