/**
 * The Assistant's sounds, without React Native: a soft whoosh when your message goes, and a soft tone when Nexus's
 * arrives, as Messages plays them. They play only while the switch in Account › Assistant is on, the app is in front,
 * and not twice within 150 ms. A sound that fails to play is dropped: it never stops the message it marks.
 */

export type Chime = "send" | "receive";

type Environment = {
  enabled: () => boolean;
  active: () => boolean;
  now: () => number;
  play: (chime: Chime) => Promise<void> | void;
};

/** Two sounds closer than this are one: a reply that lands right as you send plays once. */
export const CHIME_GAP_MS = 150;

export function createChime(environment: Environment) {
  let last: number | undefined;
  return (chime: Chime) => {
    if (!environment.enabled() || !environment.active()) return;
    const at = environment.now();
    if (last !== undefined && at - last < CHIME_GAP_MS) return;
    last = at;
    try {
      void Promise.resolve(environment.play(chime)).catch(() => undefined);
    } catch {
      // The sound is a flourish: a player that throws leaves the message as it was.
    }
  };
}

/** The switch's key on the device: `false` mutes the sounds. On by default. */
export const SOUNDS_KEY = "pref:assistant-sounds";

/** Whether a value kept under `SOUNDS_KEY` leaves the sounds on. */
export const soundsOn = (kept: unknown) => kept !== false;
