import type { OrgSummary } from "@nexus/kit";
import * as WebBrowser from "expo-web-browser";
import { useEffect, useState } from "react";
import { Platform, Pressable, ScrollView, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { useAuth } from "@/auth";
import { DrawerMenuRow } from "@/drawer-shell-menu";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { Busy, Initial } from "@/list";
import { pickerSections, roleLine, SEARCH_FROM } from "@/org-picker";
import { orgWaitingOn, useOrgWaitingCounts, waitingLine } from "@/org-waiting-counts";
import { orgLabel } from "@/orgs";
import { ripple } from "@/press";
import { Sheet } from "@/sheet";
import { ACCESSIBILITY_FONT_SCALE } from "@/text-size";
import { font, text, useTheme, type Theme } from "@/theme";

/**
 * Every organization you belong to, in a sheet over Settings (as the model sheet is): the ones you were in last, then
 * all of them in a fixed order, the one you are in filled and checked. From seven on, a search sits above the list and
 * stays there as it scrolls. A tap switches and closes the sheet; the touched row spins until the switch is done.
 */
type OrgSheetProps = {
  visible: boolean;
  onClose: () => void;
  orgs: OrgSummary[];
  recent?: string[];
  switching: string | null;
  problem: string | null;
  loading?: boolean;
  offline?: boolean;
  onEnrollmentClosed?: () => void;
  title?: string;
  /** Switches; resolves true once you are in it. */
  onPick: (org: OrgSummary) => Promise<boolean>;
};

export function OrgSheet(props: OrgSheetProps) {
  return (
    <Sheet visible={props.visible} onClose={props.onClose}>
      <OrgChoices {...props} />
    </Sheet>
  );
}

export function OrgChoices({ visible, onClose, orgs, recent = [], switching, problem, loading, offline, onEnrollmentClosed, title = "Switch organization", onPick }: OrgSheetProps) {
  const theme = useTheme();
  const { height } = useWindowDimensions();
  const { ui } = useAuth();
  const [query, setQuery] = useState("");
  const [enrollmentError, setEnrollmentError] = useState(false);
  // Opens on the full list each time.
  useEffect(() => {
    if (visible) {
      setQuery("");
      setEnrollmentError(false);
    }
  }, [visible]);
  const sections = pickerSections(orgs, query, recent);
  const waiting = useOrgWaitingCounts(visible);
  const enroll = async () => {
    if (!ui) return;
    haptic("tap");
    setEnrollmentError(false);
    try {
      await WebBrowser.openBrowserAsync(new URL("/app/select-org", ui).href);
      onEnrollmentClosed?.();
    } catch {
      haptic("error");
      setEnrollmentError(true);
    }
  };
  const pick = (org: OrgSummary) => {
    if (switching) return;
    if (org.current) haptic("tap");
    void onPick(org).then((done) => done && onClose());
  };

  return (
    <>
      <View style={styles.head}>
        <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">
          {title}
        </Text>
        <Pressable onPress={() => { haptic("tap"); onClose(); }} hitSlop={12} accessibilityRole="button" style={styles.done}>
          <Text style={[styles.doneText, { color: theme.muted }]}>Done</Text>
        </Pressable>
      </View>
      {orgs.length >= SEARCH_FROM ? (
        <View style={[styles.search, { backgroundColor: theme.glassFill }]}>
          <Icon name="search" size={15} color={theme.muted} />
          <TextInput
            style={[styles.searchInput, { color: theme.foreground }]}
            value={query}
            onChangeText={setQuery}
            placeholder={`Search ${orgs.length} organizations`}
            placeholderTextColor={theme.muted}
            autoCorrect={false}
            autoCapitalize="none"
            returnKeyType="search"
            clearButtonMode="while-editing"
            accessibilityLabel="Search organizations"
          />
        </View>
      ) : null}
      <ScrollView style={{ maxHeight: height * 0.62 }} keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag" alwaysBounceVertical={false}>
        {sections.map((section) => (
          <View key={section.key}>
            {section.title ? (
              <Text style={[styles.group, { color: theme.tertiary }]} accessibilityRole="header">
                {section.title}
              </Text>
            ) : null}
            {section.orgs.map((org) => (
              <OrgRow key={org.id} org={org} busy={switching === org.id} theme={theme} waiting={waitingLine(waiting, org.id)} disabled={!!offline && !org.current} onPress={() => pick(org)} />
            ))}
            {section.key === "results" && !section.orgs.length ? (
              <Text style={[styles.none, { color: theme.muted }]}>No organization matches “{query.trim()}”.</Text>
            ) : null}
          </View>
        ))}
        {waiting.kind !== "failed" && orgs.some((org) => waitingLine(waiting, org.id).dot)
          ? <Text style={[styles.none, { color: theme.muted }]}>Counts cover Gateway approvals and cloud sessions.</Text> : null}
        {orgWaitingOn && ui && !query.trim() ? (
          <View style={[styles.actions, { borderTopColor: theme.separator }]}>
            <DrawerMenuRow icon="plus" label="Create organization" detail="Opens Nexus on the web" disabled={offline} onPress={() => void enroll()} />
            <DrawerMenuRow icon="members" label="Join an organization" detail="Opens Nexus on the web" disabled={offline} onPress={() => void enroll()} />
          </View>
        ) : null}
      </ScrollView>
      {enrollmentError ? <Text accessibilityLiveRegion="polite" style={[styles.none, { color: theme.danger }]}>Nexus on the web didn’t open. Try again.</Text> : null}
      {loading ? <View style={styles.loading}><Busy /></View> : null}
      {offline ? <Text accessibilityLiveRegion="polite" style={[styles.none, { color: theme.muted }]}>Switching needs a connection. Reconnecting…</Text> : null}
      {problem && !offline ? <Text style={[styles.none, { color: theme.danger }]}>{problem}</Text> : null}
    </>
  );
}

/**
 * An organization's row: its logo or initial, its name and your role there; filled and checked when you are in it. On the
 * sheet's glass the tile and the fill are tints (`glassFill`, `glassChosen`), not the opaque fill.
 */
function OrgRow({ org, busy, disabled, theme, waiting, onPress }: { org: OrgSummary; busy: boolean; disabled: boolean; theme: Theme; waiting: ReturnType<typeof waitingLine>; onPress: () => void }) {
  const label = orgLabel(org);
  const detail = [roleLine(org), waiting.detail].filter(Boolean).join(" · ");
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, (org.current || pressed) && !ANDROID && { backgroundColor: theme.glassChosen }]}
      onPress={onPress}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityState={{ selected: !!org.current, busy, disabled }}
      accessibilityLabel={[label, detail, org.current ? "current" : undefined].filter(Boolean).join(", ")}
    >
      <Initial name={label} size={30} round={org.kind === "personal"} tint={theme.glassFill} logo={org.kind === "personal" ? undefined : org.imageUrl} />
      <View style={styles.grow}>
        <View style={styles.labelRow}>
          <Text style={[styles.label, { color: theme.foreground }]} numberOfLines={1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
            {label}
          </Text>
          {waiting.dot ? <View style={[styles.dot, { backgroundColor: theme.primary }]} /> : null}
        </View>
        {detail ? (
          <Text style={[styles.detail, { color: theme.tertiary }]} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
            {detail}
          </Text>
        ) : null}
      </View>
      <View style={styles.state}>{busy ? <Busy /> : org.current ? <Icon name="check" size={16} color={theme.primary} /> : null}</View>
    </Pressable>
  );
}

