// @vitest-environment happy-dom
import { act, createElement, type FormEvent, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentAsk, AgentClient, AgentConversation, AgentConversationList, AgentIntroRead } from "@nexus/kit";
import { HISTORY_UNAVAILABLE } from "./agent-conversations";
import { clock, OFFLINE_LINE, PROBE_MS } from "./assistant-thread";
import YourAgent from "./app/(app)/(drawer)/(chat)/agent";

const state = vi.hoisted(() => ({ org: "a", agent: undefined as AgentClient | undefined, push: vi.fn() }));
const seenSaves = new Map<string, unknown>();
const stores = new Map<string, { load: () => string | undefined; save: (id: string) => void; remove: () => void }>();
const consent = vi.hoisted(() => vi.fn(async () => true));
const announce = vi.hoisted(() => vi.fn());
// Search's hand-off: the screen's `ask` parameter.
const params = vi.hoisted(() => ({ ask: undefined as string | undefined }));
const setParams = vi.hoisted(() => vi.fn());
// What voice mode asks with: the screen's `send`, as `useVoiceMode` gets it.
const voice = vi.hoisted(() => ({ ask: undefined as ((words: string) => Promise<unknown>) | undefined }));

// The page's bar draws its title and actions in the screen, so a test can read and press them.
vi.mock("expo-router", () => ({ router: { push: state.push, setParams }, useLocalSearchParams: () => params, useNavigation: () => ({ dispatch: vi.fn() }) }));
vi.mock("expo-router/react-navigation", () => ({ DrawerActions: { openDrawer: () => ({}) } }));
vi.mock("@/top-bar", () => ({
  BAR_HEIGHT: 56,
  SidebarButton: () => null,
  TopBar: ({ center, right }: { center?: ReactNode; right?: ReactNode }) => createElement("header", null, center, right),
}));
vi.mock("@/glass", () => ({
  Glass: ({ children }: { children?: ReactNode }) => createElement("div", null, children),
  GlassButton: ({ label, onPress, children }: { label: string; onPress: () => void; children?: ReactNode }) => createElement("button", { onClick: onPress, "aria-label": label }, children),
  useGlass: () => false,
}));
vi.mock("@/text-size", () => ({ CHROME_FONT_SCALE: 1.3 }));
// The composer's glass, motion and haptics draw nothing here: the screen's logic is under test, not its look.
vi.mock("react-native-reanimated", () => ({
  default: { View: ({ children }: { children?: ReactNode }) => createElement("div", null, children) },
  LayoutAnimationConfig: ({ children }: { children?: ReactNode }) => children,
  useReducedMotion: () => true,
  useSharedValue: (value: number) => ({ value }),
  useAnimatedStyle: () => ({}),
  cancelAnimation: () => undefined,
  withRepeat: () => 0,
  withSpring: () => 0,
  withTiming: () => 0,
  Easing: { linear: () => 0, bezier: () => () => 0 },
  ReduceMotion: { System: "system", Never: "never" },
}));
// Sounds and the menu: what the screen calls and shows, not how they play or draw. The menu lists its rows while open,
// and a row does what the real one does (`pickRow`).
const chimes = vi.hoisted(() => vi.fn());
vi.mock("@/assistant-sounds", () => ({ chime: chimes }));
vi.mock("@/assistant-menu-view", async () => {
  const { pickRow, rowDisabled } = await vi.importActual<typeof import("./assistant-menu")>("./assistant-menu");
  type Props = { visible: boolean; rows: import("./assistant-menu").MenuRow[][]; busy: boolean; onClose: () => void; onAction: (action: import("./assistant-menu").MenuAction) => void };
  return {
    AssistantMenu: ({ visible, rows, busy, onClose, onAction }: Props) =>
      visible
        ? createElement(
            "div",
            { role: "dialog" },
            rows.flat().map((row) =>
              createElement("button", {
                key: row.label,
                "aria-label": row.label,
                disabled: rowDisabled(row, busy),
                onClick: () => {
                  onClose();
                  pickRow(row, onAction, state.push);
                },
              }, row.label),
            ),
          )
        : null,
  };
});
vi.mock("@/conversation-composer", () => ({ Composer: ({ children, footer }: { children?: ReactNode; footer?: ReactNode }) => createElement("div", null, children, footer) }));
vi.mock("@/thinking", () => ({ swapIn: undefined }));
vi.mock("@/haptics", () => ({ haptic: vi.fn() }));
vi.mock("@/press-scale", () => ({ PressScale: ({ children, onPress }: { children?: ReactNode; onPress?: () => void }) => createElement("button", { onClick: onPress }, children) }));
vi.mock("react-native", () => {
  const view = ({ children, accessibilityLabel }: { children?: ReactNode; accessibilityLabel?: string }) => createElement("div", { "aria-label": accessibilityLabel }, children);
  return {
    ActivityIndicator: () => null,
    AccessibilityInfo: { announceForAccessibility: announce },
    AppState: { addEventListener: () => ({ remove: () => undefined }) },
    Platform: { OS: "ios" },
    Pressable: ({ children, onPress, disabled, accessibilityLabel }: { children?: ReactNode; onPress?: () => void; disabled?: boolean; accessibilityLabel?: string }) =>
      createElement("button", { onClick: onPress, disabled, "aria-label": accessibilityLabel }, children),
    ScrollView: view, Text: view, View: view,
    StyleSheet: { create: (styles: unknown) => styles, hairlineWidth: 1 },
    useWindowDimensions: () => ({ width: 390, height: 844, fontScale: 1 }),
  };
});
vi.mock("react-native-keyboard-controller", () => ({
  KeyboardAvoidingView: ({ children }: { children: ReactNode }) => createElement("div", null, children),
  KeyboardController: { isVisible: () => false },
  KeyboardEvents: { addListener: () => ({ remove: () => {} }) },
}));
vi.mock("expo-router/drawer", () => ({ useDrawerStatus: () => "closed" }));
vi.mock("react-native-safe-area-context", () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
vi.mock("@/assistant-mark", () => ({ AssistantMark: ({ thinking }: { thinking?: boolean }) => createElement("i", { "data-thinking": String(!!thinking) }) }));
vi.mock("@/agent-reply", () => ({ AgentReply: ({ message }: { message: { text?: string } }) => createElement("div", null, message.text) }));
vi.mock("@/ai-consent", () => ({ agreeTo: consent, providersFor: () => [], providerName: (id: string) => id }));
vi.mock("@/auth", () => ({
  useAuth: () => ({ agent: state.agent, org: state.org, name: "Maya Chen", orgName: "Personal" }),
  useCore: () => ({}),
  // The kept conversation by account; what the screen showed for the drawer (`assistant-seen`) apart.
  useStored: (_kind: string, name: string) => (name === "agent-conversation" ? stores.get(state.org) : { load: () => seenSaves.get(state.org), save: (seen: unknown) => seenSaves.set(state.org, seen), remove: () => seenSaves.delete(state.org) }),
}));
vi.mock("@/sheet", () => ({ Sheet: ({ visible, children }: { visible: boolean; children: ReactNode }) => (visible ? createElement("div", { role: "dialog" }, children) : null) }));
vi.mock("@/consent-sheet", () => ({ keptConsent: {}, ConsentSheet: () => null }));
vi.mock("@/controls", () => ({ TextInput: ({ value, onChangeText, placeholder }: { value: string; onChangeText: (value: string) => void; placeholder?: string }) =>
  createElement("input", { value, placeholder, onInput: (event: FormEvent<HTMLInputElement>) => onChangeText(event.currentTarget.value) }) }));
vi.mock("@/icon", () => ({ Icon: () => null }));
// The errands strip's menu (`errand-menu.tsx`): a toast and the row menus, which draw nothing here.
vi.mock("@/toast", () => ({ useToast: () => () => undefined }));
vi.mock("@/row-menu", () => ({ RowMenu: ({ children }: { children: (onLongPress: undefined) => ReactNode }) => children(undefined) }));
vi.mock("@/nexus-mark", () => ({ NexusMark: () => null }));
vi.mock("@/list", () => ({ Blank: () => null }));
vi.mock("@/markdown", () => ({ Markdown: ({ source }: { source: string }) => createElement("div", null, source) }));
vi.mock("@/messages", () => ({ messageStyles: {}, Question: ({ content }: { content: string }) => createElement("div", null, content) }));
vi.mock("@/meta", () => ({ AUTO: "nexus/auto" }));
vi.mock("@/use-share-inbox", () => ({ useSharedHandoff: () => undefined }));
// The paused bar reads the agent API on its own (`paused-bar.tsx`); it draws nothing here.
vi.mock("@/paused-bar", () => ({ PausedBar: () => null }));
vi.mock("@/models", () => ({ nameOf: (_: unknown, model: string) => model, useModels: () => [] }));
vi.mock("@/theme", () => ({ font: {}, mono: "mono", text: {}, useTheme: () => ({}) }));
vi.mock("@/voice", () => ({ spokenReply: (messages: { text?: string }[]) => ({ say: messages.map((m) => m.text).join(" ") }), UNANSWERED: { say: "unanswered" } }));
vi.mock("@/voice-mode", () => ({
  useVoiceMode: (ask: (words: string) => Promise<unknown>) => {
    voice.ask = ask;
    return { open: false };
  },
  voiceAvailable: false,
  VoiceModeView: () => null,
}));

let host: HTMLDivElement;
let root: Root;
const turns: AgentConversation = { kind: "turns", turns: [{ role: "user", text: "Old question", at: "2026-11-09T00:00:00Z" }, { role: "assistant", text: "Old answer", at: "2026-11-09T00:00:01Z" }] };

function store(id?: string) {
  let value = id;
  return { load: () => value, save: vi.fn((next: string) => { value = next; }), remove: () => { value = undefined; } };
}
// The agent API's list is off (a 404) unless a test says otherwise, so the screen works as before.
function client(list: AgentConversationList = { kind: "off" }, intro: AgentIntroRead = { kind: "off" }) {
  return {
    intro: vi.fn<AgentClient["intro"]>().mockResolvedValue(intro),
    introSeen: vi.fn<AgentClient["introSeen"]>().mockResolvedValue(true),
    brief: vi.fn<AgentClient["brief"]>().mockResolvedValue({ kind: "off" }),
    conversation: vi.fn<AgentClient["conversation"]>().mockResolvedValue(turns),
    ask: vi.fn<AgentClient["ask"]>().mockResolvedValue({ kind: "reply", reply: { status: 200, conversation: "new", messages: [{ kind: "answer", text: "New answer", model: "m", costUsd: 0 }] } }),
    conversations: vi.fn<AgentClient["conversations"]>().mockResolvedValue(list),
  };
}
const press = (label: string) => act(async () => host.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!.click());
const render = () => act(async () => root.render(createElement(YourAgent)));
async function type(words: string) {
  await act(async () => {
    const input = host.querySelector("input")!;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(input, words);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}
const send = () => act(async () => host.querySelector<HTMLButtonElement>('[aria-label="Send"]')!.click());
/** Picks a row of the bar's menu. */
async function fromMenu(label: string) {
  await press("Nexus menu");
  await press(label);
}

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  consent.mockReset().mockResolvedValue(true);
  announce.mockClear();
  state.org = "a";
  params.ask = undefined;
  setParams.mockClear();
  stores.clear();
  seenSaves.clear();
  stores.set("a", store("old"));
  stores.set("b", store());
  host = document.createElement("div");
  root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); vi.unstubAllGlobals(); });

