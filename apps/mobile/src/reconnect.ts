export const RECONNECT_MS = 3000;

/** A short read deadline lets a waking computer recover without waiting for the general API timeout. */
export function connectionSignal(parent?: AbortSignal): AbortSignal {
  const deadline = AbortSignal.timeout(8000);
  return parent ? AbortSignal.any([parent, deadline]) : deadline;
}
