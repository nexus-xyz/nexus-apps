import { describe, expect, it } from "vitest";
import { DEFAULT_GATEWAY, movedSignIn, uiOrigin, uiPage } from "./links";

// The hosts a build names (app.config.ts); app-config.test.ts checks Nexus's own.
const hosts = { legacyUi: "ui.legacy.example.test", legacyGateway: "gateway.legacy.example.test", devGateway: "gateway.dev.nexus.xyz" };

describe("links to Nexus UI", () => {
  it("finds Nexus UI from the approval page, else the Gateway", () => {
    expect(uiOrigin("https://gateway.example.test", "https://nexus.xyz/app/device?code=BCDF")).toBe("https://nexus.xyz");
    expect(uiOrigin("https://gateway.example.test")).toBe("https://gateway.example.test");
  });
});

describe("a sign-in saved on a retired Gateway host", () => {
  it("moves to production, keeping its Nexus UI", () => {
    expect(movedSignIn({ gateway: "https://gateway.legacy.example.test", token: "t" }, hosts)).toEqual({
      gateway: DEFAULT_GATEWAY,
      token: "t",
      ui: "https://ui.legacy.example.test",
    });
    expect(movedSignIn({ gateway: "https://api.nexus.xyz", token: "t", ui: "https://nexus.xyz" }, hosts)).toEqual({
      gateway: DEFAULT_GATEWAY,
      token: "t",
      ui: "https://nexus.xyz",
    });
  });

  it("stays on the dev Gateway and on any other host", () => {
    expect(movedSignIn({ gateway: "https://gateway.dev.nexus.xyz", token: "t" }, hosts)).toBeUndefined();
    expect(movedSignIn({ gateway: "https://nexus.acme.test", token: "t" }, hosts)).toBeUndefined();
    expect(movedSignIn({ gateway: "http://192.168.1.20:4621", token: "t" }, hosts)).toBeUndefined();
  });

  it("moves only `*.nexus.xyz` names when the build names no hosts", () => {
    expect(movedSignIn({ gateway: "https://gateway.legacy.example.test", token: "t" }, {})).toBeUndefined();
    expect(movedSignIn({ gateway: "https://gateway.dev.nexus.xyz", token: "t" }, {})).toEqual({ gateway: DEFAULT_GATEWAY, token: "t", ui: undefined });
  });
});
