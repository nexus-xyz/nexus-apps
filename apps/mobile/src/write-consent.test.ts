// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createCoreClient, type AgentMessage, type AgentWriteResult } from "@nexus/kit";
import { AgentReply } from "./agent-reply";
import { ApprovalCard } from "./approval-card";
import type { PendingMessage } from "./assistant-cards";
import { consentPage, messageConsent, WRITE_CONSENT_FAILED, writeConsentLine, writeConsentTitle } from "./write-consent";

// The write consent prompt on the phone: a write Gateway refused with 403 `write_consent_required`.

const fixture = vi.hoisted(() => ({ startOAuth: vi.fn(), browse: vi.fn() }));
vi.mock("react-native", () => {
  const view = ({ children, accessibilityLabel }: { children?: ReactNode; accessibilityLabel?: string }) => createElement("div", { "aria-label": accessibilityLabel }, children);
  return {
    ActivityIndicator: () => null,
    Pressable: ({ children, onPress, disabled }: { children?: ReactNode; onPress?: () => void; disabled?: boolean }) => createElement("button", { onClick: onPress, disabled }, children),
    Text: view, View: view,
    Linking: { openURL: vi.fn(async () => undefined) },
    StyleSheet: { create: (styles: unknown) => styles, hairlineWidth: 1 },
  };
});
vi.mock("expo-web-browser", () => ({ openBrowserAsync: fixture.browse }));
vi.mock("@/auth", () => ({ useAuth: () => ({ org: "acme" }), useCore: () => ({ startOAuth: fixture.startOAuth }), useStored: () => ({ load: () => undefined, save: () => undefined }) }));
vi.mock("@/use-assistant-name", () => ({ useAssistantName: () => "Nexus" }));
vi.mock("@/use-storefront", () => ({ useWebBilling: () => false }));
vi.mock("@/markdown", () => ({ Markdown: ({ source }: { source: string }) => createElement("div", null, source), open: vi.fn() }));
vi.mock("@/messages", () => ({ messageStyles: {} }));
vi.mock("@/provider-mark", () => ({ ProviderMark: () => null }));
vi.mock("@/thread-cards", () => ({ ErrandCard: () => null, SessionCard: () => null }));
vi.mock("@/text-card", () => ({ TextCard: () => null }));
vi.mock("@/connect-card", () => ({ ConnectCard: () => null }));
vi.mock("@/controls", () => ({ TextInput: () => null }));
vi.mock("@/icon", () => ({ Icon: () => null }));
vi.mock("@/haptics", () => ({ haptic: vi.fn() }));
vi.mock("@/theme", () => ({ font: {}, mono: "mono", text: {} }));

// The consent the agent API keeps on the refused turn (nexus#23787), as Gateway's `error.nexus` names it.
const CONSENT = {
  connector: "github",
  account: "maya@acme.dev",
  scopes: ["repo"],
  consent: { method: "POST", path: "/v1/connections/github/oauth", body: { scope: "user", access: "write" } },
} as const;
const REFUSED = { kind: "refused", rule: "policy", detail: "GitHub was connected to read only. Allow writes, then ask again.", consent: CONSENT } as AgentMessage;
const CARD = `card_${"a".repeat(32)}`;
const PENDING: PendingMessage = {
  kind: "pending_action",
  card: CARD,
  write: { connector: "github", operation: "create_issue", summary: "Open an issue in acme/api", effect: "reversible", arguments: { title: "Bug" } },
  approveUrl: "https://nexus.inc/app/acme/approve/a1",
  detail: "Nothing runs until you approve it.",
  inChat: "approve-or-decline",
};
const theme = {} as never;

