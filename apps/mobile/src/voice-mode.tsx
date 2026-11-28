import { requireOptionalNativeModule } from "expo";
import { setAudioModeAsync } from "expo-audio";
import * as Speech from "expo-speech";
import type { ExpoSpeechRecognitionModuleType } from "expo-speech-recognition/build/ExpoSpeechRecognitionModule.types";
import { useCallback, useEffect, useRef, useState, type ComponentRef } from "react";
import { AccessibilityInfo, Alert, AppState, Linking, Platform, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useSharedValue, type SharedValue } from "react-native-reanimated";
import { speechChunks } from "@/answer-text";
import { useCore } from "@/auth";
import { Icon } from "@/icon";
import { haptic } from "@/haptics";
import { useScreenFocused } from "@/screen-focus";
import { load, save } from "@/store";
import { font, text, type Theme } from "@/theme";
import {
  bestVoice,
  createVoiceMode,
  DENIED,
  hearing,
  HEARING_TEXT,
  hearingKey,
  voiceForTone,
  voiceOn,
  VOICE_WORD,
  VOICES,
  type Recognizer,
  type Speaker,
  type VoiceMode,
  type VoiceReply,
  type VoiceState,
} from "@/voice";
import { VoiceMark } from "@/voice-mark";
import { useAssistantName } from "@/use-assistant-name";
import { gatewaySpeaker } from "@/voice-gateway";
import type { AgentConnect } from "@nexus/kit";
import { connectCard } from "@/assistant-cards";
import { ConnectCard } from "@/connect-card";
import { open as openLink } from "@/markdown";

/**
 * Voice mode in the Assistant (A18, A25): expo-speech-recognition turns the voice into words, on the phone when it can
 * (`requiresOnDeviceRecognition`). The Gateway's ElevenLabs voice says the answer (`POST /v1/chat/voice-speech`,
 * an internal ticket), and expo-speech does when the Gateway has none. `voice.ts` runs the turns.
 * A build before this module, or Expo Go, has no recognizer, so it shows no voice button.
 */
const recognizer = requireOptionalNativeModule<ExpoSpeechRecognitionModuleType>("ExpoSpeechRecognition");

/** Whether the Assistant offers voice mode: the build's flag, and the recognizer in the build. */
export const voiceAvailable = voiceOn(process.env.EXPO_PUBLIC_NEXUS_AGENT_VOICE) && recognizer !== null;

/** The phone's language, for the recognizer and the voice. */
const LANGUAGE = Intl.DateTimeFormat().resolvedOptions().locale || "en-US";
/** The phone's voices, read once: iOS names its best one; Android picks its own. */
let voices: Promise<string | undefined> | undefined;
const phoneVoice = () =>
  (voices ??= Platform.OS === "ios" ? Speech.getAvailableVoicesAsync().then((all) => bestVoice(all, LANGUAGE), () => undefined) : Promise.resolve(undefined));

/** The recognizer for one turn after another. Results carry the best transcript only. */
function phoneRecognizer(native: ExpoSpeechRecognitionModuleType): Recognizer {
  return {
    permit: () => native.requestPermissionsAsync().then((answer) => answer.granted, () => false),
    start: ({ onDevice }) =>
      native.start({
        lang: LANGUAGE,
        interimResults: true,
        continuous: false,
        maxAlternatives: 1,
        addsPunctuation: true,
        requiresOnDeviceRecognition: onDevice,
        volumeChangeEventOptions: { enabled: true, intervalMillis: 60 },
      }),
    stop: () => native.stop(),
    abort: () => native.abort(),
  };
}

/**
 * The phone's voice at the tone's pitch and rate. On iOS it speaks in the app's own audio session, set to solo ambient
 * first: the silent switch mutes it, and the captions still carry the words. Android speaks on the media stream, so
 * its media volume sets it.
 */
function phoneSpeaker(tone: string | null | undefined): Speaker {
  const { pitch, rate } = VOICES[voiceForTone(tone)];
  let speaking = 0;
  return {
    speak(words, events) {
      const mine = ++speaking;
      const session = Platform.OS === "ios" ? setAudioModeAsync({ playsInSilentMode: false, allowsRecording: false, interruptionMode: "doNotMix" }) : undefined;
      void Promise.all([phoneVoice(), session?.catch(() => undefined)]).then(([voice]) => {
        if (mine !== speaking) return;
        const chunks = speechChunks(words, Speech.maxSpeechInputLength);
        if (!chunks.length) return events.done();
        chunks.forEach((chunk, i) =>
          Speech.speak(chunk, {
            language: LANGUAGE,
            voice,
            pitch,
            rate,
            useApplicationAudioSession: true,
            onBoundary: events.word,
            onDone: i === chunks.length - 1 ? events.done : undefined,
            onStopped: events.done,
            onError: events.done,
          }),
        );
      });
    },
    stop() {
      speaking += 1;
      void Speech.stop();
    },
  };
}

