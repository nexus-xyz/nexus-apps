// @vitest-environment happy-dom
import { createCoreClient, type AgentSessionSummary } from "@nexus/kit";
import { act, createContext, createElement, Fragment, useContext, useEffect, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import AgentSessions from "./app/(app)/(main)/agent-sessions";
import AgentSession from "./app/(app)/(main)/agent-session/[id]";

const state = vi.hoisted(() => ({ id: "teammate" }));
const Focused = createContext(true);
const core = createCoreClient({ baseUrl: "https://gateway.test" });
const stores = new Map<string, { load: () => unknown; save: (value: unknown) => void }>();
vi.mock("@/auth", () => ({
  useCore: () => core,
  useStored: (_kind: string, name: string) => {
    if (!stores.has(name)) {
      let value: unknown;
      stores.set(name, { load: () => value, save: (next) => { value = next; } });
    }
    return stores.get(name);
  },
}));
vi.mock("expo-router", () => ({
  useFocusEffect: (effect: () => (() => void) | void) => {
    const focused = useContext(Focused);
    useEffect(() => focused ? effect() : undefined, [effect, focused]);
  },
  useLocalSearchParams: () => state,
  Stack: { Screen: () => null },
  router: { push: vi.fn() },
}));
vi.mock("react-native", () => {
  const view = ({ children, accessibilityLabel }: { children?: ReactNode; accessibilityLabel?: string }) => createElement("div", { "aria-label": accessibilityLabel }, children);
  return {
    Text: view, View: view, ScrollView: view,
    Pressable: ({ children, onPress, accessibilityLabel }: { children?: ReactNode; onPress?: () => void; accessibilityLabel?: string }) =>
      createElement("button", { onClick: onPress, "aria-label": accessibilityLabel }, children),
    RefreshControl: () => null,
    Platform: { OS: "android" },
    StyleSheet: { create: (styles: unknown) => styles, hairlineWidth: 1 },
    useWindowDimensions: () => ({ fontScale: 1 }),
  };
});
vi.mock("@/segmented", () => ({ Segmented: ({ options, onChange }: { options: { id: string; label: string }[]; onChange: (id: string) => void }) =>
  createElement("div", null, options.map((option) => createElement("button", { key: option.id, onClick: () => onChange(option.id) }, option.label))) }));
vi.mock("@/theme", () => ({ font: {}, mono: "mono", text: {}, useTheme: () => ({}) }));
vi.mock("@/controls", () => ({ TextInput: () => null }));
vi.mock("@/haptics", () => ({ haptic: vi.fn() }));
vi.mock("@/icon", () => ({ Icon: () => null }));
vi.mock("@/skeleton", () => ({ SkeletonRows: () => null }));
vi.mock("@/status-mark", () => ({ StatusMark: () => null }));
vi.mock("@/models", () => ({ useModels: () => [], nameOf: (_models: unknown, id: string) => id }));
vi.mock("@/use-now", () => ({ useNow: () => Date.parse("2026-11-20T15:15:00Z") }));
vi.mock("@/command", () => ({ useCopy: () => [null, vi.fn()] }));

const run = (id: string, person: string, status: AgentSessionSummary["status"] = "running"): AgentSessionSummary => ({
  id, person, status, agent: "claude", via: "subscription", model: "claude-opus-5-5", cwd: "project", startedAt: "2026-11-20T15:00:00Z",
});
let sessions: AgentSessionSummary[];
let details: Map<string, AgentSessionSummary>;
let host: HTMLDivElement;
let root: Root;
const render = (screen = AgentSessions) => act(async () => root.render(createElement(screen)));
const renderStack = (screen: "list" | "detail") => act(async () => root.render(
  createElement(Fragment, null,
    createElement(Focused.Provider, { value: screen === "list" },
      createElement("div", { "data-screen": "list", hidden: screen !== "list" }, createElement(AgentSessions))),
    screen === "detail" ? createElement("div", { "data-screen": "detail" }, createElement(AgentSession)) : null,
  ),
));

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  stores.clear();
  sessions = [run("mine", "ari@example.test"), run("teammate", "maya@example.test"), run("finished", "lee@example.test", "succeeded")];
  details = new Map([...sessions, run("older", "jules@example.test", "succeeded")].map((session) => [session.id, session]));
  state.id = "teammate";
  vi.stubGlobal("fetch", vi.fn<typeof fetch>(async (input) => {
    const path = new URL(String(input)).pathname;
    if (path === "/v1/agents/sessions") return Response.json({ sessions });
    if (path.startsWith("/v1/agents/sessions/")) {
      const detail = details.get(decodeURIComponent(path.slice("/v1/agents/sessions/".length)));
      return detail ? Response.json({ ...detail, spend: { usd: 0, calls: 0 } }) : Response.json({ error: { message: "No readable agent session", code: "not_found" } }, { status: 404 });
    }
    throw new Error(`Unexpected request: ${path}`);
  }));
  host = document.createElement("div");
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("identifies who started each running and finished agent", async () => {
  await render();
  expect(host.textContent).toContain("Started by ari@example.test");
  expect(host.textContent).toContain("Started by maya@example.test");
  expect(host.querySelector('[aria-label*="Started by maya@example.test"]')).not.toBeNull();
  await act(async () => [...host.querySelectorAll("button")].find((button) => button.textContent === "Done · 1")?.click());
  expect(host.textContent).toContain("Started by lee@example.test");
});

it("names the owner when opening an agent directly", async () => {
  await render(AgentSession);
  expect(host.querySelector('[aria-label="Started by, maya@example.test"]')).not.toBeNull();
});

it("opens a readable run outside the recent list and identifies its person", async () => {
  state.id = "older";
  await render(AgentSession);
  expect(host.querySelector('[aria-label="Started by, jules@example.test"]')).not.toBeNull();
  expect(host.textContent).not.toContain("Loading…");
});

it.each([401, 403, 404])("keeps a denied run hidden after %s when Back refocuses the mounted list during an outage", async (status) => {
  await renderStack("list");
  const list = host.querySelector('[data-screen="list"]');
  expect(list?.textContent).toContain("Started by maya@example.test");
  vi.mocked(fetch).mockResolvedValueOnce(Response.json({ error: { message: "No readable agent session", code: "not_found" } }, { status }));
  await renderStack("detail");
  const detail = host.querySelector('[data-screen="detail"]');
  expect(detail?.textContent).toContain("Couldn't load this session");
  expect(detail?.textContent).toContain("No readable agent session");
  expect(detail?.textContent).not.toContain("maya@example.test");

  vi.mocked(fetch).mockResolvedValueOnce(Response.json({ error: { message: "Gateway temporarily unavailable" } }, { status: 503 }));
  await renderStack("list");
  expect(host.querySelector('[data-screen="list"]')).toBe(list);
  expect(list?.textContent).toContain("Started by ari@example.test");
  expect(list?.textContent).toContain("Gateway temporarily unavailable");
  expect(list?.textContent).not.toContain("maya@example.test");

  vi.mocked(fetch).mockResolvedValueOnce(Response.json({ error: { message: "Gateway temporarily unavailable" } }, { status: 503 }));
  await renderStack("detail");
  const reopened = host.querySelector('[data-screen="detail"]');
  expect(reopened?.textContent).toContain("Couldn't load this session");
  expect(reopened?.textContent).toContain("Gateway temporarily unavailable");
  expect(reopened?.textContent).not.toContain("maya@example.test");

  await render(AgentSession);
  expect(host.querySelector('[aria-label="Started by, maya@example.test"]')).not.toBeNull();
});

it.each(["while blurred", "after Back"])("ignores an earlier list refresh that completes %s after a denial", async (timing) => {
  vi.useFakeTimers();
  await renderStack("list");
  const earlier = Promise.withResolvers<Response>();
  vi.mocked(fetch).mockImplementationOnce(() => earlier.promise);
  await act(async () => vi.advanceTimersByTime(10_000));

  vi.mocked(fetch).mockResolvedValueOnce(Response.json({ error: { message: "No readable agent session", code: "not_found" } }, { status: 404 }));
  await renderStack("detail");
  const complete = () => act(async () => earlier.resolve(Response.json({ sessions })));
  if (timing === "while blurred") await complete();
  vi.mocked(fetch).mockResolvedValueOnce(Response.json({ error: { message: "Gateway temporarily unavailable" } }, { status: 503 }));
  await renderStack("list");
  if (timing === "after Back") await complete();

  const list = host.querySelector('[data-screen="list"]');
  expect(list?.textContent).toContain("Started by ari@example.test");
  expect(list?.textContent).not.toContain("maya@example.test");

  vi.mocked(fetch).mockResolvedValueOnce(Response.json({ error: { message: "Gateway temporarily unavailable" } }, { status: 503 }));
  await render(AgentSession);
  expect(host.textContent).toContain("Couldn't load this session");
  expect(host.textContent).not.toContain("maya@example.test");
});

it.each([429, 503])("keeps matching cached detail with an error footnote after a transient %s", async (status) => {
  stores.set("agent-sessions", { load: () => sessions, save: vi.fn() });
  vi.mocked(fetch).mockResolvedValueOnce(Response.json({ error: { message: "Gateway temporarily unavailable" } }, { status }));
  await render(AgentSession);
  expect(host.querySelector('[aria-label="Started by, maya@example.test"]')).not.toBeNull();
  expect(host.textContent).toContain("Gateway temporarily unavailable");
  expect(host.textContent).not.toContain("Couldn't load this session");
});

it("shows a terminal error for a transient failure when no matching cached run exists", async () => {
  stores.set("agent-sessions", { load: () => sessions, save: vi.fn() });
  state.id = "older";
  vi.mocked(fetch).mockResolvedValueOnce(Response.json({ error: { message: "Gateway temporarily unavailable" } }, { status: 503 }));
  await render(AgentSession);
  expect(host.textContent).toContain("Couldn't load this session");
  expect(host.textContent).toContain("Gateway temporarily unavailable");
  expect(host.textContent).not.toContain("Loading…");
  expect(host.textContent).not.toContain("maya@example.test");
});

it("keeps the selected run when an earlier lookup completes late", async () => {
  const earlier = Promise.withResolvers<Response>();
  let signal: AbortSignal | null | undefined;
  vi.mocked(fetch).mockImplementationOnce(async (_input, init) => {
    signal = init?.signal;
    return earlier.promise;
  });
  await render(AgentSession);
  state.id = "older";
  await render(AgentSession);
  expect(signal?.aborted).toBe(true);
  await act(async () => earlier.resolve(Response.json(run("teammate", "maya@example.test"))));
  expect(host.querySelector('[aria-label="Started by, jules@example.test"]')).not.toBeNull();
  expect(host.textContent).not.toContain("maya@example.test");
});
