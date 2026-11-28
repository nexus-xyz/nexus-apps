import { View } from "react-native";
import { useSharedValue } from "react-native-reanimated";
import { NexusMark } from "@/nexus-mark";
import { useTheme } from "@/theme";
import { VoiceMark } from "@/voice-mark";

/** The voice mark keeps room for its ring, 1.7 times the mark (`voice-mark.tsx`); here the ring never shows. */
const RING = 1.7;

/**
 * The Assistant's mark at `size` points: still, or thinking while an answer streams. Thinking is the voice mark's
 * (A13, A25): a slow breath from the eye out and one turn in 14 s; Reduce Motion holds it still at a lower light.
 * The drawer's row draws it at 18 (A21·c), the thread's avatar at 28 (A1·D). It only decorates.
 */
export function AssistantMark({ size, thinking }: { size: number; thinking?: boolean }) {
  return (
    <View style={{ width: size, height: size }} accessibilityElementsHidden importantForAccessibility="no-hide-descendants" pointerEvents="none">
      {thinking ? <Thinking size={size} /> : <NexusMark size={size} />}
    </View>
  );
}

function Thinking({ size }: { size: number }) {
  const theme = useTheme();
  const level = useSharedValue(0);
  const offset = (size - size * RING) / 2;
  return (
    <View style={{ position: "absolute", left: offset, top: offset }}>
      <VoiceMark state="thinking" level={level} size={size} theme={theme} />
    </View>
  );
}
