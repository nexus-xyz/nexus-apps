/**
 * Dictation: the microphone button types what you say into the box. The engine is here, with no browser or React Native
 * API, so Nexus UI, the desktop's chat and the phone app share it. Each passes in its own microphone (`DictationEnv`).
 *
 * How it works, from the start:
 *  1. The Gateway holds Nexus's AssemblyAI key. `POST /v1/chat/voice-token` trades it for a short-lived token and the
 *     address of AssemblyAI's streaming socket (`VoiceToken`), so the key never reaches the app and the audio never
 *     passes through the Gateway.
 *  2. The microphone gives audio at the device's rate. The caller cuts it to 16 kHz, 16-bit mono PCM, the only format the
 *     socket takes (`downsample` for float samples, `resample` for 16-bit ones), and the engine sends it as binary frames.
 *  3. AssemblyAI answers with `Turn` messages. A turn is one stretch of speech: while it is open its `transcript` grows
 *     (the live words); when it ends (`end_of_turn`), a message with punctuation and capitals (`turn_is_formatted`)
 *     closes it. `reduceMessage` keeps both: `done` is the closed turns, `live` the turn being spoken.
 *  4. The composer shows `compose(before, done + live, after)`: the words go where the caret was when dictation began.
 */
import type { VoiceToken } from "./api";

/** What the socket takes: 16 kHz mono (`VoiceToken.sampleRate` says so too). */
export const TARGET_RATE = 16_000;
/** How long Stop waits for AssemblyAI's last words before it uses what it has. */
export const FINISH_WAIT_MS = 2_500;

export interface DictationText {
  /** The closed turns, joined by spaces. */
  done: string;
  /** The turn being spoken now, unformatted. */
  live: string;
  /** The `turn_order` of the last formatted turn kept: a formatted turn is kept once, however often it is sent. */
  last?: number;
}

export const EMPTY: DictationText = { done: "", live: "" };

/** `a` and `b` with one space between them, or the one that is not empty. */
export function join(a: string, b: string): string {
  const x = a.trim();
  const y = b.trim();
  return x && y ? `${x} ${y}` : x || y;
}

/** What has been said so far, finished or not. */
export function spoken(text: DictationText): string {
  return join(text.done, text.live);
}

/** The words as the box shows them once the turn being spoken is taken as it is (when Stop is pressed). */
export function settle(text: DictationText): DictationText {
  return { done: spoken(text), live: "", last: text.last };
}

/** What one socket message does to the words. `null` when the session is over (`Termination`); unknown messages change nothing. */
export function reduceMessage(text: DictationText, raw: string): DictationText | null {
  let message: { type?: string; transcript?: string; turn_is_formatted?: boolean; end_of_turn?: boolean; turn_order?: number };
  try {
    message = JSON.parse(raw);
  } catch {
    return text;
  }
  if (message.type === "Termination") return null;
  if (message.type !== "Turn") return text;
  const said = (message.transcript ?? "").trim();
  const order = message.turn_order;
  // A turn already kept is not touched again: neither a repeat of it nor a late partial of it.
  if (order !== undefined && text.last !== undefined && order <= text.last) return text;
  // A turn is over when it says so and its words are formatted. Some models mark every message formatted, the partials
  // too, so `turn_is_formatted` alone is not the end.
  if (message.end_of_turn && message.turn_is_formatted) return { done: join(text.done, said), live: "", last: order ?? text.last };
  return { done: text.done, live: said, last: text.last };
}

/**
 * The box's text with the dictated words in it: after `before` and ahead of `after`, a space between wherever there is
 * text on both sides. `caret` is where the caret goes: the end of the words.
 */
export function compose(before: string, dictated: string, after: string): { text: string; caret: number } {
  if (!dictated) return { text: before + after, caret: before.length };
  const lead = before && !/\s$/.test(before) ? " " : "";
  const trail = after && !/^\s/.test(after) ? " " : "";
  const head = before + lead + dictated;
  return { text: head + trail + after, caret: head.length };
}

/** 16-bit PCM from float samples: clamped to -1..1 first. */
const toInt16 = (sample: number) => {
  const clamped = Math.max(-1, Math.min(1, sample));
  return clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff;
};

