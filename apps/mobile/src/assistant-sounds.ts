import { createAudioPlayer, type AudioPlayer } from "expo-audio";
import { AppState, Platform } from "react-native";
import { createChime, soundsOn, SOUNDS_KEY, type Chime } from "@/assistant-chime";
import { load, save } from "@/store";

/**
 * The Assistant's send and receive sounds (`assistant-chime.ts`), through expo-audio, already in the app for dictation.
 * expo-audio's session mixes with other audio by default, so a sound never stops the person's music, and iOS keeps it
 * silent while the ring switch is off, as Messages does. Account › Assistant mutes them, kept on this phone.
 */

let enabled = soundsOn(load<unknown>(SOUNDS_KEY));

export const assistantSoundsOn = () => enabled;

export function setAssistantSounds(value: boolean) {
  enabled = value;
  save(SOUNDS_KEY, value);
}

const SOURCES = { send: require("../assets/sounds/send.wav"), receive: require("../assets/sounds/receive.wav") } as const;
/** Quiet: a flourish under the haptic, not an alert. */
const VOLUME = 0.35;
const players: Partial<Record<Chime, AudioPlayer>> = {};

function playerOf(chime: Chime): AudioPlayer {
  const kept = players[chime];
  if (kept) return kept;
  const made = createAudioPlayer(SOURCES[chime]);
  made.volume = VOLUME;
  players[chime] = made;
  return made;
}

function play(chime: Chime) {
  const player = playerOf(chime);
  // From the start each time: a second message plays its sound again rather than resuming the first.
  void player.seekTo(0).then(() => player.play(), () => player.play());
}

export const chime = createChime({
  enabled: assistantSoundsOn,
  active: () => Platform.OS !== "web" && AppState.currentState === "active",
  now: Date.now,
  play,
});
