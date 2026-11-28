import { errandName, errandsNeedingYou, type AgentClient, type AgentErrand } from "@nexus/kit";
import { router } from "expo-router";
import { useEffect, useState } from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { chipDetail, chipLabel, errandLine, errandsOn, OVERVIEW_ROWS, overviewTitle, stripAfter, type Strip } from "@/agent-errands";
import { AssistantMark } from "@/assistant-mark";
import { errandHref } from "@/drawer-places";
import { useErrandMenu } from "@/errand-menu";
import { ErrandShape } from "@/errand-shape";
import { haptic } from "@/haptics";
import { pressedFill, ripple } from "@/press";
import { RowMenu } from "@/row-menu";
import { font, text, useTheme, type Theme } from "@/theme";

/**
 * Errands on top of the Assistant (A12), behind `EXPO_PUBLIC_NEXUS_AGENT_ERRANDS=on`: the main chat stays the
 * overview, with the errands that need the person and those that run in a row of pills above it, each with its state
 * by shape (A2·A): a filled dot needs you, a turning arc runs. A tap opens the errand's own thread (`errand-thread.tsx`),
 * and a long press its menu: Pin, Rename, Mark done and Archive. When any errand waits on the person, the
 * chat then opens with "2 errands need you" and a row per errand (A12·c). Off, or an agent API with errands off or
 * failing, nothing is drawn and the screen is as before. Names and goals are the person's own text, drawn as plain text.
 */
const available = errandsOn(process.env.EXPO_PUBLIC_NEXUS_AGENT_ERRANDS);

/** `revision` changes when an answer ends, so the strip reads the list again. */
export function ErrandsStrip({ agent, revision }: { agent: AgentClient; revision: number }) {
  return available ? <ErrandsOn agent={agent} revision={revision} /> : null;
}

const openErrand = (id: string) => {
  haptic("tap");
  router.push(errandHref(id));
};

function ErrandsOn({ agent, revision }: { agent: AgentClient; revision: number }) {
  const theme = useTheme();
  const [strip, setStrip] = useState<Strip>({ kind: "unknown" });
  const [reads, setReads] = useState(0);
  const menu = useErrandMenu(agent, () => setReads((count) => count + 1));

  useEffect(() => {
    const controller = new AbortController();
    agent
      .errands(controller.signal)
      .catch(() => "unreachable" as const)
      .then((read) => !controller.signal.aborted && setStrip(stripAfter(read)));
    return () => controller.abort();
  }, [agent, revision, reads]);

  if (strip.kind !== "on") return menu.sheet;
  const needing = errandsNeedingYou(strip.errands);
  return (
    <>
      <View style={styles.strip}>
        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.chips} accessibilityLabel="Your errands">
          {strip.errands.map((errand) => {
            const detail = chipDetail(errand);
            return (
              <RowMenu key={errand.id} title={errandName(errand)} actions={menu.actions(errand)}>
                {(onLongPress) => (
                  <Pressable
                    android_ripple={ripple(theme)}
                    style={({ pressed }) => [styles.chip, { borderColor: theme.border }, pressed && pressedFill(theme)]}
                    onPress={() => openErrand(errand.id)}
                    onLongPress={onLongPress}
                    accessibilityRole="button"
                    accessibilityLabel={chipLabel(errand)}
                  >
                    <ErrandShape status={errand.status} color={theme.foreground} />
                    <Text style={[styles.chipGoal, { color: theme.foreground }]} numberOfLines={1}>
                      {errandName(errand)}
                    </Text>
                    {detail ? (
                      <Text style={[styles.chipGoal, { color: theme.tertiary }]} numberOfLines={1}>
                        {detail}
                      </Text>
                    ) : null}
                  </Pressable>
                )}
              </RowMenu>
            );
          })}
        </ScrollView>
      </View>
      <Overview errands={needing} theme={theme} />
      {menu.sheet}
    </>
  );
}

/**
 * The main chat's overview (A12·c): "2 errands need you", then one row per errand with its name and what it needs, at
 * most three. A row opens that errand's thread. None waits on the person: nothing is drawn.
 */
function Overview({ errands, theme }: { errands: AgentErrand[]; theme: Theme }) {
  const title = overviewTitle(errands.length);
  if (!title) return null;
  return (
    <View style={styles.overview}>
      <View style={styles.avatar}>
        <AssistantMark size={28} />
      </View>
      <View style={[styles.card, { backgroundColor: theme.fill }]} accessibilityLabel={title}>
        <Text style={[styles.cardTitle, { color: theme.foreground }]} accessibilityRole="header">
          {title}
        </Text>
        {errands.slice(0, OVERVIEW_ROWS).map((errand, i) => (
          <Pressable
            key={errand.id}
            android_ripple={ripple(theme)}
            style={({ pressed }) => [styles.cardRow, i > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderColor: theme.border }, pressed && pressedFill(theme)]}
            onPress={() => openErrand(errand.id)}
            accessibilityRole="button"
            accessibilityLabel={`${errandName(errand)}, ${errandLine(errand)}`}
          >
            <ErrandShape status="needs-you" color={theme.foreground} />
            <View style={styles.cardText}>
              <Text style={[styles.cardName, { color: theme.foreground }]} numberOfLines={1}>
                {errandName(errand)}
              </Text>
              <Text style={[styles.cardLine, { color: theme.tertiary }]} numberOfLines={1}>
                {errandLine(errand)}
              </Text>
            </View>
          </Pressable>
        ))}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  // A2·A's errand pills: 34 pt high and fully round, a hairline and no fill, 14 pt words, 7 pt from the state's 8 pt
  // shape, 8 pt apart, 16 pt from the screen's sides.
  strip: { paddingTop: 8, paddingBottom: 6 },
  chips: { gap: 8, paddingHorizontal: 16 },
  chip: { flexDirection: "row", alignItems: "center", gap: 7, maxWidth: 260, height: 34, paddingHorizontal: 12, borderRadius: 17, borderWidth: StyleSheet.hairlineWidth },
  chipGoal: { fontSize: 14, lineHeight: 19, fontFamily: font.regular, flexShrink: 1 },
  // The overview sits on Nexus's side, as a reply does: the 28 pt mark, then the card.
  overview: { flexDirection: "row", alignItems: "flex-end", gap: 8, marginTop: 8, marginBottom: 12 },
  avatar: { width: 28 },
  card: { flex: 1, maxWidth: 300, borderRadius: 18, borderCurve: "continuous", overflow: "hidden", paddingTop: 10 },
  cardTitle: { ...text.sm, fontFamily: font.medium, paddingHorizontal: 14, paddingBottom: 6 },
  cardRow: { flexDirection: "row", alignItems: "center", gap: 10, minHeight: 44, paddingHorizontal: 14, paddingVertical: 8 },
  cardText: { flex: 1 },
  cardName: { ...text.sm, fontFamily: font.regular },
  cardLine: { ...text.caption, fontFamily: font.regular },
});