/** The first press on this phone says where the voice goes (A11), and goes on only once the person says so. */
function allowHearing(onDevice: boolean): Promise<boolean> {
  const where = hearing(onDevice, Platform.OS);
  if (load<boolean>(hearingKey(where))) return Promise.resolve(true);
  return new Promise((answer) =>
    Alert.alert(
      "Talk to Nexus",
      HEARING_TEXT[where],
      [
        { text: "Not now", style: "cancel", onPress: () => answer(false) },
        {
          text: "Continue",
          onPress: () => {
            save(hearingKey(where), true);
            answer(true);
          },
        },
      ],
      { cancelable: true, onDismiss: () => answer(false) },
    ),
  );
}

export interface VoiceView {
  open: boolean;
  state: VoiceState;
  heard: string;
  said: string;
  problem: string | undefined;
  muted: boolean;
  /** The accounts Nexus asked to connect (A6·e): their Connect card shows under the captions until the person answers. */
  connects: AgentConnect[];
  toggleMute: () => void;
  level: SharedValue<number>;
  /** Opens voice mode and listens at once. */
  start: () => void;
  press: () => void;
  release: () => void;
  close: () => void;
}

/** Voice mode for one screen. `ask` sends the words as a normal ask; `tone` is the agent profile's. */
export function useVoiceMode(ask: (words: string) => Promise<VoiceReply>, tone?: string | null): VoiceView {
  const assistant = useAssistantName();
  const signedIn = useCore();
  const latestCore = useRef(signedIn);
  useEffect(() => {
    latestCore.current = signedIn;
  });
  const [open, setOpen] = useState(false);
  const [muted, setMuted] = useState(false);
  const [state, setState] = useState<VoiceState>("ready");
  const [heard, setHeard] = useState("");
  const [said, setSaid] = useState("");
  const [problem, setProblem] = useState<string>();
  const [connects, setConnects] = useState<AgentConnect[]>([]);
  const level = useSharedValue(0);
  const engine = useRef<VoiceMode>(undefined);
  const pressedAt = useRef(0);
  const latest = useRef(ask);
  useEffect(() => {
    latest.current = ask;
  });
  const focused = useScreenFocused();

  const close = useCallback(() => {
    engine.current?.end();
    engine.current = undefined;
    setOpen(false);
  }, []);

  const press = useCallback(() => {
    pressedAt.current = Date.now();
    void engine.current?.press();
  }, []);

  const start = useCallback(() => {
    if (!recognizer) return;
    engine.current?.end();
    setHeard("");
    setSaid("");
    setMuted(false);
    setProblem(undefined);
    setConnects([]);
    setState("ready");
    const onDevice = recognizer.supportsOnDeviceRecognition();
    engine.current = createVoiceMode(
      {
        recognizer: phoneRecognizer(recognizer),
        speaker: gatewaySpeaker(
          (words, signal) => {
            const core = latestCore.current;
            return core.createVoiceSpeech(words, signal);
          },
          phoneSpeaker(tone),
          () => (Platform.OS === "ios" ? setAudioModeAsync({ playsInSilentMode: false, allowsRecording: false, interruptionMode: "doNotMix" }) : Promise.resolve()),
        ),
        onDevice,
        allow: () => allowHearing(onDevice),
        ask: (words) => latest.current(words),
        // A spoken yes opens the connector's card on Nexus UI's Integrations page, as the Connect card's button does.
        connect: (hint) => openLink(hint.url),
        later: (ms, run) => {
          const timer = setTimeout(run, ms);
          return () => clearTimeout(timer);
        },
      },
      {
        state: setState,
        heard: setHeard,
        said: setSaid,
        problem: setProblem,
        connect: setConnects,
        level: (value) => {
          level.value = value;
        },
      },
    );
    setOpen(true);
    press();
  }, [level, press, tone]);

  // The recognizer's events go to the engine, if voice mode is open. Subscribed from the start, so none comes too soon.
  useEffect(() => {
    if (!voiceAvailable || !recognizer) return;
    const subscriptions = [
      recognizer.addListener("result", (event) => engine.current?.heard.result(event.results[0]?.transcript ?? "")),
      recognizer.addListener("volumechange", (event) => engine.current?.heard.volume(event.value)),
      recognizer.addListener("error", (event) => engine.current?.heard.error(event.error)),
      recognizer.addListener("end", () => engine.current?.heard.end()),
    ];
    return () => subscriptions.forEach((subscription) => subscription.remove());
  }, []);
  // Leaving the screen or going to the background ends it. Not "inactive": iOS's permission prompt makes the app that.
  useEffect(() => {
    if (!focused) close();
  }, [focused, close]);
  useEffect(() => {
    const background = AppState.addEventListener("change", (next) => next === "background" && close());
    return () => {
      background.remove();
      engine.current?.end();
    };
  }, [close]);
  // The word is announced on a change, never while it listens: the microphone would hear it.
  useEffect(() => {
    if (open && state !== "listening") AccessibilityInfo.announceForAccessibility(`${assistant}: ${VOICE_WORD[state]}`);
  }, [open, state]);

  const release = useCallback(() => engine.current?.release(Date.now() - pressedAt.current), []);
  const toggleMute = () => {
    const next = !muted;
    haptic("selection");
    engine.current?.mute(next);
    setMuted(next);
  };
  return { open, state, heard, said, problem, muted, connects, toggleMute, level, start, press, release, close };
}