it("clears the old organization and aborts its answer before asking in the new one", async () => {
  const old = client();
  const answer = Promise.withResolvers<AgentAsk>();
  old.ask.mockReturnValue(answer.promise);
  state.agent = old;
  await render();
  await type("A private follow-up");
  await send();
  const request = old.ask.mock.calls[0]![1]!;
  const next = client();
  state.org = "b";
  state.agent = next;
  await render();
  expect(host.textContent).not.toContain("Old answer");
  expect(host.textContent).not.toContain("A private follow-up");
  expect(request.signal?.aborted).toBe(true);
  await act(async () => {
    request.onEvent?.({ type: "started", conversation: "late-old" });
    answer.resolve({ kind: "reply", reply: { status: 200, conversation: "late-old", messages: [{ kind: "answer", text: "Late old answer", model: "m", costUsd: 0 }] } });
  });
  expect(stores.get("a")!.save).not.toHaveBeenCalled();
  expect(host.textContent).not.toContain("Late old answer");
  await type("New organization's question");
  await send();
  expect(next.ask.mock.calls[0]?.[1]?.conversation).toBeUndefined();
});

it("does not send after organization switching while provider consent is pending", async () => {
  const allowed = Promise.withResolvers<boolean>();
  consent.mockReturnValueOnce(allowed.promise);
  const old = client();
  state.agent = old;
  await render();
  await type("A private question");
  await send();
  state.org = "b";
  state.agent = client();
  await render();
  await act(async () => allowed.resolve(true));
  expect(old.ask).not.toHaveBeenCalled();
  expect(host.textContent).not.toContain("A private question");
});

