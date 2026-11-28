// @vitest-environment happy-dom
import type { PageComment } from "@nexus/kit";
import { expect, it, vi } from "vitest";
import { readSitePinEvent, siteMarkers, sitePinScript, updateSitePins } from "./site-pins";

const root: PageComment = {
  id: "pcm_new",
  pageId: "pag_site",
  mine: true,
  body: "Check this heading",
  pin: { board: "site:/", x: 60, y: 100 },
  version: 5,
  createdAt: "2026-11-24T12:00:00Z",
};

it("renders only root pins for the specific saved Site version", () => {
  expect(siteMarkers([
    { ...root, id: "pcm_old", version: 4 },
    { ...root, id: "pcm_reply", threadId: root.id },
    { ...root, id: "pcm_design", pin: { ...root.pin!, board: "Design" } },
    root,
  ], 5)).toEqual([{ id: root.id, pin: root.pin }]);
});

it("opens a marker to read its thread and starts a draft only on a writable long press", () => {
  document.body.innerHTML = "<main><h1>Site preview</h1></main>";
  const report = vi.fn<(data: string) => void>();
  vi.stubGlobal("ReactNativeWebView", { postMessage: report });
  vi.useFakeTimers();
  try {
    new Function(sitePinScript)();
    expect(readSitePinEvent(JSON.parse(report.mock.lastCall![0]))).toEqual({ type: "ready", supported: true });
    new Function(updateSitePins(siteMarkers([root], 5), false))();
    document.querySelector("button")!.click();
    expect(readSitePinEvent(JSON.parse(report.mock.lastCall![0]))).toEqual({ type: "open", id: root.id });
    const hold = () => {
      document.querySelector("h1")!.dispatchEvent(new PointerEvent("pointerdown", {
        bubbles: true, clientX: 60, clientY: 100, button: 0, isPrimary: true, pointerId: 1,
      }));
      vi.advanceTimersByTime(550);
    };
    hold();
    expect(report).toHaveBeenCalledTimes(2);
    new Function(updateSitePins(siteMarkers([root], 5), true))();
    hold();
    expect(readSitePinEvent(JSON.parse(report.mock.lastCall![0]))).toEqual({
      type: "draft",
      pin: { board: "site:/", x: 60, y: 100 },
    });
  } finally {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  }
});