/** Float samples at `from` Hz as 16-bit PCM at `to` Hz (linear interpolation; `to` no more than `from`). */
export function downsample(input: Float32Array, from: number, to: number = TARGET_RATE): Int16Array {
  const ratio = from / to;
  const length = Math.floor(input.length / ratio);
  const out = new Int16Array(length);
  for (let i = 0; i < length; i++) {
    const at = i * ratio;
    const low = Math.floor(at);
    const high = Math.min(low + 1, input.length - 1);
    const a = input[low] ?? 0;
    out[i] = toInt16(a + ((input[high] ?? a) - a) * (at - low));
  }
  return out;
}

/**
 * 16-bit PCM at `from` Hz as 16-bit PCM at `to` Hz (linear interpolation). The phone asks its microphone for 16 kHz, but
 * hardware that cannot deliver it sends its own rate; the same samples come back when the rates match.
 */
export function resample(input: Int16Array, from: number, to: number = TARGET_RATE): Int16Array {
  if (from === to) return input;
  const ratio = from / to;
  const length = Math.floor(input.length / ratio);
  const out = new Int16Array(length);
  for (let i = 0; i < length; i++) {
    const at = i * ratio;
    const low = Math.floor(at);
    const high = Math.min(low + 1, input.length - 1);
    const a = input[low] ?? 0;
    out[i] = Math.round(a + ((input[high] ?? a) - a) * (at - low));
  }
  return out;
}

/** How loud a chunk is, 0 to 1 (its root mean square, scaled so that speech reaches about the middle). */
export function level(samples: Int16Array): number {
  if (!samples.length) return 0;
  let sum = 0;
  for (const sample of samples) sum += (sample / 0x8000) ** 2;
  return Math.min(1, Math.sqrt(sum / samples.length) * 4);
}

/** Why dictation could not run, as the composer words it. */
export type DictationProblem = "denied" | "no-microphone" | "unavailable" | "failed";

export const PROBLEM_TEXT: Record<DictationProblem, string> = {
  denied: "Nexus can’t use the microphone.",
  "no-microphone": "No microphone found.",
  unavailable: "Dictation isn’t available here.",
  failed: "Dictation stopped. Try again.",
};

/** Shown once, at the first press on a device (privacy sign-off 2026-11-03, condition 4). */
export const NOTICE_TEXT = "While you dictate, your voice goes to our speech-to-text provider. It keeps no audio.";

/**
 * The problem a failed start or session is, from what was thrown. A microphone throws the browser's names
 * (`NotAllowedError` when refused, `NotFoundError` when there is none); the phone's throws the same names.
 */
export function problemOf(error: unknown): DictationProblem {
  const name = (error as { name?: string } | null)?.name;
  if (name === "NotAllowedError" || name === "SecurityError") return "denied";
  if (name === "NotFoundError" || name === "OverconstrainedError") return "no-microphone";
  const status = (error as { status?: number } | null)?.status;
  const code = (error as { code?: string } | null)?.code;
  if (status === 503 || code === "voice_not_configured") return "unavailable";
  return "failed";
}

export type DictationState = "idle" | "starting" | "listening" | "finishing";

export interface DictationEvents {
  state: (state: DictationState) => void;
  /** The words changed. */
  text: (text: DictationText) => void;
  /** The microphone's level, once per chunk. */
  level: (level: number) => void;
  /** It could not run, or stopped on its own. The state is already `idle`. */
  problem: (problem: DictationProblem) => void;
}

export interface Dictation {
  start(): Promise<void>;
  /** Stops listening and waits a moment for the last words. */
  stop(): Promise<void>;
  /** Stops now, keeping the words as they are. */
  cancel(): void;
}

/**
 * The part of a WebSocket the engine uses: the browser's and React Native's both fit. The handlers take `never`, so each
 * platform's own event types are accepted; the engine reads only `data` of a message.
 */
export interface DictationSocket {
  readonly readyState: number;
  binaryType: string;
  onopen: ((event: never) => void) | null;
  onerror: ((event: never) => void) | null;
  onclose: ((event: never) => void) | null;
  onmessage: ((event: never) => void) | null;
  send(data: ArrayBuffer | string): void;
  close(): void;
}

