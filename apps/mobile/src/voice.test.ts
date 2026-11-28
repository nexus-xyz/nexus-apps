import type { AgentConnect, AgentMessage } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import {
  bestVoice,
  createVoiceMode,
  DENIED,
  hearing,
  HOLD_MS,
  levelOf,
  spokenReply,
  UNANSWERED,
  voiceForTone,
  voiceOn,
  WORD_LEVEL,
  YIELD_MS,
  type VoiceReply,
  type VoiceState,
} from "./voice";

// Voice mode in the Assistant (A18, A17): the engine on fake speech modules, what Nexus says, and the voice a tone picks.

/** A fake recognizer and voice, a fake clock, and what the engine told the screen. */
function harness(options: { allow?: boolean | Promise<boolean>; permit?: boolean | Promise<boolean>; reply?: VoiceReply; connect?: (hint: AgentConnect) => void } = {}) {
  const calls: string[] = [];
  const states: VoiceState[] = [];
  const shown = { heard: "", said: "", problem: undefined as string | undefined, level: 0, connects: [] as string[] };
  const timers: { ms: number; run: () => void }[] = [];
  let spoken: { word(): void; done(): void; piece?(said: string): void } | undefined;
  let answer: (reply: VoiceReply) => void = () => undefined;
  const asked: string[] = [];
  const mode = createVoiceMode(
    {
      recognizer: {
        permit: async () => options.permit ?? true,
        start: ({ onDevice }) => calls.push(`start:${onDevice ? "device" : "server"}`),
        stop: () => calls.push("stop"),
        abort: () => calls.push("abort"),
      },
      speaker: {
        speak: (text, events) => {
          calls.push(`speak:${text}`);
          spoken = events;
        },
        stop: () => calls.push("hush"),
      },
      onDevice: true,
      allow: async () => options.allow ?? true,
      connect: options.connect,
      ask: (words) => {
        asked.push(words);
        return options.reply ? Promise.resolve(options.reply) : new Promise((resolve) => (answer = resolve));
      },
      later: (ms, run) => {
        const timer = { ms, run };
        timers.push(timer);
        return () => timers.splice(timers.indexOf(timer), 1);
      },
    },
    {
      state: (next) => states.push(next),
      heard: (words) => (shown.heard = words),
      said: (words) => (shown.said = words),
      problem: (text) => (shown.problem = text),
      level: (value) => (shown.level = value),
      connect: (hints) => (shown.connects = hints.map((hint) => hint.connector)),
    },
  );
  const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
  return { mode, calls, states, shown, timers, asked, settle, answer: (reply: VoiceReply) => answer(reply), speech: () => spoken! };
}

describe("asking to connect an account (A6·e)", () => {
  const linear = { connector: "linear", label: "Linear", url: "https://nexus.example/settings/connectors#linear" };

  it("asks to connect, shows the card, and opens the connection on a spoken yes", async () => {
    const opened: string[] = [];
    const h = harness({ connect: (hint) => opened.push(hint.connector) });
    await h.mode.press();
    h.mode.heard.result("what's on my plate in Linear");
    h.mode.heard.end();
    h.answer(spokenReply([{ kind: "answer", text: "Linear is not connected.", model: "m", costUsd: 0, connect: [linear] }]));
    await h.settle();
    expect(h.shown.said).toBe("To do that, I need to connect Linear. Can I? Say yes, or tap Connect.");
    expect(h.shown.connects).toEqual(["linear"]);
    h.speech().done();
    await h.mode.press();
    h.mode.heard.result("sí");
    h.mode.heard.end();
    await h.settle();
    expect(opened).toEqual(["linear"]);
    expect(h.shown.connects).toEqual([]);
    expect(h.shown.said).toBe("Opening Linear. Sign in there, then ask me again.");
    expect(h.asked).toEqual(["what's on my plate in Linear"]);
  });
});

