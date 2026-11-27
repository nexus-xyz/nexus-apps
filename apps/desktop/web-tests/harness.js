// Shared by every test page: a page calls check(name, fn) for each feature. fn returns true, false, or a promise of
// one; a throw is a failure. The page's title becomes "<name> <passed>/<total>", so the panel header shows the
// score in a screenshot, and the #score box lists each check. Pages inline this file (see build.py) so each stays
// one self-contained HTML file, as an artifact is.
(function () {
  const results = [];
  const title = document.title;
  function draw() {
    const passed = results.filter((r) => r.state === "PASS").length;
    const pending = results.some((r) => r.state === "...");
    document.title = `${title} ${passed}/${results.length}${pending ? " ..." : ""}`;
    const box = document.getElementById("score");
    if (!box) return;
    box.innerHTML = results
      .map((r) => `<span class="chk ${r.state === "PASS" ? "ok" : r.state === "..." ? "wait" : "bad"}">${r.state === "PASS" ? "✓" : r.state === "..." ? "…" : "✗"} ${r.name}</span>`)
      .join("");
  }
  window.check = function (name, fn) {
    const entry = { name, state: "..." };
    results.push(entry);
    const settle = (ok) => { entry.state = ok ? "PASS" : "FAIL"; draw(); };
    try {
      const value = fn();
      if (value && typeof value.then === "function") {
        Promise.race([value, new Promise((_, no) => setTimeout(() => no(new Error("timeout")), 8000))]).then(
          (ok) => settle(!!ok),
          () => settle(false),
        );
      } else settle(!!value);
    } catch (error) {
      settle(false);
    }
    draw();
  };
  window.supports = (name, declaration) => check(name, () => CSS.supports(declaration));
  window.loaded = (url) =>
    new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = url;
      script.onload = resolve;
      script.onerror = reject;
      document.head.appendChild(script);
    });
})();
