import { describe, expect, it } from "vitest";
import { pageNavigation } from "./pages";

const FRAME = "https://p-abc.nexususercontent.dev/v/pag_1/3?sig=x&exp=1";

describe("pageNavigation", () => {
  it("loads the page's own sandbox origin", () => {
    expect(pageNavigation(FRAME, FRAME)).toBe("load");
    expect(pageNavigation(FRAME, "https://P-ABC.nexususercontent.dev/v/pag_1/3#part")).toBe("load");
  });

  it("opens another web origin in the browser", () => {
    expect(pageNavigation(FRAME, "https://example.com/")).toBe("browser");
    expect(pageNavigation(FRAME, "https://p-other.nexususercontent.dev/")).toBe("browser");
    expect(pageNavigation(FRAME, "https://p-abc.nexususercontent.dev@evil.example/")).toBe("browser");
    expect(pageNavigation(FRAME, "http://p-abc.nexususercontent.dev/")).toBe("browser");
  });

  it("drops every other scheme, the app's own included", () => {
    for (const url of ["nexus://session/new", "file:///etc/hosts", "javascript:alert(1)", "data:text/html,x", "tel:123", "about:blank"]) {
      expect(pageNavigation(FRAME, url)).toBe("drop");
    }
  });
});
