import { StyleSheet, Text, View } from "react-native";
import Svg, { Circle } from "react-native-svg";
import { CONTEXT_NEAR_FULL_AT, CONTEXT_WARNING_AT, tokens, type contextUse } from "@/context";
import { haptic } from "@/haptics";
import { Footnote, Row } from "@/list";
import { PressScale } from "@/press-scale";
import { Sheet, SheetRow } from "@/sheet";
import { font, mono, text, type Theme } from "@/theme";

type Use = NonNullable<ReturnType<typeof contextUse>>;

const SIZE = 16;
const STROKE = 2;
const R = (SIZE - STROKE) / 2;
const AROUND = 2 * Math.PI * R;

export function ContextRing({ use, model, theme, visible, onOpen, onClose, onLarger }: {
  use: Use | undefined;
  model: string;
  theme: Theme;
  visible: boolean;
  onOpen: () => void;
  onClose: () => void;
  onLarger?: () => void;
}) {
  const percent = Math.round((use?.share ?? 0) * 100);
  const status = use && use.share >= CONTEXT_WARNING_AT ? (use.share >= 1 ? "Context full" : "Context filling up") : "Context";
  const ink = use && use.share >= CONTEXT_WARNING_AT ? theme.attention : theme.muted;

  return (
    <>
      {use ? <PressScale
        onPress={() => { haptic("tap"); onOpen(); }}
        style={styles.ring}
        accessibilityRole="button"
        accessibilityLabel={`Context ${percent}% full`}
        accessibilityHint="Shows token usage from the latest answer"
      >
        <Svg width={SIZE} height={SIZE} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
          <Circle cx={SIZE / 2} cy={SIZE / 2} r={R} stroke={theme.fill} strokeWidth={STROKE} fill="none" />
          <Circle cx={SIZE / 2} cy={SIZE / 2} r={R} stroke={ink} strokeWidth={STROKE} fill="none" strokeLinecap="round"
            strokeDasharray={`${AROUND * use.share} ${AROUND}`} transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`} />
        </Svg>
      </PressScale> : null}
      <Sheet visible={visible} onClose={onClose}>
        <View style={styles.head}>
          <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">{status}</Text>
          {use ? <Text style={[styles.count, { color: ink }]}>{percent}%</Text> : null}
        </View>
        {use ? <>
          <View style={styles.meter}>
            <View style={[styles.track, { backgroundColor: theme.fill }]}>
              <View style={[styles.track, { width: `${percent}%`, backgroundColor: ink }]} />
            </View>
            <Text style={[styles.count, { color: theme.muted }]}>{tokens(use.used)} of {tokens(use.window)} tokens</Text>
          </View>
          <Row label="Input" value={tokens(use.input)} monoValue />
          <Row label="Output" value={tokens(use.output)} monoValue />
          <Footnote>{`The latest answer’s usage on ${model}.`}</Footnote>
          {onLarger && use.share >= CONTEXT_NEAR_FULL_AT ? (
            <SheetRow label="Larger context…" onPress={() => { haptic("tap"); onLarger(); }} />
          ) : null}
        </> : <Footnote>Context usage is available after an answer reports token counts.</Footnote>}
        <View style={styles.foot} />
      </Sheet>
    </>
  );
}

const styles = StyleSheet.create({
  ring: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  head: { flexDirection: "row", alignItems: "baseline", justifyContent: "space-between", paddingHorizontal: 16, paddingTop: 12 },
  title: { ...text.body, fontFamily: font.medium },
  count: { fontFamily: mono, fontSize: 13, fontVariant: ["tabular-nums"] },
  meter: { paddingHorizontal: 16, paddingTop: 12, gap: 8, paddingBottom: 6 },
  track: { height: 6, borderRadius: 3, overflow: "hidden" },
  foot: { height: 8 },
});