it("waits for history before allowing a follow-up, then keeps both exchanges", async () => {
  const history = Promise.withResolvers<AgentConversation>();
  const agent = client();
  agent.conversation.mockReturnValue(history.promise);
  state.agent = agent;
  await render();
  await type("New question");
  // Send shows only once the message can go.
  expect(host.querySelector('[aria-label="Send"]')).toBeNull();
  expect(agent.ask).not.toHaveBeenCalled();
  await act(async () => history.resolve(turns));
  await send();
  expect(agent.ask.mock.calls[0]?.[1]?.conversation).toBe("old");
  expect(host.textContent).toContain("Old answer");
  expect(host.textContent).toContain("New question");
  expect(host.textContent).toContain("New answer");
});

const slack = { id: "c_slack", title: "Deploy plan", surface: "slack" as const, updatedAt: new Date(Date.now() - 7_200_000).toISOString(), turns: 2, preview: "Ship it Friday." };
const slackTurns: AgentConversation = { kind: "turns", turns: [{ role: "user", text: "Plan the deploy", at: "2026-11-09T00:00:00Z", surface: "slack" }, { role: "assistant", text: "Ship it Friday.", at: "2026-11-09T00:00:01Z" }] };

it("hides History when the agent API keeps no shared conversations, and asks as the phone in the kept one", async () => {
  const agent = client();
  state.agent = agent;
  await render();
  expect(agent.conversations).toHaveBeenCalledOnce();
  await press("Nexus menu");
  expect(host.querySelector('[aria-label="History"]')).toBeNull();
  expect(host.querySelector('[aria-label="New conversation"]')).not.toBeNull();
  await type("A follow-up");
  await send();
  expect(agent.ask.mock.calls[0]?.[1]).toMatchObject({ conversation: "old", surface: "mobile" });
  expect(host.textContent).toContain("New answer");
});

