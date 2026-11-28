export type DeckInfo = { visibleSlides: number; hiddenSlides: number };

export function readDeckInfo(data: unknown): DeckInfo | undefined {
  if (!data || typeof data !== "object" || !("kind" in data) || data.kind !== "nexus.deck-info") return;
  if (!("visibleSlides" in data) || !("hiddenSlides" in data)) return;
  const { visibleSlides, hiddenSlides } = data;
  if (typeof visibleSlides !== "number" || typeof hiddenSlides !== "number" ||
      !Number.isSafeInteger(visibleSlides) || !Number.isSafeInteger(hiddenSlides) ||
      visibleSlides < 0 || hiddenSlides < 0 || visibleSlides + hiddenSlides === 0) return;
  return { visibleSlides, hiddenSlides };
}

export const deckInfoScript = `(() => {
  if (window.nexusMobileDeckInfo) return;
  window.nexusMobileDeckInfo = true;
  let previous = "";
  function report() {
    const slides = Array.from(document.querySelectorAll("section[data-slide]"))
      .filter((slide) => !slide.parentElement?.closest("section[data-slide]"));
    if (!slides.length || document.querySelector("[data-artboard]")) return;
    const hiddenSlides = slides.filter((slide) => slide.matches("[hidden], [data-slide-hidden]")).length;
    const data = JSON.stringify({ kind: "nexus.deck-info", visibleSlides: slides.length - hiddenSlides, hiddenSlides });
    if (previous === data) return;
    previous = data;
    window.ReactNativeWebView.postMessage(data);
  }
  report();
  let update;
  new MutationObserver(() => {
    clearTimeout(update);
    update = setTimeout(report, 75);
  }).observe(document.body, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ["data-slide", "data-artboard", "hidden", "data-slide-hidden"],
  });
})(); true;`;