const ANDROID = Platform.OS === "android";
// The model sheet's measures (`model-sheet.tsx`): title and Done 20 pt in, rows 52 pt, the chosen one filled.
const INSET = ANDROID ? 24 : 20;
const styles = StyleSheet.create({
  head: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: INSET, paddingTop: ANDROID ? 0 : 8, paddingBottom: 6 },
  title: { ...text.body, fontFamily: font.medium },
  done: { minHeight: 44, justifyContent: "center" },
  doneText: { ...text.body, fontFamily: font.regular },
  search: { flexDirection: "row", alignItems: "center", gap: 8, marginHorizontal: 16, marginBottom: 4, minHeight: 40, paddingHorizontal: 12, borderRadius: 12, borderCurve: "continuous" },
  searchInput: { ...text.body, fontFamily: font.regular, flex: 1, paddingVertical: 8 },
  group: { ...text.caption, fontFamily: font.regular, paddingHorizontal: INSET, paddingTop: 12, paddingBottom: 4 },
  row: { flexDirection: "row", alignItems: "center", gap: 12, minHeight: ANDROID ? 56 : 52, paddingHorizontal: ANDROID ? INSET : 14, marginHorizontal: ANDROID ? 0 : 6, borderRadius: 12, borderCurve: "continuous" },
  grow: { flex: 1, minWidth: 0 },
  labelRow: { flexDirection: "row", alignItems: "center", gap: 6 },
  label: { ...text.base, fontFamily: font.regular, flexShrink: 1 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  detail: { ...text.caption, fontFamily: font.regular },
  state: { width: 24, height: 24, alignItems: "center", justifyContent: "center" },
  actions: { borderTopWidth: StyleSheet.hairlineWidth, marginTop: 8, paddingTop: 4 },
  loading: { paddingVertical: 16, alignItems: "center" },
  none: { ...text.secondary, fontFamily: font.regular, paddingHorizontal: INSET, paddingVertical: 12 },
});