it("says in one line that the history could not be read on a 503, and the chat still works", async () => {
  const agent = client({ kind: "failed", failure: { status: 503, error: "conversations_unavailable" } });
  state.agent = agent;
  await render();
  await fromMenu("History");
  expect(agent.conversations).toHaveBeenCalledTimes(2);
  expect(host.textContent).toContain(HISTORY_UNAVAILABLE);
  await act(async () => [...host.querySelectorAll("button")].find((button) => button.textContent === "Done")!.click());
  expect(host.textContent).not.toContain(HISTORY_UNAVAILABLE);
  await type("Still there?");
  await send();
  expect(host.textContent).toContain("New answer");
});

it("opens a Slack conversation from History, loads more with the cursor, and the next ask continues it", async () => {
  const agent = client({ kind: "page", conversations: [slack], next: "cur" });
  agent.conversation.mockImplementation(async (id) => (id === "c_slack" ? slackTurns : turns));
  state.agent = agent;
  await render();
  await fromMenu("History");
  expect(host.textContent).toContain("Slack · 2h");
  await press("Load more conversations");
  expect(agent.conversations.mock.calls.at(-1)?.[0]).toMatchObject({ limit: 20, before: "cur" });
  await act(async () => host.querySelector<HTMLButtonElement>('[aria-label^="Deploy plan, from Slack"]')!.click());
  expect(agent.conversation).toHaveBeenLastCalledWith("c_slack", expect.any(AbortSignal));
  expect(host.textContent).not.toContain("Old answer");
  expect(host.textContent).toContain("Plan the deploy");
  expect(stores.get("a")!.save).toHaveBeenCalledWith("c_slack");
  await type("And the rollback?");
  await send();
  expect(agent.ask).toHaveBeenLastCalledWith("And the rollback?", expect.objectContaining({ conversation: "c_slack", surface: "mobile" }));
});

