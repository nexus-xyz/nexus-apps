import { answerConnects, connectAsk, isNo, isYes, type AgentConnect, type AgentMessage } from "@nexus/kit";
import { answerText } from "./answer-text";
import type { VoiceState } from "./voice-motion";

export type { VoiceState };

/**
 * Voice mode in the Assistant (Assistant boards A18, A25 and A17), without React Native: its states, the voice a tone
 * picks, what the first press says about where the voice goes, and the engine that runs each turn: listen, ask,
 * speak. `voice-mode.tsx` gives the engine the phone's recognizer (expo-speech-recognition) and its voice
 * (expo-speech). The tests give it fakes.
 */

/** The build turns voice mode on with `EXPO_PUBLIC_NEXUS_AGENT_VOICE=on`. */
export const voiceOn = (flag: string | undefined) => flag?.trim().toLowerCase() === "on";

/** The one word under the mark (A25). */
export const VOICE_WORD: Record<VoiceState, string> = {
  ready: "Ready",
  listening: "Listening",
  thinking: "Thinking",
  speaking: "Speaking",
  interrupted: "Interrupted",
  "needs-you": "Needs you",
  "cant-hear": "Can't hear you",
};

/**
 * A17's six voices. The phone does not say how a voice sounds, so each one is the phone's best voice at its own pitch
 * and rate: Ash low and even, Brook clear and quick, Cedar warm and slower, Dune soft and unhurried, Fern bright and
 * light, Slate deep and plain. The picker (A17·d) is a follow-up; until then the tone picks the voice.
 */
export const VOICES = {
  ash: { pitch: 0.9, rate: 1 },
  brook: { pitch: 1, rate: 1.1 },
  cedar: { pitch: 0.95, rate: 0.9 },
  dune: { pitch: 1, rate: 0.85 },
  fern: { pitch: 1.15, rate: 1.05 },
  slate: { pitch: 0.8, rate: 1 },
} as const;
export type VoiceName = keyof typeof VOICES;

/** The voice an agent profile's tone starts with. Calm, the default, and `custom` take Ash. */
export function voiceForTone(tone: string | null | undefined): VoiceName {
  if (tone === "warm") return "cedar";
  if (tone === "direct") return "brook";
  if (tone === "lively") return "fern";
  return "ash";
}

/** One voice the phone has, as expo-speech lists it. */
export interface PhoneVoice {
  identifier: string;
  quality: string;
  language: string;
}

/** Apple's novelty and legacy voices (Bells, Zarvox, Eloquence): never Nexus's voice. */
const NOVELTY = /speech\.synthesis\.voice|eloquence/i;

/**
 * iOS's best voice for `language` (BCP 47): the same language and region first, then the language alone; a premium
 * voice before an enhanced one, and an enhanced one before the default. `undefined`: the system's voice for the
 * language. Android lists voices it has not installed, so there the system picks.
 */
export function bestVoice(voices: PhoneVoice[], language: string): string | undefined {
  const tag = (value: string) => value.toLowerCase().replace("_", "-");
  const want = tag(language);
  const base = want.split("-")[0];
  let best: string | undefined;
  let top = 0;
  for (const voice of voices) {
    const lang = tag(voice.language);
    const match = lang === want ? 2 : lang.split("-")[0] === base ? 1 : 0;
    if (!match || NOVELTY.test(voice.identifier)) continue;
    const score = match * 10 + (/premium/i.test(voice.identifier) ? 2 : voice.quality === "Enhanced" ? 1 : 0);
    if (score > top) [top, best] = [score, voice.identifier];
  }
  return best;
}

/** Where the voice goes while it becomes words (A11): it stays on the phone, or the platform's recognizer hears it. */
export type Hearing = "device" | "apple" | "android";

export const hearing = (onDevice: boolean, os: string): Hearing => (onDevice ? "device" : os === "ios" ? "apple" : "android");

/** What the first press says, once per phone and per place the voice goes. */
export const HEARING_TEXT: Record<Hearing, string> = {
  device: "This phone turns your voice into words itself. The audio stays on the phone, and Nexus gets only the words.",
  apple: "This iPhone sends your voice to Apple to turn it into words, as Apple's dictation does. Nexus gets only the words, never the audio.",
  android: "This phone sends your voice to its speech service to turn it into words. On most phones that is Google's. Nexus gets only the words, never the audio.",
};

/** The device keeps that the person read it (a `pref:` key: it stays on sign-out). */
export const hearingKey = (where: Hearing) => `pref:voice-hearing:${where}`;

/**
 * What the agent's reply gives voice mode: the words to say, or why there are none. `connect`: the accounts the answer
 * needs first, which voice mode asks to connect (A6·e).
 */
export type VoiceReply = { say: string; needsYou: boolean; connect?: AgentConnect[] } | { problem: string };

export const UNANSWERED: VoiceReply = { problem: "Nexus did not answer. Try again, or tap Type." };