describe("a turn: listen, ask, speak", () => {
  it("shows the live words, sends them as an ask, and says the answer", async () => {
    const h = harness();
    await h.mode.press();
    expect(h.calls).toEqual(["start:device"]);
    expect(h.states.at(-1)).toBe("listening");
    h.mode.heard.volume(5);
    expect(h.shown.level).toBe(0.5);
    h.mode.heard.result("find a gift");
    h.mode.heard.result("find a gift for Ana");
    expect(h.shown.heard).toBe("find a gift for Ana");
    h.mode.heard.end();
    expect(h.asked).toEqual(["find a gift for Ana"]);
    expect(h.states.at(-1)).toBe("thinking");
    h.answer({ say: "On it. New errand: Gift for Ana.", needsYou: false });
    await h.settle();
    expect(h.states.at(-1)).toBe("speaking");
    expect(h.shown.said).toBe("On it. New errand: Gift for Ana.");
    expect(h.calls.at(-1)).toBe("speak:On it. New errand: Gift for Ana.");
    h.speech().word();
    expect(h.shown.level).toBe(WORD_LEVEL);
    // A voice in pieces moves the caption with it.
    h.speech().piece?.("On it.");
    expect(h.shown.said).toBe("On it.");
    h.speech().done();
    expect(h.states.at(-1)).toBe("ready");
    expect(h.shown.level).toBe(0);
    h.speech().piece?.("late");
    expect(h.shown.said, "a piece after the answer ended changes nothing").toBe("On it.");
  });

  it("a tap stops listening; a hold stops when let go", async () => {
    const tap = harness();
    await tap.mode.press();
    tap.mode.release(120);
    expect(tap.calls).toEqual(["start:device"]);
    await tap.mode.press();
    expect(tap.calls).toEqual(["start:device", "stop"]);

    const hold = harness();
    await hold.mode.press();
    hold.mode.release(HOLD_MS);
    expect(hold.calls).toEqual(["start:device", "stop"]);
  });

  it("talking over Nexus stops it at once, settles for 600 ms, then listens", async () => {
    const h = harness({ reply: { say: "The Uber is about $64, so", needsYou: false } });
    await h.mode.press();
    h.mode.heard.result("book the Uber");
    h.mode.heard.end();
    await h.settle();
    await h.mode.press();
    expect(h.calls.at(-1)).toBe("hush");
    expect(h.states.at(-1)).toBe("interrupted");
    // The old answer's end changes nothing now.
    h.speech().done();
    expect(h.states.at(-1)).toBe("interrupted");
    expect(h.timers.map((timer) => timer.ms)).toEqual([YIELD_MS]);
    h.timers[0].run();
    await h.settle();
    expect(h.states.at(-1)).toBe("listening");
    expect(h.calls.at(-1)).toBe("start:device");
  });

  it("does not start listening after an interrupted hold is released", async () => {
    const h = harness({ reply: { say: "Here is the answer.", needsYou: false } });
    await h.mode.press();
    h.mode.heard.result("hello");
    h.mode.heard.end();
    await h.settle();
    await h.mode.press();
    const yielded = h.timers[0]!;
    h.mode.release(500);
    yielded.run();
    await h.settle();
    expect(h.calls.filter((call) => call.startsWith("start:"))).toHaveLength(1);
    expect(h.states.at(-1)).toBe("ready");
  });

  it.each(["allow", "permit"] as const)("a released hold does not listen after %s resolves", async (pending) => {
    const permission = Promise.withResolvers<boolean>();
    const h = harness({ [pending]: permission.promise });
    const pressed = h.mode.press();
    await h.settle();
    h.mode.release(HOLD_MS);
    permission.resolve(true);
    await pressed;
    expect(h.calls).toEqual([]);
  });

  it("muting cancels capture and does not send the partial words", async () => {
    const h = harness();
    await h.mode.press();
    h.mode.heard.result("This stays on the phone");
    h.mode.mute(true);
    h.mode.heard.end();
    await h.mode.press();
    expect(h.calls).toEqual(["start:device", "abort"]);
    expect(h.shown.heard).toBe("");
    expect(h.asked).toEqual([]);

    h.mode.mute(false);
    await h.mode.press();
    expect(h.calls.at(-1)).toBe("start:device");
    expect(h.states.at(-1)).toBe("listening");
  });

  it("muting keeps the answer playing and blocks the microphone", async () => {
    const h = harness({ reply: { say: "Here is the answer.", needsYou: false } });
    await h.mode.press();
    h.mode.heard.result("hello");
    h.mode.heard.end();
    await h.settle();
    h.mode.mute(true);
    await h.mode.press();
    expect(h.calls).toEqual(["start:device", "speak:Here is the answer."]);
    h.speech().done();
    expect(h.states.at(-1)).toBe("ready");
  });

  it("a write the agent asks for ends on Needs you", async () => {
    const h = harness({ reply: { say: "Send the dinner text. That needs your approval in Nexus UI.", needsYou: true } });
    await h.mode.press();
    h.mode.heard.result("tell them about Friday");
    h.mode.heard.end();
    await h.settle();
    h.speech().done();
    expect(h.states.at(-1)).toBe("needs-you");
  });

  it("End stops listening or speaking, and a late answer is not said", async () => {
    const h = harness();
    await h.mode.press();
    h.mode.heard.result("what's happening with dinner");
    h.mode.heard.end();
    h.mode.end();
    h.answer({ say: "Booked.", needsYou: false });
    await h.settle();
    expect(h.calls.some((call) => call.startsWith("speak:"))).toBe(false);
    expect(h.states.at(-1)).toBe("ready");
    await h.mode.press();
    expect(h.calls).toEqual(["start:device"]);

    const listening = harness();
    await listening.mode.press();
    listening.mode.end();
    expect(listening.calls).toEqual(["start:device", "abort"]);
  });
});

