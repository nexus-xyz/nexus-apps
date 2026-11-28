import type { AgentBrief, AgentClient, AgentProactivityView, AgentWhy } from "@nexus/kit";
import { useEffect, useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { NexusHeader, NexusSide } from "@/assistant-bubbles";
import { briefLabel, briefTime, whyActions } from "@/brief-words";
import { haptic } from "@/haptics";
import { Sheet } from "@/sheet";
import { SheetRow, SheetTitle } from "@/sheet-rows";
import { font, text, useTheme, type Theme } from "@/theme";

/**
 * The morning brief at the top of today's thread (A1·A). Nexus writes it first, so it carries the small
 * "Nexus · 08:02" header (the Assistant canvas). Then the greeting and up to three lines as plain text, and the
 * errands that need the person beside a filled dot. Under it "Why am I seeing this?" (A10·b) when the agent API says
 * why: a sheet with the reasons, Fewer like this and Turn off. Each read is quiet: off, none today or a failure shows
 * no brief.
 */

/** Today's brief, read while `wanted`, and again when `revision` changes. Undefined while there is none to show. */
export function useTodayBrief(agent: AgentClient, wanted: boolean, revision = 0): AgentBrief | undefined {
  const [brief, setBrief] = useState<AgentBrief | undefined>();
  useEffect(() => {
    if (!wanted) return;
    const controller = new AbortController();
    agent.brief(controller.signal).then(
      (read) => !controller.signal.aborted && setBrief(read.kind === "brief" ? read.brief : undefined),
      () => undefined,
    );
    return () => controller.abort();
  }, [agent, wanted, revision]);
  return wanted ? brief : undefined;
}

export function BriefTurn({ brief, agent, theme }: { brief: AgentBrief; agent: AgentClient; theme: Theme }) {
  const [why, setWhy] = useState(false);
  return (
    <>
      <NexusSide>
        <View style={styles.block}>
          <View style={styles.brief} accessible accessibilityLabel={`Today's brief. ${briefLabel(brief)}`}>
            <NexusHeader when={briefTime(brief)} theme={theme} />
            <Text style={[styles.words, { color: theme.foreground }]}>{brief.head}</Text>
            {brief.lines.map((line) => (
              <Text key={line} style={[styles.words, { color: theme.foreground }]}>
                {"• "}
                {line}
              </Text>
            ))}
            {brief.errands ? (
              <View style={styles.errands}>
                <View style={[styles.dot, { backgroundColor: theme.foreground }]} />
                <Text style={[styles.words, styles.errandsText, { color: theme.foreground }]}>{brief.errands}</Text>
              </View>
            ) : null}
          </View>
          {brief.why ? (
            <Pressable
              onPress={() => {
                haptic("tap");
                setWhy(true);
              }}
              accessibilityRole="button"
              hitSlop={8}
              style={styles.whyHost}
            >
              <Text style={[styles.why, { color: theme.muted }]}>Why am I seeing this?</Text>
            </Pressable>
          ) : null}
        </View>
      </NexusSide>
      {brief.why ? <WhySheet why={brief.why} agent={agent} visible={why} onClose={() => setWhy(false)} /> : null}
    </>
  );
}

/** "Why am I seeing this?" (A10·b): the reasons, then Fewer like this and Turn off, read from the person's Proactivity. */
function WhySheet({ why, agent, visible, onClose }: { why: AgentWhy; agent: AgentClient; visible: boolean; onClose: () => void }) {
  const theme = useTheme();
  const [view, setView] = useState<AgentProactivityView | undefined>();
  const [done, setDone] = useState<string>();
  const [working, setWorking] = useState(false);
  useEffect(() => {
    if (!visible) return;
    setDone(undefined);
    const controller = new AbortController();
    agent.proactivity(controller.signal).then(
      (read) => !controller.signal.aborted && setView(read.kind === "view" ? read.view : undefined),
      () => undefined,
    );
    return () => controller.abort();
  }, [agent, visible]);
  const act = (action: ReturnType<typeof whyActions>[number]) => {
    if (!view || working) return;
    setWorking(true);
    agent.setProactivity(action.change(view)).then(
      (read) => {
        setWorking(false);
        if (read.kind !== "view") return setDone("That did not change. Try again in Account › Assistant › Proactivity.");
        setView(read.view);
        setDone(`${action.done(read.view)} Change it in Account › Assistant › Proactivity.`);
        haptic("success");
      },
      () => {
        setWorking(false);
        setDone("Nexus could not reach the Assistant. Try again in a moment.");
      },
    );
  };
  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetTitle>Why am I seeing this?</SheetTitle>
      {why.lines.map((line) => (
        <Text key={line} style={[sheetStyles.line, { color: theme.foreground }]}>
          {line}
        </Text>
      ))}
      {done ? <Text style={[sheetStyles.done, { color: theme.muted }]} accessibilityLiveRegion="polite">{done}</Text> : null}
      {view && !done ? whyActions(why.kind).map((action) => <SheetRow key={action.label} label={working ? "Saving…" : action.label} onPress={() => act(action)} />) : null}
      <SheetRow label="Close" tone="muted" onPress={onClose} />
    </Sheet>
  );
}

// The brief keeps the transcript's metrics (`assistant-bubbles.tsx`): 17/24 prose, 4 pt in from the sides, 20 pt
// before the next speaker.
const styles = StyleSheet.create({
  block: { marginBottom: 20 },
  brief: { gap: 2, paddingHorizontal: 4 },
  words: { ...text.reading, fontFamily: font.regular },
  errands: { flexDirection: "row", alignItems: "center", gap: 8, marginTop: 4 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  errandsText: { flexShrink: 1 },
  whyHost: { alignSelf: "flex-start", paddingLeft: 4, paddingTop: 6, paddingBottom: 4 },
  why: { fontSize: 12.5, lineHeight: 17, fontFamily: font.regular, textDecorationLine: "underline" },
});

const sheetStyles = StyleSheet.create({
  line: { ...text.base, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 4 },
  done: { ...text.sm, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 8 },
});
