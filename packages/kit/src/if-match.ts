/**
 * `If-Match` on an edit of an editable resource. Gateway refuses such an edit without `If-Match` (428
 * `if_match_required`) once `NEXUS_IF_MATCH_MODE=enforce`, and a stale one with 412 `version_mismatch`.
 *
 * The client keeps the `ETag` of the last read of each editable path, and sends it as `If-Match` on that path's edit.
 * So an edit carries the version its screen showed, and an edit made elsewhere since then is refused, not overwritten.
 * When the client never read the path, it reads it first. A path read without an `ETag` (a Gateway that does not
 * version it yet) is edited without `If-Match` and not read again. A connector grant set is versioned with the
 * organization's grant list, so its version is the list's (`READ_AT`). It mirrors the Rust client's
 * `nexus-client/src/etag.rs`.
 */

/**
 * The edits that need `If-Match`: `IF_MATCH_REQUIRED` in the Gateway's preconditions and the
 * contract's `x-if-match: required` (nexus-api-service). Each path's `GET` serves the `ETag`.
 */
// A copied list, as the Rust client's: derive it from the published contract once it ships `x-if-match`.
export const IF_MATCH_REQUIRED: readonly (readonly [method: string, path: string])[] = [
  ["PUT", "/v1/agents/access"],
  ["PATCH", "/v1/chat/sessions/{id}"],
  ["DELETE", "/v1/chat/sessions/{id}"],
  ["PUT", "/v1/notifications/settings"],
  ["PUT", "/v1/orgs/settings"],
  ["PUT", "/v1/people/{userId}/budget"],
  ["PUT", "/v1/policies/agents/{agent}"],
  ["PUT", "/v1/policies/chat-writes"],
  ["PUT", "/v1/policies/grants/{product}/{capability}"],
  ["PUT", "/v1/policies/organization"],
  ["PUT", "/v1/policies/review-events"],
  ["PUT", "/v1/policies/teams/{slug}"],
  ["PUT", "/v1/presence/settings"],
  ["PATCH", "/v1/teams/{slug}"],
  ["DELETE", "/v1/teams/{slug}"],
];

/** The path without its query: the resource an `ETag` belongs to. */
export const resourceOf = (path: string): string => path.split(/[?#]/)[0] ?? path;

/**
 * The edits whose `ETag` another path's `GET` sends: a connector grant set takes the version of the organization's
 * grants, which their list sends. Every edit of the pattern shares the list's version.
 */
export const READ_AT: readonly (readonly [edit: string, read: string])[] = [["/v1/policies/grants/{product}/{capability}", "/v1/policies/grants"]];

/**
 * The path whose `GET` sends the `ETag` an edit of `path` needs: the list it is versioned with (`READ_AT`), else its own
 * resource. The client keeps each version under it.
 */
export const readPathOf = (path: string): string => READ_AT.find(([edit]) => fills(edit, path))?.[1] ?? resourceOf(path);

/** Whether `path` fills `pattern`, where `{name}` stands for one non-empty segment. */
function fills(pattern: string, path: string): boolean {
  const want = pattern.split("/");
  const have = resourceOf(path).split("/");
  return want.length === have.length && want.every((segment, i) => (segment.startsWith("{") ? have[i] !== "" : segment === have[i]));
}

/** Whether this call edits an editable resource, so it needs `If-Match`. */
export const needsIfMatch = (method: string, path: string): boolean =>
  IF_MATCH_REQUIRED.some(([verb, pattern]) => verb === method.toUpperCase() && fills(pattern, path));

/** The editable resource `path` is, or lies under (a session's `/messages`), if any. */
function editableAt(path: string): string | undefined {
  const segments = resourceOf(path).split("/");
  for (let end = segments.length; end > 1; end--) {
    const prefix = segments.slice(0, end).join("/");
    if (IF_MATCH_REQUIRED.some(([, pattern]) => fills(pattern, prefix)) || READ_AT.some(([, read]) => read === prefix)) return prefix;
  }
  return undefined;
}

/**
 * What a client knows of each editable path's version: absent, never read; a string, the last `ETag`; `null`, read
 * with none (a Gateway that does not version it yet, so an edit need not read it again).
 */
export class Etags {
  private readonly known = new Map<string, string | null>();

  /** `undefined`: never read. `null`: read, with no `ETag`. */
  get(path: string): string | null | undefined {
    return this.known.get(readPathOf(path));
  }

  /**
   * Learn from an answer to `method path`. A read's `ETag`, or its lack, is the version now, and so is a successful
   * edit's `ETag`. A refused version, a deleted resource, an edit that answers no `ETag` over a known one, and any
   * successful write under the resource (a session's new message bumps the session) are forgotten, so the next edit
   * reads again.
   */
  learn(method: string, path: string, status: number, etag: string | null): void {
    const resource = editableAt(path);
    if (!resource) return;
    const verb = method.toUpperCase();
    const ok = status >= 200 && status < 300;
    if (resource !== resourceOf(path)) {
      if (ok && verb !== "GET" && verb !== "HEAD") this.known.delete(resource);
      return;
    }
    const key = readPathOf(resource);
    if (status === 412 || (ok && verb === "DELETE")) this.known.delete(key);
    else if (ok && verb === "GET") this.known.set(key, etag);
    else if (ok && etag) this.known.set(key, etag);
    else if (ok && this.known.get(key)) this.known.delete(key);
  }
}