it("says the conversation ended when History opens one the agent no longer holds", async () => {
  const agent = client({ kind: "page", conversations: [slack], next: null });
  agent.conversation.mockImplementation(async (id) => (id === "c_slack" ? { kind: "gone" } : turns));
  state.agent = agent;
  await render();
  await fromMenu("History");
  await act(async () => host.querySelector<HTMLButtonElement>('[aria-label^="Deploy plan"]')!.click());
  expect(host.textContent).toContain("That conversation has ended. Start a new one.");
  await type("Fresh start");
  await send();
  expect(agent.ask.mock.calls[0]?.[1]?.conversation).toBeUndefined();
});

it("asks in voice mode in the conversation opened from History, as the phone, and says the reply", async () => {
  const agent = client({ kind: "page", conversations: [slack], next: null });
  agent.conversation.mockImplementation(async (id) => (id === "c_slack" ? slackTurns : turns));
  state.agent = agent;
  await render();
  await fromMenu("History");
  await act(async () => host.querySelector<HTMLButtonElement>('[aria-label^="Deploy plan"]')!.click());
  let said: unknown;
  await act(async () => {
    said = await voice.ask!("What about Monday?");
  });
  expect(agent.ask).toHaveBeenLastCalledWith("What about Monday?", expect.objectContaining({ conversation: "c_slack", surface: "mobile" }));
  expect(said).toEqual({ say: "New answer" });
  expect(host.textContent).toContain("What about Monday?");
});

const INTRO: AgentIntroRead = {
  kind: "intro",
  intro: { line: "Hand me anything: questions and drafts.", starters: ["Summarize my unread Gmail threads", "Draft a polite reply"], reads: ["Gmail"], accounts: true, discover: true },
};

it("shows the first open with no conversation kept, keeps it seen, and a starter asks its line as the phone (A19)", async () => {
  stores.set("a", store());
  const agent = client({ kind: "off" }, INTRO);
  state.agent = agent;
  await render();
  expect(host.textContent).toContain("Hi Maya. Hand me anything: questions and drafts.");
  expect(host.textContent).toContain("Nexus can read Gmail.");
  expect(host.textContent).toContain("Personal");
  expect(agent.introSeen).toHaveBeenCalledOnce();
  await press("Summarize my unread Gmail threads");
  expect(agent.ask).toHaveBeenCalledWith("Summarize my unread Gmail threads", expect.objectContaining({ surface: "mobile" }));
  expect(host.textContent).toContain("New answer");
  expect(host.textContent).not.toContain("Hand me anything");
  expect(announce).toHaveBeenCalledWith("Nexus answered.");
});

it("asks `discover` for See what I can do, and opens Integrations for Connect your accounts", async () => {
  stores.set("a", store());
  const agent = client({ kind: "off" }, INTRO);
  state.agent = agent;
  await render();
  await press("Connect your accounts");
  expect(state.push).toHaveBeenCalledWith("/integrations");
  await press("See what I can do");
  expect(agent.ask.mock.calls[0]?.[0]).toBe("discover");
});