/**
 * The words Nexus says for a reply: its answers as plain text, a refusal's reason, and, for a write it asks for, that
 * it needs you. The phone never approves a write (Nexus never writes from here), so it says where to approve. An
 * answer that needs an account first is replaced by a question to connect it.
 */
export function spokenReply(messages: AgentMessage[]): VoiceReply {
  const parts: string[] = [];
  let needsYou = false;
  const connect: AgentConnect[] = [];
  for (const message of messages) {
    for (const hint of message.kind === "answer" ? answerConnects(message) : []) {
      if (!connect.some((known) => known.connector === hint.connector)) connect.push(hint);
    }
  }
  for (const message of messages) {
    // An answer that needs an account is asked as a question, not read out (A6·e).
    if (message.kind === "answer" && connect.length) continue;
    if (message.kind === "answer") parts.push(answerText(message.text));
    else if (message.kind === "refused") parts.push(message.detail);
    else if (message.kind === "pending_action") {
      needsYou = true;
      parts.push(`${message.write.summary}. That needs your approval in Nexus UI.`);
    }
  }
  if (connect.length) parts.unshift(connectAsk(connect));
  const say = parts.filter(Boolean).join("\n\n");
  if (connect.length) return { say, needsYou, connect };
  return say || needsYou ? { say, needsYou } : UNANSWERED;
}

/** The phone's speech recognizer, as the engine drives it. */
export interface Recognizer {
  /** Asks for the microphone and speech recognition; false when the person refuses. */
  permit(): Promise<boolean>;
  start(options: { onDevice: boolean }): void;
  /** Stops listening: the words so far come as a last result, then `end`. */
  stop(): void;
  /** Stops at once, with no result. */
  abort(): void;
}

/** What the recognizer tells the engine, in the order it does: results and volume, maybe an error, then `end`. */
export interface Heard {
  result(words: string): void;
  /** The input's volume, -2 to 10; under 0 is silence. */
  volume(value: number): void;
  error(code: string): void;
  end(): void;
}

/**
 * The phone's voice. `done` comes once the words end, or when they stop or fail. A voice that speaks in pieces calls
 * `piece` with the words said so far, through the piece it starts, so the caption follows it.
 */
export interface Speaker {
  speak(text: string, events: { word(): void; done(): void; piece?(said: string): void }): void;
  stop(): void;
}

export interface VoiceEnv {
  recognizer: Recognizer;
  speaker: Speaker;
  /** Whether the phone can turn speech into words by itself (`requiresOnDeviceRecognition`). */
  onDevice: boolean;
  /** The first time, says where the voice goes (`HEARING_TEXT`); false when the person says not now. */
  allow(): Promise<boolean>;
  /** Sends the words as a normal ask in the conversation. */
  ask(words: string): Promise<VoiceReply>;
  /** Opens where the person connects `hint`. Without it, a "yes" goes to the agent as words. */
  connect?(hint: AgentConnect): void;
  /** Runs `run` after `ms`; the result cancels it. */
  later(ms: number, run: () => void): () => void;
}

export interface VoiceEvents {
  state(next: VoiceState): void;
  /** The person's words: live while they talk, then the last thing they said. */
  heard(words: string): void;
  /** The last thing Nexus said. */
  said(words: string): void;
  /** The voice's level, 0 to 1: the microphone's while listening, a pulse per word while speaking. */
  level(value: number): void;
  problem(text: string | undefined): void;
  /** The accounts Nexus asked to connect, for the Connect card under the captions; empty once answered. */
  connect?(hints: AgentConnect[]): void;
}

/** A press held this long is hold to talk: letting go stops. A shorter one is a tap: tap again to stop. */
export const HOLD_MS = 450;
/** A25: interrupted, the mark settles for 600 ms, then it listens. */
export const YIELD_MS = 600;
/** The level each spoken word pulses to. */
export const WORD_LEVEL = 0.75;

const HEARD_NOTHING = "I didn't hear anything. Tap Talk and try again.";
export const DENIED = "Nexus can't use the microphone or speech recognition. Allow both in Settings.";
const PROBLEMS: Record<string, string> = {
  "no-speech": HEARD_NOTHING,
  "speech-timeout": HEARD_NOTHING,
  "not-allowed": DENIED,
  "language-not-supported": "This phone can't turn speech into words in your language yet. Tap Type.",
  "service-not-allowed": "Speech recognition is off on this phone. Turn on dictation in Settings, or tap Type.",
  network: "The phone's speech service did not answer. Try again, or tap Type.",
  interrupted: "Something else took the microphone. Tap Talk to try again.",
};
const FAILED = "That didn't work. Tap Talk to try again, or tap Type.";

/** The microphone's volume (-2 to 10) as a level, 0 to 1. */
export const levelOf = (volume: number) => Math.min(1, Math.max(0, volume / 10));

export interface VoiceMode {
  /** Wire the recognizer's events here. */
  heard: Heard;
  /** Talk pressed: listen, stop listening, or talk over Nexus. */
  press(): Promise<void>;
  /** Talk let go after `heldMs`: a hold stops listening. */
  release(heldMs: number): void;
  mute(muted: boolean): void;
  /** Leaves voice mode: stops listening and speaking. An answer still coming lands in the thread, unspoken. */
  end(): void;
}

