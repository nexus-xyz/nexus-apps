// @vitest-environment happy-dom
import { expect, it, vi } from "vitest";
import { docCommentScript, readDocCommentAnchor } from "./doc-comment-anchor";

it("reads the second occurrence of a passage with the context needed to find it", () => {
  document.body.innerHTML = '<main class="page-doc"><p>Before the repeated word.</p><p>After the <em>repeated</em> word.</p></main>';
  const bridge = vi.fn<(message: string) => void>();
  vi.stubGlobal("ReactNativeWebView", { postMessage: bridge });
  try {
    new Function(docCommentScript)();
    const range = document.createRange();
    range.selectNodeContents(document.querySelector("em")!);
    window.getSelection()!.addRange(range);
    document.dispatchEvent(new Event("selectionchange"));
    const result = readDocCommentAnchor(JSON.parse(bridge.mock.lastCall![0]));
    expect(result).toMatchObject({
      quote: "repeated",
      prefix: "Before the repeated word.After the ",
      suffix: " word.",
    });
  } finally {
    vi.unstubAllGlobals();
  }
});

it("rejects an invalid selected passage", () => {
  expect(readDocCommentAnchor({ kind: "nexus.doc-comment-anchor", quote: "x".repeat(2001) })).toBeUndefined();
});
