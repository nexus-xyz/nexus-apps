// @vitest-environment happy-dom
import { expect, it, vi } from "vitest";
import { docOutlineScript, jumpToDocHeading, readDocOutline } from "./doc-contents";

it("reads rendered headings in order and jumps to the chosen heading even after an empty one", () => {
  document.body.innerHTML = "<h1>Overview</h1><h2></h2><h2>Results</h2><p>Read this</p>";
  const bridge = vi.fn<(message: string) => void>();
  vi.stubGlobal("ReactNativeWebView", { postMessage: bridge });
  const selected = document.querySelectorAll("h2")[1]!;
  const jump = vi.spyOn(selected, "scrollIntoView");
  try {
    new Function(docOutlineScript)();
    const wire = bridge.mock.calls[0]?.[0];
    expect(wire && readDocOutline(JSON.parse(wire))).toMatchObject({
      headings: [
        { index: 0, title: "Overview", level: 1 },
        { index: 2, title: "Results", level: 2 },
      ],
    });
    new Function(jumpToDocHeading(2))();
    expect(jump).toHaveBeenCalledOnce();
  } finally {
    vi.unstubAllGlobals();
    jump.mockRestore();
  }
});
