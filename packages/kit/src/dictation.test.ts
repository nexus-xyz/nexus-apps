import { describe, expect, it, vi } from "vitest";
import { clock, compose, createDictation, downsample, EMPTY, join, level, problemOf, reduceMessage, resample, settle, spoken, type DictationSocket, type DictationState } from "./dictation";

const turn = (transcript: string, formatted = false) => JSON.stringify({ type: "Turn", transcript, turn_is_formatted: formatted, end_of_turn: formatted });

describe("reduceMessage", () => {
  it("shows the turn being spoken as it grows, then swaps in its formatted twin", () => {
    let text = reduceMessage(EMPTY, turn("compare the three"))!;
    expect(text).toEqual({ done: "", live: "compare the three" });
    text = reduceMessage(text, turn("compare the three decoders"))!;
    expect(spoken(text)).toBe("compare the three decoders");
    text = reduceMessage(text, turn("Compare the three decoders.", true))!;
    expect(text).toEqual({ done: "Compare the three decoders.", live: "" });
  });

  it("keeps earlier turns and adds the next one after them", () => {
    let text = reduceMessage({ done: "Compare the three decoders.", live: "" }, turn("and pick one"))!;
    expect(spoken(text)).toBe("Compare the three decoders. and pick one");
    text = reduceMessage(text, turn("And pick one.", true))!;
    expect(text.done).toBe("Compare the three decoders. And pick one.");
  });

  it("ends on Termination and ignores what it does not know", () => {
    expect(reduceMessage(EMPTY, JSON.stringify({ type: "Termination" }))).toBeNull();
    expect(reduceMessage(EMPTY, JSON.stringify({ type: "Begin", id: "x" }))).toBe(EMPTY);
    expect(reduceMessage(EMPTY, "not json")).toBe(EMPTY);
  });

  it("settles the live words as they are when Stop is pressed", () => {
    expect(settle({ done: "One.", live: "two" })).toEqual({ done: "One. two", live: "" });
  });
});

describe("compose", () => {
  it("puts the words at the caret with one space on each side that needs it", () => {
    expect(compose("Hello", "world", "")).toEqual({ text: "Hello world", caret: 11 });
    expect(compose("Hello ", "world", "!")).toEqual({ text: "Hello world !", caret: 11 });
    expect(compose("", "world", " now")).toEqual({ text: "world now", caret: 5 });
  });

  it("gives the box back as it was when nothing was said", () => {
    expect(compose("a", "", "b")).toEqual({ text: "ab", caret: 1 });
  });
});

describe("join", () => {
  it("trims and skips an empty side", () => {
    expect(join(" a ", "")).toBe("a");
    expect(join("", " b")).toBe("b");
    expect(join("a", "b")).toBe("a b");
  });
});

describe("downsample", () => {
  it("makes one 16 kHz sample for every three at 48 kHz", () => {
    const input = new Float32Array(480).fill(0.5);
    const out = downsample(input, 48_000);
    expect(out.length).toBe(160);
    expect(out[0]).toBe(Math.trunc(0.5 * 0x7fff));
  });

  it("clamps to the 16-bit range", () => {
    const out = downsample(new Float32Array([2, 2, -2, -2]), 16_000);
    expect(Array.from(out)).toEqual([0x7fff, 0x7fff, -0x8000, -0x8000]);
  });
});

describe("level", () => {
  it("is 0 for silence and rises with the sound", () => {
    expect(level(new Int16Array(100))).toBe(0);
    expect(level(new Int16Array(100).fill(8000))).toBeGreaterThan(level(new Int16Array(100).fill(1000)));
    expect(level(new Int16Array(100).fill(0x7fff))).toBe(1);
  });
});

describe("problemOf", () => {
  it("names what went wrong", () => {
    expect(problemOf({ name: "NotAllowedError" })).toBe("denied");
    expect(problemOf({ name: "NotFoundError" })).toBe("no-microphone");
    expect(problemOf({ status: 503, code: "voice_not_configured" })).toBe("unavailable");
    expect(problemOf(new Error("socket"))).toBe("failed");
  });
});

describe("a turn is kept once", () => {
  const order = (transcript: string, turn_order: number, formatted: boolean) =>
    JSON.stringify({ type: "Turn", transcript, turn_order, turn_is_formatted: formatted, end_of_turn: formatted });
  it("ignores a formatted turn that is sent again, and a late partial of a finished turn", () => {
    let text = reduceMessage(EMPTY, order("hello there", 0, true))!;
    text = reduceMessage(text, order("hello there", 0, true))!;
    text = reduceMessage(text, order("hello", 0, false))!;
    expect(spoken(text)).toBe("hello there");
    text = reduceMessage(text, order("and more", 1, true))!;
    expect(spoken(text)).toBe("hello there and more");
  });
});

