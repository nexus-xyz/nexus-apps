import * as Speech from "expo-speech";
import { useCallback, useEffect, useRef, useState } from "react";
import { Alert, AppState } from "react-native";
import { useCore } from "@/auth";
import { speechChunks } from "@/answer-text";
import { useScreenFocused } from "@/screen-focus";

/** One response at a time. Leaving the screen, organization or foreground stops playback. */
export function useAnswerSpeech() {
  const core = useCore();
  const focused = useScreenFocused();
  const [reading, setReading] = useState<string | null>(null);
  const generation = useRef(0);
  const stop = useCallback(async () => {
    generation.current += 1;
    setReading(null);
    await Speech.stop();
  }, []);

  useEffect(() => {
    const background = AppState.addEventListener("change", (state) => {
      if (state !== "active") void stop();
    });
    return () => { background.remove(); void stop(); };
  }, [core, stop]);
  useEffect(() => { if (!focused) void stop(); }, [focused, stop]);

  const read = async (id: string, text: string) => {
    const current = generation.current + 1;
    await stop();
    if (current !== generation.current) return;
    const chunks = speechChunks(text, Speech.maxSpeechInputLength);
    if (!chunks.length) return;
    setReading(id);
    const finish = () => { if (generation.current === current) setReading(null); };
    chunks.forEach((chunk, index) => Speech.speak(chunk, {
      useApplicationAudioSession: false,
      onDone: index === chunks.length - 1 ? finish : undefined,
      onStopped: finish,
      onError: (error) => {
        if (generation.current !== current) return;
        void stop();
        Alert.alert("Couldn't read this answer", error.message);
      },
    }));
  };
  return { reading, read, stop };
}
