import type { CoreClient } from "@nexus/kit";
import type { ComputerScope } from "@nexus/kit/computer-types";
import type { DeviceKey } from "@nexus/kit/e2e";
import { useEffect, useMemo, useRef, useState } from "react";
import { codeDraftStore, codeWaitStore, restoredDraft, type CodeDraftStore, type CodeWaitStore } from "./code-draft";
import { connectionSignal } from "./reconnect";

const SAVE_DELAY_MS = 400;

/**
 * The draft store and the waiting-message store for one session, scoped like `ComputerSession` (Gateway,
 * organization, person). Both `undefined` until Gateway names the person, or when it names none: they then stay in
 * memory only.
 */
export function useCodeStores(core: CoreClient, gateway: string | undefined, threadId: string, reader: { deviceId: string; holder: DeviceKey }): { draft?: CodeDraftStore; wait?: CodeWaitStore } {
  const [held, setHeld] = useState<{ core: CoreClient; gateway: string; scope: ComputerScope }>();
  useEffect(() => {
    if (!gateway) return;
    let live = true;
    core.me(connectionSignal()).then(
      (me) => { if (live && me.user?.id) setHeld({ core, gateway, scope: { gateway, tenantId: me.tenant.id, personId: me.user.id } }); },
      () => undefined,
    );
    return () => { live = false; };
  }, [core, gateway]);
  const scope = held?.core === core && held.gateway === gateway ? held.scope : undefined;
  const { deviceId, holder } = reader;
  return useMemo(() => (scope ? { draft: codeDraftStore(scope, threadId, { deviceId, holder }), wait: codeWaitStore(scope, threadId, { deviceId, holder }) } : {}), [scope, threadId, deviceId, holder]);
}

/**
 * A Code session's composer text, kept through navigation and an app restart. The store writes only ciphertext.
 * `delivered` is the text of a message the computer confirmed: a saved copy of it is dropped, not shown again. An
 * empty draft removes the saved one, so a confirmed send clears it.
 */
export function useCodeDraft(store: CodeDraftStore | undefined, initial: string, delivered: string | undefined) {
  const [draft, setDraft] = useState(initial);
  const loaded = useRef(!store);
  const latest = useRef(draft);
  latest.current = draft;
  const confirmed = useRef(delivered);
  confirmed.current = delivered;
  const unsaved = useRef<string | undefined>(undefined);

  useEffect(() => {
    if (!store) return;
    let live = true;
    loaded.current = false;
    store.load().then(
      (saved) => {
        if (!live) return;
        loaded.current = true;
        const text = restoredDraft(saved, latest.current, confirmed.current);
        if (text !== undefined) setDraft(text);
        else if (saved && !latest.current) void store.remove().catch(() => undefined);
      },
      () => {
        // A saved draft that no longer opens is dropped: it must never lock the composer.
        if (!live) return;
        loaded.current = true;
        void store.remove().catch(() => undefined);
      },
    );
    return () => {
      live = false;
      write(store, unsaved);
    };
  }, [store]);

  useEffect(() => {
    if (!store || !loaded.current) return;
    unsaved.current = draft;
    const timer = setTimeout(() => write(store, unsaved), SAVE_DELAY_MS);
    return () => clearTimeout(timer);
  }, [store, draft]);

  return [draft, setDraft] as const;
}

function write(store: CodeDraftStore, unsaved: { current: string | undefined }) {
  const text = unsaved.current;
  if (text === undefined) return;
  unsaved.current = undefined;
  void (text.trim() ? store.save({ text }) : store.remove()).catch(() => undefined);
}