describe("a model that marks every message formatted", () => {
  const msg = (transcript: string, turn_order: number, end: boolean) =>
    JSON.stringify({ type: "Turn", transcript, turn_order, turn_is_formatted: true, end_of_turn: end });
  it("keeps the growing partial as live, and closes the turn on end_of_turn", () => {
    let text = reduceMessage(EMPTY, msg("Hey", 0, false))!;
    text = reduceMessage(text, msg("Hey, this is an audio test.", 0, false))!;
    expect(text).toMatchObject({ done: "", live: "Hey, this is an audio test." });
    text = reduceMessage(text, msg("Hey, this is an audio test.", 0, true))!;
    expect(text).toMatchObject({ done: "Hey, this is an audio test.", live: "" });
    text = reduceMessage(text, msg("And again", 1, false))!;
    expect(spoken(text)).toBe("Hey, this is an audio test. And again");
  });
});

describe("resample", () => {
  it("gives the same samples back at the target rate", () => {
    const input = new Int16Array([1, 2, 3]);
    expect(resample(input, 16_000)).toBe(input);
  });

  it("makes one 16 kHz sample for every three at 48 kHz", () => {
    const out = resample(new Int16Array(480).fill(1000), 48_000);
    expect(out.length).toBe(160);
    expect(out[0]).toBe(1000);
  });
});

describe("clock", () => {
  it("shows minutes and two-digit seconds", () => {
    expect(clock(7)).toBe("0:07");
    expect(clock(72)).toBe("1:12");
  });
});

/** A socket that records what is sent; `say` plays a message from AssemblyAI. */
function fakeSocket() {
  const sent: (ArrayBuffer | string)[] = [];
  const socket: DictationSocket & { say: (data: unknown) => void } = {
    readyState: 0,
    binaryType: "blob",
    onopen: null,
    onerror: null,
    onclose: null,
    onmessage: null,
    send: (data) => void sent.push(data),
    close: () => {
      (socket as { readyState: number }).readyState = 3;
    },
    say: (data) => (socket.onmessage as ((event: { data: unknown }) => void) | null)?.({ data }),
  };
  /** The socket the engine asks for: it opens on the next tick, once the engine listens for it. */
  const connect = () => {
    queueMicrotask(() => {
      (socket as { readyState: number }).readyState = 1;
      (socket.onopen as (() => void) | null)?.();
    });
    return socket;
  };
  return { socket, sent, connect };
}

const token = async () => ({ token: "t", url: "wss://speech.test/v3/ws", sampleRate: 16_000, expiresInSeconds: 60, maxSessionSeconds: 600 });

describe("createDictation", () => {
  it("streams the microphone once the socket is open, shows the words, and closes everything on cancel", async () => {
    const { socket, sent, connect } = fakeSocket();
    let chunk: ((pcm: Int16Array) => void) | undefined;
    const closed = vi.fn();
    const states: DictationState[] = [];
    const words: string[] = [];
    const dictation = createDictation(
      { token, microphone: async (c) => ((chunk = c), closed), socket: connect },
      { state: (s) => states.push(s), text: (t) => words.push(spoken(t)), level: () => {}, problem: () => {} },
    );
    await dictation.start();
    expect(states).toEqual(["starting", "listening"]);
    expect(socket.binaryType).toBe("arraybuffer");
    chunk!(new Int16Array([1, 2]));
    expect(sent).toHaveLength(1);
    expect((sent[0] as ArrayBuffer).byteLength).toBe(4);
    socket.say(turn("hello there"));
    expect(words.at(-1)).toBe("hello there");
    dictation.cancel();
    expect(closed).toHaveBeenCalledOnce();
    expect(socket.readyState).toBe(3);
    expect(states.at(-1)).toBe("idle");
  });

  it("closes the microphone when the token fails, and says why", async () => {
    const closed = vi.fn();
    const problems: string[] = [];
    const dictation = createDictation(
      {
        token: () => Promise.reject(Object.assign(new Error("no key"), { status: 503 })),
        microphone: async () => closed,
        socket: () => fakeSocket().connect(),
      },
      { state: () => {}, text: () => {}, level: () => {}, problem: (p) => problems.push(p) },
    );
    await dictation.start();
    expect(closed).toHaveBeenCalledOnce();
    expect(problems).toEqual(["unavailable"]);
  });

  it("says the microphone was refused", async () => {
    const problems: string[] = [];
    const dictation = createDictation(
      { token, microphone: () => Promise.reject({ name: "NotAllowedError" }), socket: () => fakeSocket().connect() },
      { state: () => {}, text: () => {}, level: () => {}, problem: (p) => problems.push(p) },
    );
    await dictation.start();
    expect(problems).toEqual(["denied"]);
  });
});