describe("the consent a refusal carries", () => {
  it("is read strictly from a refused message, and nothing else asks", () => {
    expect(messageConsent(REFUSED)).toEqual(CONSENT);
    expect(messageConsent({ kind: "refused", rule: "policy", detail: "No." })).toBeUndefined();
    // Only the call the contract names: another connector's OAuth start is not one.
    expect(messageConsent({ ...REFUSED, consent: { ...CONSENT, consent: { ...CONSENT.consent, path: "/v1/connections/slack/oauth" } } } as AgentMessage)).toBeUndefined();
    expect(messageConsent({ kind: "answer", text: "Done.", model: "m", costUsd: 0, consent: CONSENT } as AgentMessage)).toBeUndefined();
  });

  it("is worded as Nexus UI's prompt", () => {
    expect(writeConsentTitle(CONSENT)).toBe("GitHub needs your permission to write");
    expect(writeConsentLine(CONSENT)).toBe("maya@acme.dev was connected to read only. Continue to GitHub to allow writes, then ask again.");
  });

  it("asks Gateway for the write consent with the call it named, and gives the vendor's page", async () => {
    const calls: { url: string; method?: string; body?: unknown }[] = [];
    const core = createCoreClient({
      baseUrl: "https://gateway.test",
      fetch: async (input, init) => {
        calls.push({ url: String(input), method: init?.method, body: init?.body ? JSON.parse(String(init.body)) : undefined });
        return Response.json({ authorizeUrl: "https://github.com/login/oauth/authorize?state=s" });
      },
    });
    expect(await consentPage(core, { ...CONSENT, consent: { ...CONSENT.consent, body: { scope: "org", access: "write" } } })).toBe("https://github.com/login/oauth/authorize?state=s");
    expect(calls).toEqual([{ url: "https://gateway.test/v1/connections/github/oauth", method: "POST", body: { scope: "org", access: "write" } }]);
  });

  it("gives no page when the call fails or answers one the phone does not open", async () => {
    expect(await consentPage({ startOAuth: async () => Promise.reject(new Error("503")) }, CONSENT)).toBeUndefined();
    expect(await consentPage({ startOAuth: async () => ({ authorizeUrl: "javascript:alert(1)" }) }, CONSENT)).toBeUndefined();
  });
});

describe("the prompt in the thread", () => {
  let host: HTMLDivElement;
  let root: Root;
  beforeEach(() => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.clearAllMocks();
    fixture.startOAuth.mockResolvedValue({ authorizeUrl: "https://github.com/login/oauth/authorize?state=s" });
    fixture.browse.mockResolvedValue({ type: "dismiss" });
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
  });
  afterEach(async () => {
    await act(async () => root.unmount());
    host.remove();
    vi.unstubAllGlobals();
  });
  const press = async (label: string) => {
    const button = [...host.querySelectorAll("button")].find((b) => b.textContent === label);
    expect(button, label).toBeDefined();
    await act(async () => button!.click());
  };

  it("a refusal read back ends with it, and Continue opens the vendor's page without sending the write again", async () => {
    await act(async () => root.render(createElement(AgentReply, { message: REFUSED, name: (m: string) => m, theme })));
    expect(host.textContent).toContain(REFUSED.kind === "refused" ? REFUSED.detail : "");
    expect(host.textContent).toContain("GitHub needs your permission to write");
    expect(host.textContent).toContain("It asks for: repo");
    await press("Continue");
    expect(fixture.startOAuth).toHaveBeenCalledWith("github", "user", "write");
    expect(fixture.browse).toHaveBeenCalledWith("https://github.com/login/oauth/authorize?state=s");
  });

  it("says so when the page could not be opened, and Not now hides it", async () => {
    fixture.startOAuth.mockRejectedValueOnce(new Error("503"));
    await act(async () => root.render(createElement(AgentReply, { message: REFUSED, name: (m: string) => m, theme })));
    await press("Continue");
    expect(fixture.browse).not.toHaveBeenCalled();
    expect(host.textContent).toContain(WRITE_CONSENT_FAILED);
    await press("Not now");
    expect(host.textContent).not.toContain("needs your permission");
  });

  it("a refusal without consent shows no prompt", async () => {
    await act(async () => root.render(createElement(AgentReply, { message: { kind: "refused", rule: "policy", detail: "No." }, name: (m: string) => m, theme })));
    expect(host.textContent).not.toContain("needs your permission");
  });

  it("an approved write Gateway refused for consent ends the card with it", async () => {
    const decide = vi.fn(async (): Promise<AgentWriteResult> => ({
      kind: "decided",
      decided: { card: CARD, decision: "approve", outcome: "failed", text: "GitHub was connected to read only. Allow writes, then ask again.", consent: { ...CONSENT, scopes: [...CONSENT.scopes] } },
    }));
    await act(async () => root.render(createElement(ApprovalCard, { message: PENDING, theme, decide })));
    expect(host.textContent).not.toContain("needs your permission");
    await press("Approve");
    expect(host.textContent).toContain("GitHub needs your permission to write");
    await press("Continue");
    expect(fixture.startOAuth).toHaveBeenCalledWith("github", "user", "write");
    expect(decide).toHaveBeenCalledTimes(1);
  });
});