/** A18 on the phone: the mark at 96 px, its word, the two captions, and Type · Talk · End at the foot. */
export function VoiceModeView({ view, theme, bottom, onType }: { view: VoiceView; theme: Theme; bottom: number; onType: () => void }) {
  const spoken = useRef<ComponentRef<typeof ScrollView>>(null);
  const { state } = view;
  const assistant = useAssistantName();
  const talkLabel = state === "listening" ? "Stop listening" : state === "speaking" ? `Talk over ${assistant}` : "Talk";
  const waiting = view.muted || state === "thinking" || state === "interrupted";
  return (
    <View style={[styles.screen, { paddingBottom: Math.max(bottom, 12) }]}>
      <View style={styles.center}>
        <VoiceMark state={state} level={view.level} size={96} theme={theme} />
        <Text style={[styles.word, { color: theme.foreground }]} accessibilityLiveRegion="polite">
          {view.muted && state === "ready" ? "Muted" : VOICE_WORD[state]}
        </Text>
      </View>
      <View style={styles.captions}>
        {view.heard ? <Text style={[styles.you, { color: theme.muted }]} numberOfLines={3}>{view.heard}</Text> : null}
        {view.said ? (
          // Nexus's words grow piece by piece as it speaks: six lines show, scrolled to the piece it says now.
          <ScrollView ref={spoken} style={styles.spoken} showsVerticalScrollIndicator={false} onContentSizeChange={() => spoken.current?.scrollToEnd({ animated: true })}>
            <Text style={[styles.nexus, { color: theme.foreground }]}>{view.said}</Text>
          </ScrollView>
        ) : null}
        {view.connects.map((hint) => (
          <ConnectCard key={hint.connector} view={connectCard(hint, assistant)} theme={theme} />
        ))}
        {view.problem ? (
          <Text style={[styles.you, { color: theme.muted }]} accessibilityRole="alert">
            {view.problem}
            {view.problem === DENIED ? (
              <Text style={[styles.link, { color: theme.foreground }]} onPress={() => void Linking.openSettings()} accessibilityRole="link">
                {" Open Settings"}
              </Text>
            ) : null}
          </Text>
        ) : null}
      </View>
      <Pressable style={styles.mute} onPress={view.toggleMute} accessibilityRole="button" accessibilityState={{ selected: view.muted }}>
        <Text style={[styles.label, { color: theme.foreground }]}>{view.muted ? "Unmute" : "Mute"}</Text>
      </Pressable>
      <View style={styles.foot}>
        <Pressable style={[styles.button, { borderColor: theme.border }]} onPress={() => { haptic("tap"); onType(); }} accessibilityRole="button" accessibilityLabel="Type">
          <Text style={[styles.label, { color: theme.foreground }]}>Type</Text>
        </Pressable>
        <Pressable
          style={({ pressed }) => [styles.talk, { borderColor: theme.border }, (pressed || state === "listening") && { backgroundColor: theme.fill }, waiting && styles.idle]}
          onPressIn={view.press}
          onPressOut={view.release}
          disabled={waiting}
          accessibilityRole="button"
          accessibilityLabel={talkLabel}
          accessibilityHint="Tap to talk and tap again to send, or hold while you talk"
          accessibilityState={{ disabled: waiting, selected: state === "listening" }}
        >
          <Icon name={state === "listening" ? "stop" : "mic"} size={22} color={theme.foreground} />
        </Pressable>
        <Pressable style={[styles.button, { backgroundColor: theme.primary, borderColor: theme.primary }]} onPress={() => { haptic("tap"); view.close(); }} accessibilityRole="button" accessibilityLabel="End voice mode">
          <Text style={[styles.label, { color: theme.onPrimary }]}>End</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, paddingHorizontal: 24 },
  center: { flex: 1, alignItems: "center", justifyContent: "center", gap: 4 },
  word: { ...text.base, fontFamily: font.medium },
  captions: { gap: 10, minHeight: 120, paddingBottom: 24 },
  you: { ...text.secondary, fontFamily: font.regular },
  nexus: { ...text.lg, fontFamily: font.regular },
  spoken: { flexGrow: 0, maxHeight: text.lg.lineHeight * 6 },
  link: { fontFamily: font.medium, textDecorationLine: "underline" },
  mute: { alignSelf: "center", minWidth: 88, minHeight: 44, alignItems: "center", justifyContent: "center" },
  foot: { flexDirection: "row", alignItems: "center", justifyContent: "space-between" },
  button: { minWidth: 88, height: 44, borderRadius: 22, borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center", paddingHorizontal: 20 },
  label: { ...text.body, fontFamily: font.medium },
  talk: { width: 64, height: 64, borderRadius: 32, borderWidth: StyleSheet.hairlineWidth, alignItems: "center", justifyContent: "center" },
  idle: { opacity: 0.4 },
});