describe("when it cannot hear", () => {
  it("says nothing was heard, and a refused microphone says where to allow it", async () => {
    const silent = harness();
    await silent.mode.press();
    silent.mode.heard.error("no-speech");
    silent.mode.heard.end();
    expect(silent.states.at(-1)).toBe("cant-hear");
    expect(silent.shown.problem).toMatch(/didn't hear anything/);
    expect(silent.asked).toEqual([]);

    const refused = harness({ permit: false });
    await refused.mode.press();
    expect(refused.calls).toEqual([]);
    expect(refused.shown.problem).toBe(DENIED);
  });

  it("never sends the words of a failed recognition", async () => {
    const h = harness();
    await h.mode.press();
    h.mode.heard.result("send the");
    h.mode.heard.error("network");
    h.mode.heard.end();
    expect(h.asked).toEqual([]);
    expect(h.shown.problem).toMatch(/speech service did not answer/);
  });

  it("does not listen when the person says not now to where the voice goes", async () => {
    const h = harness({ allow: false });
    await h.mode.press();
    expect(h.calls).toEqual([]);
    expect(h.states).toEqual([]);
  });

  it("an ask that fails says so", async () => {
    const h = harness({ reply: UNANSWERED });
    await h.mode.press();
    h.mode.heard.result("hello");
    h.mode.heard.end();
    await h.settle();
    expect(h.states.at(-1)).toBe("cant-hear");
    expect(h.shown.problem).toMatch(/did not answer/);
  });
});

describe("what Nexus says for a reply", () => {
  const answer = (text: string): AgentMessage => ({ kind: "answer", text, model: "m", costUsd: 0 });
  it("says the answer's words, not its Markdown", () => {
    expect(spokenReply([answer("**Booked:** Nopa, Friday at 7:30.")])).toEqual({ say: "Booked: Nopa, Friday at 7:30.", needsYou: false });
  });
  it("says a write needs approval in Nexus UI, and a refusal's reason", () => {
    const write: AgentMessage = {
      kind: "pending_action",
      card: "c",
      write: { connector: "slack", operation: "post", summary: "Post the dinner text in #family", effect: "write", arguments: {} },
      approveUrl: "https://nexus.inc/a",
      detail: "d",
    };
    expect(spokenReply([write])).toEqual({ say: "Post the dinner text in #family. That needs your approval in Nexus UI.", needsYou: true });
    expect(spokenReply([{ kind: "refused", rule: "r", detail: "Your budget is used up." }])).toEqual({ say: "Your budget is used up.", needsYou: false });
    expect(spokenReply([])).toBe(UNANSWERED);
  });
});

describe("the voice", () => {
  it("is on only with the build's flag", () => {
    expect(voiceOn("on")).toBe(true);
    expect(voiceOn(" ON ")).toBe(true);
    expect(voiceOn(undefined)).toBe(false);
    expect(voiceOn("1")).toBe(false);
  });

  it("starts from the profile's tone (A17): Calm and custom take Ash", () => {
    expect([undefined, null, "calm", "custom", "warm", "direct", "lively"].map(voiceForTone)).toEqual(["ash", "ash", "ash", "ash", "cedar", "brook", "fern"]);
  });

  it("takes iOS's best voice in the phone's language, never a novelty one", () => {
    const voices = [
      { identifier: "com.apple.speech.synthesis.voice.Zarvox", quality: "Enhanced", language: "en-US" },
      { identifier: "com.apple.voice.compact.en-GB.Daniel", quality: "Default", language: "en-GB" },
      { identifier: "com.apple.voice.compact.en-US.Samantha", quality: "Default", language: "en-US" },
      { identifier: "com.apple.voice.enhanced.en-US.Ava", quality: "Enhanced", language: "en-US" },
      { identifier: "com.apple.voice.premium.es-ES.Marisol", quality: "Default", language: "es-ES" },
    ];
    expect(bestVoice(voices, "en-US")).toBe("com.apple.voice.enhanced.en-US.Ava");
    // No voice for the region: the language's best, here an enhanced one over a default one.
    expect(bestVoice(voices, "en_AU")).toBe("com.apple.voice.enhanced.en-US.Ava");
    expect(bestVoice(voices, "es-MX")).toBe("com.apple.voice.premium.es-ES.Marisol");
    expect(bestVoice(voices, "fr-FR")).toBeUndefined();
  });

  it("says where the voice goes: the phone itself, or the platform's recognizer", () => {
    expect([hearing(true, "ios"), hearing(false, "ios"), hearing(false, "android")]).toEqual(["device", "apple", "android"]);
  });

  it("reads the microphone's volume as a level", () => {
    expect([levelOf(-2), levelOf(0), levelOf(7), levelOf(12)]).toEqual([0, 0, 0.7, 1]);
  });
});