it("reopens a kept conversation instead, and opens on the empty chat while the first open is off or seen", async () => {
  const kept = client({ kind: "off" }, INTRO);
  state.agent = kept;
  await render();
  expect(kept.intro).not.toHaveBeenCalled();
  expect(host.textContent).toContain("Old answer");
  for (const read of [{ kind: "off" }, { kind: "seen" }, { kind: "failed", status: 503 }] as AgentIntroRead[]) {
    await act(async () => root.unmount());
    root = createRoot(host);
    stores.set("a", store());
    const agent = client({ kind: "off" }, read);
    state.agent = agent;
    await render();
    // The empty thread starts with Nexus's first message, not the first open's line and starters.
    expect(host.textContent).not.toContain("questions and drafts");
    expect(host.textContent).toContain("Hi Maya. I’m Nexus. Ask me for anything.");
    expect(agent.introSeen).not.toHaveBeenCalled();
  }
});

it("names Nexus and where it lives, opens on the profile header, thinks until the answer starts, and asks Nexus anything", async () => {
  const agent = client();
  const answer = Promise.withResolvers<AgentAsk>();
  agent.ask.mockReturnValue(answer.promise);
  state.agent = agent;
  await render();
  // The canvas's phone header: Nexus over "Personal · Assistant", and the menu's one control.
  expect(host.querySelector("header")?.textContent).toBe("NexusPersonal · Assistant");
  expect(host.querySelector('[aria-label="Nexus. Your assistant in Personal"]')?.textContent).toBe("NexusYour assistant in Personal");
  expect(host.querySelector("input")?.getAttribute("placeholder")).toBe("Ask Nexus anything");
  // The answer being written says Thinking until its first words arrive.
  await type("Next?");
  await send();
  expect(host.textContent).toContain("Thinking");
  await act(async () => agent.ask.mock.calls[0]![1]!.onEvent?.({ type: "text", delta: "Wor" }));
  expect(host.textContent).not.toContain("Thinking");
  expect(host.textContent).toContain("Wor");
  await act(async () => answer.resolve({ kind: "reply", reply: { status: 200, conversation: "old", messages: [{ kind: "answer", text: "Working on it.", model: "m", costUsd: 0 }] } }));
  expect(host.textContent).toContain("Working on it.");
});

it("starts today's thread with the brief, and the empty chat offers Discover without one (A1·A, A15·f)", async () => {
  // An organization with no conversation kept: the thread is today's, and empty.
  state.org = "b";
  const agent = client();
  agent.brief.mockResolvedValue({
    kind: "brief",
    brief: { day: "2026-11-18", at: "2026-11-18T08:02:00.000Z", head: "Morning, Maya. Three things:", lines: ["Ana’s birthday is Saturday"], errands: "Dinner Friday needs you" },
  });
  state.agent = agent;
  await render();
  expect(host.querySelector('[aria-label^="Today\'s brief."]')?.textContent).toContain("Morning, Maya. Three things:");
  expect(host.textContent).toContain("Dinner Friday needs you");
  // Nexus started it, so it carries the small header with the time.
  expect(host.querySelector('[aria-label^="Today\'s brief."]')?.textContent).toMatch(/^Nexus · \d\d:\d\d/);
  expect(host.textContent).not.toContain("See what I can do");

  agent.brief.mockResolvedValue({ kind: "off" });
  act(() => root.unmount());
  root = createRoot(host);
  await render();
  expect(host.textContent).not.toContain("Morning, Maya");
  await press("See what I can do");
  expect(state.push).toHaveBeenCalledWith("/discover");
});

