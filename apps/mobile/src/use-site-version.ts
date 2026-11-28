import { useEffect, useState } from "react";
import { followLive, type Follow } from "./live-page";
import { useScreenFocused } from "./screen-focus";
import { nextSiteVersion, type SiteVersion } from "./site-version";

/** The last saved version and its draft indicator. An outage retains the last state the reader observed. */
export function useSiteVersion(pageId: string, follow: Follow | undefined): SiteVersion | undefined {
  const focused = useScreenFocused();
  const [state, setState] = useState<SiteVersion>();
  useEffect(() => {
    if (!focused || !follow) return;
    const request = new AbortController();
    void followLive(follow, (event) => {
      if (!request.signal.aborted) setState((current) => nextSiteVersion(current, pageId, event));
    }, request.signal);
    return () => request.abort();
  }, [follow, focused, pageId]);
  return state?.pageId === pageId ? state : undefined;
}
