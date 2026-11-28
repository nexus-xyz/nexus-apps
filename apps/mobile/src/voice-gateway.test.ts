import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Speaker } from "@/voice";

// The phone's Gateway voice: pieces play in turn from the cache, and the phone's voice takes over when the
// Gateway has none.

const audio = vi.hoisted(() => {
  const players: { uri: string; paused: boolean; removed: boolean; finish: () => void }[] = [];
  const written: string[] = [];
  const deleted: string[] = [];
  return { players, written, deleted };
});

vi.mock("expo-audio", () => ({
  createAudioPlayer: ({ uri }: { uri: string }) => {
    let listener: ((status: { didJustFinish: boolean }) => void) | undefined;
    const player = {
      uri,
      paused: false,
      removed: false,
      finish: () => listener?.({ didJustFinish: true }),
      addListener: (_event: string, run: (status: { didJustFinish: boolean }) => void) => {
        listener = run;
      },
      play: () => undefined,
      pause() {
        player.paused = true;
      },
      remove() {
        player.removed = true;
      },
    };
    audio.players.push(player);
    return player;
  },
}));

vi.mock("expo-file-system", () => ({
  Paths: { cache: "cache:" },
  File: class {
    uri: string;
    constructor(folder: string, name: string) {
      this.uri = `${folder}${name}`;
    }
    writeSync() {
      audio.written.push(this.uri);
    }
    delete() {
      audio.deleted.push(this.uri);
    }
  },
}));

const { gatewaySpeaker } = await import("./voice-gateway");

/** Lets the speaker's promises run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A fallback that records what it was asked to say, and finishes at once. */
function phoneVoice() {
  const said: string[] = [];
  const speaker: Speaker = {
    speak(text, events) {
      said.push(text);
      events.done();
    },
    stop() {},
  };
  return { said, speaker };
}

beforeEach(() => {
  audio.players.length = 0;
  audio.written.length = 0;
  audio.deleted.length = 0;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("gatewaySpeaker on the phone", () => {
  it("tells the caption each piece as it starts, so the words follow the voice", async () => {
    const asked: string[] = [];
    const speaker = gatewaySpeaker(async (text) => {
      asked.push(text);
      return new ArrayBuffer(3);
    }, phoneVoice().speaker);
    const pieces: string[] = [];
    speaker.speak(`${"Uno dos tres. ".repeat(20)}${"Cuatro cinco seis. ".repeat(20)}`, { word: () => undefined, done: () => undefined, piece: (said) => pieces.push(said) });
    await settle();
    for (let i = 0; i < asked.length; i++) {
      audio.players.at(-1)?.finish();
      await settle();
    }
    expect(asked.length).toBeGreaterThan(1);
    expect(pieces).toEqual(asked.map((_, i) => asked.slice(0, i + 1).join(" ")));
  });

  it("plays each piece in turn from the cache, deletes it, and is done after the last", async () => {
    const asked: string[] = [];
    const phone = phoneVoice();
    const speaker = gatewaySpeaker(async (text) => {
      asked.push(text);
      return new ArrayBuffer(3);
    }, phone.speaker);
    const done = vi.fn();
    speaker.speak(`${"Uno dos tres. ".repeat(20)}${"Cuatro cinco seis. ".repeat(20)}`, { word: () => undefined, done });
    await settle();
    expect(asked.length).toBeGreaterThan(1);
    for (let i = 0; i < asked.length; i++) {
      expect(done).not.toHaveBeenCalled();
      audio.players.at(-1)?.finish();
      await settle();
    }
    expect(audio.players).toHaveLength(asked.length);
    expect(audio.deleted).toEqual(audio.written);
    expect(done).toHaveBeenCalledTimes(1);
    expect(phone.said).toEqual([]);
  });

  it("says the answer with the phone's voice when the Gateway has none, and keeps doing so", async () => {
    const say = vi.fn(async () => {
      throw Object.assign(new Error("voice answers are not configured"), { status: 503 });
    });
    const phone = phoneVoice();
    const speaker = gatewaySpeaker(say, phone.speaker);
    const done = vi.fn();
    speaker.speak("Hola, soy Nexus.", { word: () => undefined, done });
    await settle();
    expect(phone.said).toEqual(["Hola, soy Nexus."]);
    expect(done).toHaveBeenCalledTimes(1);
    speaker.speak("Otra respuesta.", { word: () => undefined, done });
    await settle();
    expect(say).toHaveBeenCalledTimes(1);
    expect(phone.said).toEqual(["Hola, soy Nexus.", "Otra respuesta."]);
  });

  it("says the answer with the phone's voice when the voice minutes are used up, and keeps doing so", async () => {
    const say = vi.fn(async () => {
      throw Object.assign(new Error("voice answers are used up for now"), { status: 402 });
    });
    const phone = phoneVoice();
    const speaker = gatewaySpeaker(say, phone.speaker);
    const done = vi.fn();
    speaker.speak("Hola, soy Nexus.", { word: () => undefined, done });
    await settle();
    expect(phone.said).toEqual(["Hola, soy Nexus."]);
    expect(done).toHaveBeenCalledTimes(1);
    speaker.speak("Otra respuesta.", { word: () => undefined, done });
    await settle();
    expect(say).toHaveBeenCalledTimes(1);
    expect(phone.said).toEqual(["Hola, soy Nexus.", "Otra respuesta."]);
  });

  it("stops and removes the playing piece, and never finishes a stopped answer", async () => {
    const phone = phoneVoice();
    const speaker = gatewaySpeaker(async () => new ArrayBuffer(3), phone.speaker);
    const done = vi.fn();
    speaker.speak("Hola, soy Nexus.", { word: () => undefined, done });
    await settle();
    const player = audio.players.at(-1);
    speaker.stop();
    expect(player?.paused).toBe(true);
    expect(player?.removed).toBe(true);
    player?.finish();
    await settle();
    expect(done).not.toHaveBeenCalled();
  });
});