export function createVoiceMode(env: VoiceEnv, events: VoiceEvents): VoiceMode {
  let state: VoiceState = "ready";
  let words = "";
  let failure: string | undefined;
  let holding = false;
  let ended = false;
  let muted = false;
  // Each turn and each end moves it on, so a late answer, word or end of an older turn changes nothing.
  let turn = 0;
  let cancelYield: (() => void) | undefined;

  const set = (next: VoiceState) => {
    if (next === state) return;
    state = next;
    events.state(next);
    if (next !== "listening" && next !== "speaking") events.level(0);
  };

  const listen = async () => {
    const mine = ++turn;
    events.problem(undefined);
    if (!(await env.allow()) || mine !== turn) return;
    if (!(await env.recognizer.permit())) {
      if (mine === turn) fail(DENIED);
      return;
    }
    if (mine !== turn) return;
    words = "";
    failure = undefined;
    events.heard("");
    set("listening");
    env.recognizer.start({ onDevice: env.onDevice });
  };

  const fail = (text: string) => {
    events.problem(text);
    set("cant-hear");
  };

  /** The accounts Nexus last asked to connect: the next short yes opens the first, a no puts them away. */
  let asked: AgentConnect[] = [];
  const offer = (hints: AgentConnect[]) => {
    if (!hints.length && !asked.length) return;
    asked = hints;
    events.connect?.(hints);
  };
  /** A spoken answer to "Can I?" that voice mode answers itself; `undefined` sends the words to the agent. */
  const connectAnswer = (said: string): VoiceReply | undefined => {
    const hint = asked[0];
    if (!hint) return undefined;
    offer([]);
    if (isYes(said) && env.connect) {
      env.connect(hint);
      return { say: `Opening ${hint.label}. Sign in there, then ask me again.`, needsYou: false };
    }
    if (isNo(said)) return { say: "Okay, not now.", needsYou: false };
    return undefined;
  };

  const answer = async (said: string) => {
    const mine = ++turn;
    events.said("");
    set("thinking");
    const local = connectAnswer(said);
    const reply = local ?? (await env.ask(said).catch(() => UNANSWERED));
    if (mine !== turn) return;
    if ("problem" in reply) return fail(reply.problem);
    const after: VoiceState = reply.needsYou ? "needs-you" : "ready";
    offer(reply.connect ?? []);
    events.said(reply.say);
    if (!reply.say) return set(after);
    set("speaking");
    env.speaker.speak(reply.say, {
      word: () => {
        if (mine === turn && state === "speaking") events.level(WORD_LEVEL);
      },
      piece: (soFar) => {
        if (mine === turn && state === "speaking") events.said(soFar);
      },
      done: () => {
        if (mine === turn && state === "speaking") set(after);
      },
    });
  };

  const heard: Heard = {
    result: (said) => {
      if (state !== "listening") return;
      words = said;
      events.heard(said);
    },
    volume: (value) => {
      if (state === "listening") events.level(levelOf(value));
    },
    error: (code) => {
      if (state === "listening" && code !== "aborted") failure = PROBLEMS[code] ?? FAILED;
    },
    end: () => {
      if (state !== "listening") return;
      const said = words.trim();
      if (failure && failure !== HEARD_NOTHING) return fail(failure);
      if (said) return void answer(said);
      fail(HEARD_NOTHING);
    },
  };

  return {
    heard,
    async press() {
      if (ended || muted || state === "thinking" || state === "interrupted") return;
      if (state === "listening") return env.recognizer.stop();
      holding = true;
      if (state !== "speaking") return listen();
      // Talking over Nexus: it stops at once, settles, then listens (A18: within 200 ms, a 600 ms yield).
      const mine = ++turn;
      env.speaker.stop();
      set("interrupted");
      cancelYield = env.later(YIELD_MS, () => {
        cancelYield = undefined;
        if (mine === turn) void listen();
      });
    },
    release(heldMs) {
      const held = holding && heldMs >= HOLD_MS;
      holding = false;
      if (!held) return;
      if (state === "listening") env.recognizer.stop();
      else if (state !== "thinking" && state !== "speaking") {
        // A released hold also cancels startup waiting on the yield or a permission prompt.
        turn += 1;
        cancelYield?.();
        cancelYield = undefined;
        if (state === "interrupted") set("ready");
      }
    },
    mute(next) {
      if (ended || muted === next) return;
      muted = next;
      if (!muted || state === "thinking" || state === "speaking") return;
      turn += 1;
      holding = false;
      cancelYield?.();
      cancelYield = undefined;
      const listening = state === "listening";
      set("ready");
      if (listening) env.recognizer.abort();
      events.heard("");
      events.problem(undefined);
    },
    end() {
      if (ended) return;
      ended = true;
      turn += 1;
      cancelYield?.();
      if (state === "listening") env.recognizer.abort();
      if (state === "speaking") env.speaker.stop();
      set("ready");
    },
  };
}