it("shows the step that runs while Nexus thinks, then quick replies that send their words (A1·C b, A1·D)", async () => {
  const agent = client();
  const answer = Promise.withResolvers<AgentAsk>();
  agent.ask.mockReturnValueOnce(answer.promise);
  state.agent = agent;
  await render();
  await type("Find dinner");
  await send();
  const request = agent.ask.mock.calls[0]![1]!;
  await act(async () => request.onEvent?.({ type: "step", line: "Searching Resy" }));
  expect(host.textContent).toContain("Searching Resy");
  expect(host.textContent).not.toContain("Thinking");
  await act(async () => request.onEvent?.({ type: "text", delta: "Nopa" }));
  expect(host.textContent).not.toContain("Searching Resy");
  const asks = { kind: "answer", text: "Nopa has 7:30. Book it?", model: "m", costUsd: 0, suggestions: ["Book it", "Other times"] } as const;
  await act(async () => answer.resolve({ kind: "reply", reply: { status: 200, conversation: "new", messages: [asks] } }));
  expect(host.querySelector('[aria-label="Quick replies"]')?.textContent).toBe("Book itOther times");
  await act(async () => host.querySelector<HTMLButtonElement>('[aria-label="Quick replies"] button')!.click());
  expect(agent.ask.mock.calls[1]![0]).toBe("Book it");
  // Once the person answers, the replies leave with the answer they belonged to.
  expect(host.querySelector('[aria-label="Quick replies"]')).toBeNull();
});

it("opens the menu from the bar's one control, with the Assistant's settings last", async () => {
  state.agent = client();
  await render();
  expect(host.querySelector('[role="dialog"]')).toBeNull();
  await press("Nexus menu");
  expect(host.querySelector('[role="dialog"]')?.textContent).toBe("New conversationAssistant settings");
  await press("Assistant settings");
  expect(state.push).toHaveBeenCalledWith("/assistant-settings");
});

it("starts a new conversation from the menu", async () => {
  const agent = client();
  state.agent = agent;
  await render();
  expect(host.textContent).toContain("Old answer");
  await fromMenu("New conversation");
  expect(host.textContent).not.toContain("Old answer");
  expect(host.textContent).toContain("New conversation.");
  await type("Fresh start");
  await send();
  expect(agent.ask.mock.calls[0]?.[1]?.conversation).toBeUndefined();
});

it("plays the send sound as the message goes and the receive sound once, with the first words", async () => {
  const agent = client();
  const answer = Promise.withResolvers<AgentAsk>();
  agent.ask.mockReturnValue(answer.promise);
  state.agent = agent;
  await render();
  chimes.mockClear();
  await type("Book Nopa");
  await send();
  expect(chimes.mock.calls).toEqual([["send"]]);
  await act(async () => agent.ask.mock.calls[0]![1]!.onEvent?.({ type: "text", delta: "On it." }));
  await act(async () => answer.resolve({ kind: "reply", reply: { status: 200, conversation: "old", messages: [{ kind: "answer", text: "On it.", model: "m", costUsd: 0 }] } }));
  expect(chimes.mock.calls).toEqual([["send"], ["receive"]]);
  // Voice mode speaks and listens, so it plays none.
  chimes.mockClear();
  agent.ask.mockResolvedValue({ kind: "reply", reply: { status: 200, conversation: "old", messages: [{ kind: "answer", text: "Sure.", model: "m", costUsd: 0 }] } });
  await act(async () => {
    await voice.ask!("And Saturday?");
  });
  expect(chimes).not.toHaveBeenCalled();
});

