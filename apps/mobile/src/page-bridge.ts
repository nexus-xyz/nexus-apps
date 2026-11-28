/**
 * The page bridge's way in and out of the phone's WebView. In Nexus UI a page posts to its frame's parent;
 * here the page is the WebView's top document, so the sandbox host's shim posts JSON text to `ReactNativeWebView`, and
 * the answer goes back as a message from the page's own window. `createBridge` (`@nexus/kit`) checks and answers the
 * call in between, as it does for Nexus UI.
 */
import type { BridgeReply } from "@nexus/kit";
import { pageNavigation } from "./pages";

/**
 * A page's call, from the WebView's `onMessage`: its data, or undefined when it is not one. Only a message sent while
 * the WebView shows the frame's own origin counts; `pageNavigation` keeps it there, and this checks it again.
 */
export function nativeCall(frameUrl: string, url: string, text: string): unknown {
  if (pageNavigation(frameUrl, url) !== "load") return undefined;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

/** A reader's kept Allow or Deny for one question about one page, as `useStored` keeps it. */
export interface Choice {
  load(): boolean | undefined;
  save(allow: boolean): void;
}

/**
 * One question the bridge asks the reader (`consent`, `confirmShared`), as Nexus UI's frame asks it. A kept choice
 * answers at once. Else `show` asks, and calls that arrive while it is open wait for the same answer, which is kept.
 * `show` calls `decide` once.
 */
export function asker(choice: () => Choice, show: (decide: (allow: boolean) => void) => void): () => Promise<boolean> {
  let open: Promise<boolean> | undefined;
  return () => {
    const kept = choice().load();
    if (kept !== undefined) return Promise.resolve(kept);
    return (open ??= new Promise<boolean>((resolve) => {
      const asked = choice();
      show((allow) => {
        open = undefined;
        asked.save(allow);
        resolve(allow);
      });
    }));
  };
}

/**
 * The script that hands `reply` to the page, for `injectJavaScript`: a message from the page's own window, which the
 * shim takes as its parent's. The reply is JSON, so a model's answer is a string literal here, never code. U+2028 and
 * U+2029 are escaped for engines that still end a line at them. `true` last, as react-native-webview asks.
 *
 * JSON has no `Uint8Array`, so `fs.readFile`'s bytes cross as base64 and become one again in the page.
 */
export function replyScript(reply: BridgeReply): string {
  const bytes = "result" in reply && reply.result instanceof Uint8Array ? reply.result : undefined;
  if (!bytes) return `window.postMessage(${literal(reply)}, "*");true;`;
  return `(function () { var b = atob(${literal(base64(bytes))}), a = new Uint8Array(b.length), r = ${literal({ ...reply, result: null })}; for (var i = 0; i < b.length; i++) a[i] = b.charCodeAt(i); r.result = a; window.postMessage(r, "*"); })();true;`;
}

const literal = (value: unknown) => JSON.stringify(value).replace(/\u2028/g, "\\u2028").replace(/\u2029/g, "\\u2029");

/** In chunks: `String.fromCharCode` takes one argument per byte, and a call has a limit on its arguments. */
function base64(bytes: Uint8Array): string {
  let binary = "";
  for (let start = 0; start < bytes.length; start += 0x8000) binary += String.fromCharCode(...bytes.subarray(start, start + 0x8000));
  return btoa(binary);
}