/** What `createDictation` needs from its surroundings: the platform's microphone and socket, stand-ins in the tests. */
export interface DictationEnv {
  /** Asks the Gateway for a `VoiceToken` (`core.createVoiceToken`). */
  token: () => Promise<VoiceToken>;
  /**
   * Opens the microphone and calls `chunk` with 16 kHz, 16-bit mono PCM until the returned function is called. It
   * throws when the microphone is refused or missing (`problemOf` reads what it throws).
   */
  microphone: (chunk: (pcm: Int16Array) => void) => Promise<() => void>;
  socket: (url: string) => DictationSocket;
}

// The socket's ready states, as numbers: React Native's WebSocket has no static constants on every version.
const OPEN = 1;
const CLOSING = 2;

/** The bytes of exactly these samples, for the socket: a view into a larger buffer is copied out. */
const bytesOf = (pcm: Int16Array): ArrayBuffer =>
  pcm.byteOffset === 0 && pcm.byteLength === pcm.buffer.byteLength ? (pcm.buffer as ArrayBuffer) : (pcm.slice().buffer as ArrayBuffer);

/** One dictation at a time. */
export function createDictation(env: DictationEnv, events: DictationEvents): Dictation {
  let state: DictationState = "idle";
  let words = EMPTY;
  let socket: DictationSocket | undefined;
  let closeMicrophone: (() => void) | undefined;
  let waiting: (() => void) | undefined;
  let run = 0;

  const move = (next: DictationState) => {
    state = next;
    events.state(next);
  };
  const quiet = () => {
    const close = closeMicrophone;
    closeMicrophone = undefined;
    close?.();
  };
  const release = () => {
    quiet();
    const open = socket;
    socket = undefined;
    if (open && open.readyState < CLOSING) open.close();
  };
  /** Takes the turn being spoken as it is, closes everything and goes idle. */
  const end = () => {
    run++;
    words = settle(words);
    events.text(words);
    release();
    waiting?.();
    move("idle");
  };
  const fail = (error: unknown) => {
    end();
    events.problem(problemOf(error));
  };

  return {
    async start() {
      if (state !== "idle") return;
      const mine = ++run;
      words = EMPTY;
      events.text(words);
      move("starting");
      // The microphone and the token are asked for together; a token that fails after the microphone opened closes it.
      const voice = env.token();
      voice.catch(() => {});
      try {
        const close = await env.microphone((pcm) => {
          const open = socket;
          if (state !== "listening" || !open || open.readyState !== OPEN) return;
          events.level(level(pcm));
          open.send(bytesOf(pcm));
        });
        if (mine !== run) {
          close();
          return;
        }
        closeMicrophone = close;
        const { url } = await voice;
        if (mine !== run) return;
        const open = env.socket(url);
        open.binaryType = "arraybuffer";
        socket = open;
        await new Promise<void>((resolve, reject) => {
          open.onopen = () => resolve();
          open.onerror = () => reject(new Error("socket"));
          open.onclose = () => reject(new Error("socket"));
        });
        if (mine !== run) return;
        open.onerror = () => mine === run && fail(new Error("socket"));
        open.onclose = () => mine === run && (state === "finishing" ? end() : fail(new Error("socket")));
        open.onmessage = (message: { data: unknown }) => {
          if (typeof message.data !== "string") return;
          const next = reduceMessage(words, message.data);
          if (next === null) return end();
          words = next;
          events.text(words);
        };
        move("listening");
      } catch (error) {
        if (mine === run) fail(error);
        else release();
      }
    },

    async stop() {
      if (state === "starting") return end();
      if (state !== "listening") return;
      move("finishing");
      quiet();
      const open = socket;
      if (open?.readyState === OPEN) open.send(JSON.stringify({ type: "Terminate" }));
      await new Promise<void>((resolve) => {
        waiting = resolve;
        setTimeout(resolve, FINISH_WAIT_MS);
      });
      if ((state as DictationState) === "finishing") end();
    },

    cancel() {
      if (state !== "idle") end();
    },
  };
}

/** Whole seconds as the button shows them: `0:07`, `1:12`. */
export const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