describe("the thread as a session transcript (the Assistant canvas)", () => {
  it("says no Read under a question read back, and keeps what it showed for the drawer", async () => {
    state.agent = client();
    await render();
    expect(host.textContent).toContain("Old question");
    expect(host.textContent).not.toContain(`Read ${clock(Date.parse("2026-11-09T00:00:01Z"))}`);
    expect(seenSaves.get("a")).toEqual({ conversation: "old", answers: 1 });
  });

  it("says neither Delivered nor Read while Nexus answers, and thinks until its words start", async () => {
    const agent = client();
    const answer = Promise.withResolvers<AgentAsk>();
    agent.ask.mockReturnValue(answer.promise);
    state.agent = agent;
    await render();
    await type("Book Nopa");
    await send();
    const request = agent.ask.mock.calls[0]![1]!;
    expect(host.textContent).toContain("Thinking");
    await act(async () => request.onEvent?.({ type: "started", conversation: "old" }));
    expect(host.textContent).not.toContain("Delivered");
    await act(async () => request.onEvent?.({ type: "text", delta: "On it." }));
    expect(host.textContent).not.toMatch(/Read \d\d:\d\d/);
    expect(host.textContent).not.toContain("Thinking");
    await act(async () => answer.resolve({ kind: "reply", reply: { status: 200, conversation: "old", messages: [{ kind: "answer", text: "Booked.", model: "m", costUsd: 0 }] } }));
    expect(host.textContent).toContain("Booked.");
    expect(seenSaves.get("a")).toEqual({ conversation: "old", answers: 2 });
  });

  it("says Not sent with Retry when the agent refused the message, and Retry sends it again in its place", async () => {
    const agent = client();
    agent.ask.mockResolvedValueOnce({ kind: "failed", failure: { status: 503, error: "agent_unavailable" } });
    state.agent = agent;
    await render();
    await type("Make it 8 instead");
    await send();
    expect(host.textContent).toContain("Not sent · Retry");
    expect(host.textContent).not.toContain(OFFLINE_LINE);
    await press("Retry");
    expect(agent.ask).toHaveBeenCalledTimes(2);
    expect(agent.ask.mock.calls[1]![0]).toBe("Make it 8 instead");
    expect(host.textContent).not.toContain("Not sent");
    expect(host.textContent).toContain("New answer");
    expect(host.textContent!.split("Make it 8 instead")).toHaveLength(2);
  });

  it("offline, says so, keeps what you send, and sends it in order once the agent API answers again", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const agent = client();
      agent.ask.mockRejectedValueOnce(new TypeError("Network request failed"));
      state.agent = agent;
      await render();
      await type("First");
      await send();
      expect(host.textContent).toContain(OFFLINE_LINE);
      expect(host.textContent).toContain("Not sent · Retry");
      await type("Second");
      await send();
      expect(agent.ask).toHaveBeenCalledTimes(1);
      agent.conversations.mockRejectedValueOnce(new TypeError("Network request failed"));
      await act(async () => void (await vi.advanceTimersByTimeAsync(PROBE_MS)));
      expect(host.textContent).toContain(OFFLINE_LINE);
      await act(async () => void (await vi.advanceTimersByTimeAsync(PROBE_MS)));
      await act(async () => undefined);
      expect(host.textContent).not.toContain(OFFLINE_LINE);
      expect(agent.ask.mock.calls.map((call) => call[0])).toEqual(["First", "First", "Second"]);
      expect(host.textContent).not.toContain("Not sent");
    } finally {
      vi.useRealTimers();
    }
  });

  it("sends the words handed over from Search once, in the kept conversation", async () => {
    params.ask = "dinner friday";
    const agent = client();
    state.agent = agent;
    await render();
    expect(setParams).toHaveBeenCalledWith({ ask: undefined });
    expect(agent.ask).toHaveBeenCalledOnce();
    expect(agent.ask).toHaveBeenLastCalledWith("dinner friday", expect.objectContaining({ conversation: "old", surface: "mobile" }));
    await render();
    expect(agent.ask).toHaveBeenCalledOnce();
  });

  it("sends each new hand-off from Search while the screen stays mounted", async () => {
    params.ask = "dinner friday";
    const agent = client();
    state.agent = agent;
    await render();
    // Expo Router clears the parameter, and the screen stays mounted in the drawer.
    params.ask = undefined;
    await render();
    params.ask = "lunch monday";
    setParams.mockClear();
    await render();
    expect(setParams).toHaveBeenCalledWith({ ask: undefined });
    // Words that arrive with no clear in between are a new hand-off too.
    params.ask = "dinner friday";
    await render();
    expect(agent.ask.mock.calls.map((call) => call[0])).toEqual(["dinner friday", "lunch monday", "dinner friday"]);
  });
});
