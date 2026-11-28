import type { PageComment, PagePin } from "@nexus/kit";

const SITE = "site:/";

export type SiteMarker = { id: string; pin: PagePin };

export type SitePinEvent =
  | { type: "ready"; supported: boolean }
  | { type: "open"; id: string }
  | { type: "draft"; pin: PagePin };

export function siteMarkers(comments: readonly PageComment[], version: number | undefined): SiteMarker[] {
  if (version === undefined) return [];
  return comments.flatMap((comment) =>
    !comment.threadId && comment.version === version && comment.pin?.board === SITE
      ? [{ id: comment.id, pin: comment.pin }]
      : []);
}

const bounded = (value: unknown, max: number): value is number =>
  typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= max;

export function readSitePinEvent(value: unknown): SitePinEvent | undefined {
  if (!value || typeof value !== "object" || !("kind" in value) || value.kind !== "nexus.site-pin" || !("type" in value)) return;
  if (value.type === "ready" && "supported" in value && typeof value.supported === "boolean") {
    return { type: "ready", supported: value.supported };
  }
  if (value.type === "open" && "id" in value && typeof value.id === "string" && /^pcm_[A-Za-z0-9_-]{1,100}$/.test(value.id)) {
    return { type: "open", id: value.id };
  }
  if (value.type !== "draft" || !("x" in value) || !bounded(value.x, 100_000) || !("y" in value) || !bounded(value.y, 100_000)) return;
  const pin: PagePin = { board: SITE, x: value.x, y: value.y };
  if ("path" in value && typeof value.path === "string" && /^\d{1,5}(\.\d{1,5}){0,32}$/.test(value.path)
    && "tag" in value && typeof value.tag === "string" && /^[a-z][a-z0-9-]{0,40}$/.test(value.tag)
    && "px" in value && bounded(value.px, 1) && "py" in value && bounded(value.py, 1)) {
    Object.assign(pin, { path: value.path, tag: value.tag, px: value.px, py: value.py });
  }
  return { type: "draft", pin };
}

export function updateSitePins(markers: SiteMarker[], writable: boolean): string {
  return `window.nexusSitePins?.(${JSON.stringify(markers)}, ${writable}); true;`;
}

/** Installs the markers and the long-press selection in the sandboxed live Site frame. */
export const sitePinScript = `(() => {
  if (window.nexusSitePins) return;
  const report = (event) => window.ReactNativeWebView.postMessage(JSON.stringify({ kind: "nexus.site-pin", ...event }));
  const root = document.createElement("div");
  root.setAttribute("data-native-site-pins", "");
  root.style.cssText = "position:absolute;inset:0;pointer-events:none;z-index:2147483647;";
  document.body.append(root);
  let writable = false;
  let pending;
  let suppressUntil = 0;
  const supported = () => !document.querySelector("[data-artboard], [data-slide]");
  const point = (pin) => {
    if (pin.path && pin.tag && pin.px !== undefined && pin.py !== undefined) {
      let target = document.body;
      for (const part of pin.path.split(".")) target = target?.children[Number(part)];
      if (target && target.tagName.toLowerCase() === pin.tag) {
        const rect = target.getBoundingClientRect();
        if (rect.width && rect.height) {
          return { x: window.scrollX + rect.left + pin.px * rect.width, y: window.scrollY + rect.top + pin.py * rect.height };
        }
      }
    }
    return pin;
  };
  const cancel = () => {
    if (pending) clearTimeout(pending.timer);
    pending = undefined;
  };

  function localAnchor(target, x, y) {
    const rect = target.getBoundingClientRect();
    const path = [];
    for (let node = target; node && node !== document.body; node = node.parentElement) {
      const index = Array.prototype.indexOf.call(node.parentElement.children, node);
      path.unshift(index);
    }
    const result = { x: x + window.scrollX, y: y + window.scrollY };
    if (path.length && path.length <= 32 && rect.width && rect.height) {
      return {
        ...result, tag: target.tagName.toLowerCase(), path: path.join("."),
        px: Math.min(1, Math.max(0, (x - rect.left) / rect.width)),
        py: Math.min(1, Math.max(0, (y - rect.top) / rect.height))
      };
    }
    return result;
  }

  let markers = [];
  function draw() {
    root.replaceChildren();
    if (!supported()) return;
    const origin = root.getBoundingClientRect();
    const left = origin.left + window.scrollX;
    const top = origin.top + window.scrollY;
    for (const [index, marker] of markers.entries()) {
      const { x, y } = point(marker.pin);
      const button = document.createElement("button");
      button.type = "button";
      button.setAttribute("aria-label", "Comment " + (index + 1));
      button.textContent = String(index + 1);
      button.style.cssText = "position:absolute;min-width:44px;height:44px;border-radius:22px;border:2px solid Canvas;color:Canvas;background:CanvasText;pointer-events:auto;font:600 13px system-ui;transform:translate(-50%,-50%);";
      button.style.left = Math.max(22, x - left) + "px";
      button.style.top = Math.max(22, y - top) + "px";
      button.addEventListener("click", (event) => {
        event.stopPropagation();
        report({ type: "open", id: marker.id });
      });
      root.append(button);
    }
  }

  window.nexusSitePins = (next, canWrite) => {
    writable = canWrite;
    markers = next;
    draw();
  };
  window.addEventListener("resize", draw);
  window.addEventListener("scroll", cancel, true);
  document.addEventListener("pointerdown", (event) => {
    const target = event.target;
    if (!writable || !supported() || !event.isPrimary || event.button !== 0
      || !(target instanceof Element) || target.closest("[data-native-site-pins],a,button,input,textarea,select,[contenteditable]")) return;
    cancel();
    const { clientX: x, clientY: y, pointerId } = event;
    const timer = setTimeout(() => {
      pending = undefined;
      if (!writable || !target.isConnected || !supported()) return;
      suppressUntil = Date.now() + 700;
      report({ type: "draft", ...localAnchor(target, x, y) });
    }, 500);
    pending = { x, y, pointerId, timer };
  }, true);
  document.addEventListener("pointermove", (event) => {
    if (pending && event.pointerId === pending.pointerId && Math.hypot(event.clientX - pending.x, event.clientY - pending.y) > 10) cancel();
  }, true);
  document.addEventListener("pointerup", cancel, true);
  document.addEventListener("pointercancel", cancel, true);
  document.addEventListener("click", (event) => {
    if (Date.now() < suppressUntil && event.target instanceof Element && !event.target.closest("[data-native-site-pins]")) {
      event.preventDefault();
      event.stopPropagation();
    }
  }, true);
  report({ type: "ready", supported: supported() });
})(); true;`;
