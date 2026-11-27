import { createAudioPlayer, type AudioPlayer } from "expo-audio";
import { File, Paths } from "expo-file-system";
import { speechChunks } from "@/answer-text";
import type { Speaker } from "@/voice";

// Voice mode's answers in the Gateway's ElevenLabs voice. `voice-mode.tsx` gives it the phone's own voice
// to fall back on.

/** The longest piece the Gateway says at once: the first audio comes fast, and it stays under the Gateway's limit. */
const GATEWAY_CHUNK = 300;
/** The level pulses this often while a piece plays: the Gateway's audio has no word events. */
const PULSE_MS = 350;

/** A failure's HTTP status, from a `CoreError` or anything shaped like one. */
const statusOf = (error: unknown) => {
  const status = (error as { status?: unknown } | null)?.status;
  return typeof status === "number" ? status : undefined;
};

/**
 * The Gateway's voice: `say` turns a piece of text into MP3 bytes (`core.createVoiceSpeech`), written to the cache and
 * played with expo-audio, in turn, the next one downloading while one plays. `session` readies the audio first, as the
 * phone's voice does (iOS's solo ambient session). A 503 means the Gateway has no voice, and a 402
 * (`voice_minutes_used_up`) that the voice minutes are used up, so this call and every later one use `fallback`; any
 * other failure says the rest of this answer with `fallback`.
 */
export function gatewaySpeaker(
  say: (text: string, signal: AbortSignal) => Promise<ArrayBuffer>,
  fallback: Speaker,
  session: () => Promise<unknown> = () => Promise.resolve(),
): Speaker {
  let run = 0;
  let unavailable = false;
  let abort: AbortController | undefined;
  let player: AudioPlayer | undefined;
  let pulse: ReturnType<typeof setInterval> | undefined;
  const quiet = () => {
    if (pulse !== undefined) clearInterval(pulse);
    pulse = undefined;
    player?.pause();
    player?.remove();
    player = undefined;
    abort?.abort();
    abort = undefined;
  };
  /** Plays `bytes` to the end, or until it fails; never rejects. The file goes once it has played. */
  const play = (bytes: ArrayBuffer, piece: number) =>
    new Promise<void>((resolve) => {
      const file = new File(Paths.cache, `voice-${Date.now()}-${piece}.mp3`);
      const done = () => {
        try {
          file.delete();
        } catch {
          // Already gone: the cache is the system's to clear.
        }
        resolve();
      };
      try {
        file.writeSync(new Uint8Array(bytes));
        const made = createAudioPlayer({ uri: file.uri });
        player = made;
        made.addListener("playbackStatusUpdate", (status) => {
          if (status.didJustFinish) done();
        });
        made.play();
      } catch {
        done();
      }
    });
  return {
    speak(text, events) {
      const mine = ++run;
      quiet();
      fallback.stop();
      const chunks = speechChunks(text, GATEWAY_CHUNK);
      if (!chunks.length) return events.done();
      if (unavailable) return fallback.speak(text, events);
      // The caption starts with the first piece and grows as each one plays.
      events.piece?.(chunks[0]);
      const controller = new AbortController();
      abort = controller;
      const pending: Promise<ArrayBuffer>[] = [];
      const fetchAt = (i: number) => {
        if (i < chunks.length) pending[i] ??= say(chunks[i], controller.signal);
        return pending[i];
      };
      const ours = () => mine === run;
      const ready = session().catch(() => undefined);
      void (async () => {
        await ready;
        for (let i = 0; i < chunks.length; i++) {
          let bytes: ArrayBuffer;
          try {
            bytes = await fetchAt(i);
            void fetchAt(i + 1)?.catch(() => undefined);
          } catch (error) {
            if (!ours()) return;
            if (statusOf(error) === 503 || statusOf(error) === 402) unavailable = true;
            quiet();
            // The device's voice says the rest in one go, so the caption shows it all.
            events.piece?.(text);
            fallback.speak(chunks.slice(i).join(" "), {
              word: () => ours() && events.word(),
              done: () => {
                if (!ours()) return;
                run += 1;
                events.done();
              },
            });
            return;
          }
          if (!ours()) return;
          pulse ??= setInterval(() => events.word(), PULSE_MS);
          if (i > 0) events.piece?.(chunks.slice(0, i + 1).join(" "));
          await play(bytes, i);
          if (!ours()) return;
        }
        run += 1;
        quiet();
        events.done();
      })();
    },
    stop() {
      run += 1;
      quiet();
      fallback.stop();
    },
  };
}
