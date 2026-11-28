// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentConversationSummary } from "@nexus/kit";
import { HISTORY_UNAVAILABLE, type History } from "./agent-conversations";
import { ConversationsSheet } from "./conversations-sheet";

// The sheet of conversations in light and dark, with the app's own theme: each row's title, preview and line in the
// theme's greys, the one on screen checked, the 503 line, and Load more.

const scheme = vi.hoisted(() => ({ value: "light" as "light" | "dark" }));
type Style = Record<string, unknown> | false | null | undefined | Style[];
const flat = (style: Style): Record<string, unknown> => (Array.isArray(style) ? Object.assign({}, ...style.map(flat)) : style || {});

vi.mock("react-native", () => {
  const view = ({ children }: { children?: ReactNode }) => createElement("div", null, children);
  return {
    ActivityIndicator: ({ accessibilityLabel }: { accessibilityLabel?: string }) => createElement("progress", { "aria-label": accessibilityLabel }),
    Appearance: { setColorScheme: vi.fn() },
    Platform: { OS: "ios" },
    Pressable: ({ children, onPress, style, accessibilityLabel }: { children?: ReactNode; onPress?: () => void; style?: Style | ((state: { pressed: boolean }) => Style); accessibilityLabel?: string }) =>
      createElement("button", { onClick: onPress, "aria-label": accessibilityLabel, "data-fill": flat(typeof style === "function" ? style({ pressed: false }) : style).backgroundColor }, children),
    ScrollView: view,
    View: view,
    Text: ({ children, style }: { children?: ReactNode; style?: Style }) => createElement("span", { "data-color": flat(style).color }, children),
    StyleSheet: { create: (styles: unknown) => styles, hairlineWidth: 1 },
    useColorScheme: () => scheme.value,
    useWindowDimensions: () => ({ width: 390, height: 844, fontScale: 1 }),
  };
});
vi.mock("./store", () => ({ load: () => undefined, save: vi.fn(), remove: vi.fn() }));
vi.mock("@/sheet", () => ({ Sheet: ({ visible, children }: { visible: boolean; children: ReactNode }) => (visible ? createElement("div", { role: "dialog" }, children) : null) }));
vi.mock("@/icon", () => ({ Icon: ({ name }: { name: string }) => createElement("i", { "data-icon": name }) }));

const HOUR = 3_600_000;
const row = (id: string, more: Partial<AgentConversationSummary> = {}): AgentConversationSummary => ({
  id,
  title: `Title ${id}`,
  surface: "slack",
  updatedAt: new Date(Date.now() - 2 * HOUR).toISOString(),
  turns: 4,
  preview: `Preview ${id}`,
  ...more,
});

let host: HTMLDivElement;
let root: Root;
const props = { visible: true, onClose: vi.fn(), reading: false, onOpen: vi.fn(), onMore: vi.fn() };
const render = (history: History, more: Partial<typeof props> & { current?: string } = {}) =>
  act(async () => root.render(createElement(ConversationsSheet, { ...props, history, ...more })));
const span = (words: string) => [...host.querySelectorAll("span")].find((node) => node.textContent === words);

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  props.onOpen.mockClear();
  props.onMore.mockClear();
  host = document.createElement("div");
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.unstubAllGlobals();
});

describe.each([
  ["light", { foreground: "#1c1c1e", muted: "#5c5c60", tertiary: "#6e6e73", rowPressed: "#e6e6e8" }],
  ["dark", { foreground: "#ececec", muted: "#a1a1a6", tertiary: "#8a8a8e", rowPressed: "#2a2a2c" }],
] as const)("in %s", (mode, colors) => {
  beforeEach(() => {
    scheme.value = mode;
  });

  it("draws each row's title, preview and line in the theme's greys, and checks the one on screen", async () => {
    await render({ kind: "on", rows: [row("a"), row("b", { surface: "cli" })], next: null }, { current: "b" });
    expect(span("Title a")?.dataset.color).toBe(colors.foreground);
    expect(span("Preview a")?.dataset.color).toBe(colors.muted);
    expect(span("Slack · 2h")?.dataset.color).toBe(colors.tertiary);
    expect(span("Terminal · 2h")).toBeDefined();
    const rows = [...host.querySelectorAll<HTMLButtonElement>("button[aria-label^='Title']")];
    expect(rows.map((button) => button.getAttribute("aria-label"))).toEqual(["Title a, from Slack, 2 hours ago, Preview a", "Title b, from Terminal, 2 hours ago, Preview b"]);
    expect(rows[1]!.dataset.fill).toBe(colors.rowPressed);
    expect(rows[1]!.querySelector("[data-icon='check']")).not.toBeNull();
    expect(rows[0]!.querySelector("[data-icon='check']")).toBeNull();
  });

  it("shows one line when the history cannot be read", async () => {
    await render({ kind: "unavailable" });
    expect(span(HISTORY_UNAVAILABLE)?.dataset.color).toBe(colors.muted);
    expect(host.querySelector("button[aria-label^='Title']")).toBeNull();
  });
});

it("opens a row, and loads the next page on Load more", async () => {
  const a = row("a");
  await render({ kind: "on", rows: [a], next: "cur" });
  await act(async () => host.querySelector<HTMLButtonElement>("button[aria-label^='Title a']")!.click());
  expect(props.onOpen).toHaveBeenCalledWith(a);
  await act(async () => host.querySelector<HTMLButtonElement>("[aria-label='Load more conversations']")!.click());
  expect(props.onMore).toHaveBeenCalledOnce();
  // While a page loads, a spinner stands in for Load more; on the last page there is neither.
  await render({ kind: "on", rows: [row("a")], next: "cur" }, { reading: true });
  expect(host.querySelector("[aria-label='Load more conversations']")).toBeNull();
  expect(host.querySelector("progress")).not.toBeNull();
  await render({ kind: "on", rows: [row("a")], next: null });
  expect(host.querySelector("[aria-label='Load more conversations']")).toBeNull();
});

it("keeps the rows with the line when a next page fails, and says when there are none yet", async () => {
  await render({ kind: "on", rows: [row("a")], next: "cur", failed: true });
  expect(span("Title a")).toBeDefined();
  expect(span(HISTORY_UNAVAILABLE)).toBeDefined();
  await render({ kind: "on", rows: [], next: null });
  expect(host.textContent).toContain("No conversations yet.");
});
