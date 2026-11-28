import { router } from "expo-router";
import { useEffect, useRef, useState, type ComponentProps } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";
import { activityChips, connectorLabel, type Chip } from "@/activity";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Row } from "@/list";
import { open } from "@/markdown";
import { PressScale } from "@/press-scale";
import { Sheet, SheetTitle } from "@/sheet";
import { font, mono, text, type Theme } from "@/theme";

const ICON: Record<Chip["kind"], ComponentProps<typeof Icon>["name"]> = { skill: "skill", connector: "connector", web: "globe", page: "pages" };
const SCOPE = { personal: "Yours", team: "A team's", org: "Your organization's" } as const;
const SOURCE_PREVIEW_COUNT = 3;
const BROWSER_AFTER_SHEET_MS = 450;

type Message = Parameters<typeof activityChips>[0];

/**
 * An answer's calls as chips under it (brand v5's P38 on the phone): a tap opens the call's detail in a sheet, and the
 * page chip opens the page. Nothing shows for an answer that made no call.
 */
export function ActivityChips({ message, theme }: { message: Message; theme: Theme }) {
  const chips = activityChips(message);
  const [shown, setShown] = useState<Chip | null>(null);
  const opening = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (opening.current) clearTimeout(opening.current);
  }, []);
  if (!chips.length) return null;
  const visit = (url: string) => {
    if (opening.current) return;
    haptic("tap");
    setShown(null);
    // Close this sheet before iOS presents the system browser.
    opening.current = setTimeout(() => {
      opening.current = null;
      open(url);
    }, BROWSER_AFTER_SHEET_MS);
  };
  const button = (chip: Chip, label = chip.label, key = chip.key) => (
    <PressScale
      key={key}
      style={[styles.chip, { backgroundColor: theme.fill }]}
      hostStyle={styles.host}
      onPress={() => {
        haptic("tap");
        if (chip.kind === "page") router.push({ pathname: "/page/[id]", params: { id: chip.page.pageId } });
        else setShown(chip);
      }}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityHint={chip.kind === "page" ? "Opens the page" : chip.kind === "web" ? `Shows all ${chip.label}` : "Shows what it did"}
      hitSlop={{ top: 9, bottom: 9 }}
    >
      <Icon name={ICON[chip.kind]} size={12} color={theme.muted} />
      <Text style={[styles.chipText, { color: theme.muted }]} numberOfLines={1}>{label}</Text>
    </PressScale>
  );
  return (
    <View style={styles.chips}>
      {chips.flatMap((chip) => chip.kind === "web"
        ? [...new Set(chip.sources.map((source) => source.host))].slice(0, SOURCE_PREVIEW_COUNT).map((host) => button(chip, host, `web:${host}`))
        : [button(chip)])}
      <Sheet visible={!!shown} onClose={() => setShown(null)}>
        {shown ? <Detail chip={shown} theme={theme} onVisit={visit} /> : null}
      </Sheet>
    </View>
  );
}

/** A call's detail: what ran and with what. */
function Detail({ chip, theme, onVisit }: { chip: Chip; theme: Theme; onVisit: (url: string) => void }) {
  if (chip.kind === "skill") {
    return (
      <>
        <SheetTitle>{chip.label}</SheetTitle>
        <Row label="Version" value={`v${chip.skill.version}`} monoValue />
        <Row label="From" value={SCOPE[chip.skill.scope ?? "personal"]} />
      </>
    );
  }
  if (chip.kind === "connector") {
    return (
      <>
        <SheetTitle>{`Read ${connectorLabel(chip.connector)}, as you, with your organization's access`}</SheetTitle>
        <ScrollView style={styles.scroll} contentContainerStyle={styles.reads}>
          {chip.reads.map((read, n) => (
            <View key={n} style={styles.read}>
              <Text style={[styles.operation, { color: theme.foreground }]}>{read.operation}</Text>
              <Text style={[styles.input, { color: theme.muted, backgroundColor: theme.fill }]} numberOfLines={12} selectable>
                {JSON.stringify(read.input, null, 2)}
              </Text>
            </View>
          ))}
        </ScrollView>
      </>
    );
  }
  if (chip.kind === "web") {
    return (
      <>
        <SheetTitle>{chip.label}</SheetTitle>
        <ScrollView style={styles.scroll}>
          {chip.sources.map((source, n) => (
            <Row key={n} label={source.title || source.host} detail={`${source.host} · “${source.query}”`} onPress={() => onVisit(source.url)} accessibilityRole="link" />
          ))}
        </ScrollView>
      </>
    );
  }
  return null;
}

const styles = StyleSheet.create({
  chips: { flexDirection: "row", flexWrap: "wrap", gap: 6, marginTop: 10 },
  host: { maxWidth: "100%" },
  chip: { flexDirection: "row", alignItems: "center", gap: 5, height: 26, paddingHorizontal: 10, borderRadius: 13 },
  chipText: { ...text.caption, fontFamily: font.regular, flexShrink: 1 },
  scroll: { maxHeight: 420 },
  reads: { paddingHorizontal: 16, paddingBottom: 8, gap: 12 },
  read: { gap: 6 },
  operation: { fontFamily: mono, fontSize: 14 },
  input: { fontFamily: mono, fontSize: 12, lineHeight: 17, padding: 10, borderRadius: 8, overflow: "hidden" },
});
