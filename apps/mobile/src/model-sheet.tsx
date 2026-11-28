import { useEffect, useMemo, useState } from "react";
import { Platform, Pressable, ScrollView, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { TextInput } from "@/controls";
import { contextLabel, findModels, shortList } from "@/current-models";
import { Icon } from "@/icon";
import { haptic } from "@/haptics";
import { AUTO } from "@/meta";
import type { Model } from "@/models";
import { freeStartPays } from "@/plan";
import { pressedFill, ripple } from "@/press";
import { provider, providerOf, providerRank } from "@/providers";
import { ProviderMark } from "@/provider-mark";
import { Sheet, SheetRow } from "@/sheet";
import { font, text, useTheme, type Theme } from "@/theme";

/**
 * The model picker on the phone (brand v5's P32·F): a sheet, not a menu. At half height it holds Auto, three models
 * (the one in use, the organization's default, then the latest of each line: `shortList`) and All models; All models fills the sheet with a search and every model by provider. A
 * pick closes it. The design's scopes (Plans, the account's keys, Gateway) and Request wait on the Gateway saying
 * which source serves a model; until then the groups are the providers, as the menu's were. On the free start, a line
 * under the title says which models it pays for, and each of them says "Free start".
 */
export function ModelSheet({
  visible,
  models,
  latest,
  selected,
  onPick,
  onClose,
  freeStartMaxUsd,
  appliesNextTurn = false,
  largerThan,
}: {
  visible: boolean;
  /** Every model you may use, Auto first. */
  models: Model[];
  /** The latest of each line, Auto and the organization's default among them. */
  latest: Model[];
  selected: string;
  onPick: (model: string) => void;
  onClose: () => void;
  /** Set while the organization is on its free start: the highest output price per million tokens it pays for. */
  freeStartMaxUsd?: number;
  appliesNextTurn?: boolean;
  largerThan?: number;
}) {
  const theme = useTheme();
  const { height } = useWindowDimensions();
  const [view, setView] = useState<"recommended" | "search" | "filters">("recommended");
  const [detail, setDetail] = useState<Model>();
  const [maker, setMaker] = useState("");
  const [capabilities, setCapabilities] = useState<Capability[]>([]);
  const [query, setQuery] = useState("");
  // Opens at half height each time, as the board draws it.
  useEffect(() => {
    if (!visible) return;
    setView(largerThan === undefined ? "recommended" : "search");
    setDetail(undefined);
    setMaker("");
    setCapabilities([]);
    setQuery("");
  }, [largerThan, visible]);
  const pick = (id: string) => {
    if (id !== selected) {
      haptic("selection");
      onPick(id);
    }
    onClose();
  };
  const auto = models.find((m) => m.id === AUTO);
  const named = useMemo(() => models.filter((model) => model.id !== AUTO &&
    (largerThan === undefined || (model.allowed !== false && (model.context_window ?? 0) > largerThan))), [largerThan, models]);
  const makers = useMemo(() => byProvider(named).map(([id]) => id), [named]);
  const matching = useMemo(() => findModels(named, query).filter((model) =>
    (!maker || providerOf(model.id) === maker) && capabilities.every((key) => CAPABILITIES[key].matches(model))), [named, query, maker, capabilities]);
  const groups = useMemo(() => byProvider(matching), [matching]);
  const free = (m: Model) => freeStartMaxUsd !== undefined && freeStartPays(m, freeStartMaxUsd) === true;

  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.head}>
        <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">
          {detail ? "Model details" : view === "filters" ? "Filters" : view === "search" ? (largerThan === undefined ? "Search models" : "Larger context") : "Model"}
        </Text>
        <Pressable onPress={onClose} hitSlop={12} accessibilityRole="button" style={styles.done}>
          <Text style={[styles.doneText, { color: theme.muted }]}>Done</Text>
        </Pressable>
      </View>
      {detail ? (
        <ModelDetails model={detail} free={free(detail)} onBack={() => { haptic("tap"); setDetail(undefined); }} onPick={() => pick(detail.id)} />
      ) : view === "filters" ? (
        <>
          <SheetRow label="Back to search" onPress={() => { haptic("tap"); setView("search"); }} />
          <ScrollView style={{ maxHeight: height * 0.55 }}>
            <Text style={[styles.group, { color: theme.tertiary }]} accessibilityRole="header">Capabilities</Text>
            {CAPABILITY_KEYS.map((key) => (
              <FilterRow key={key} label={CAPABILITIES[key].label} active={capabilities.includes(key)} theme={theme}
                onPress={() => {
                  haptic("selection");
                  setCapabilities((values) => values.includes(key) ? values.filter((value) => value !== key) : [...values, key]);
                }} />
            ))}
            <Text style={[styles.group, { color: theme.tertiary }]} accessibilityRole="header">Maker</Text>
            <FilterRow label="All makers" active={!maker} theme={theme} onPress={() => { haptic("selection"); setMaker(""); }} />
            {makers.map((id) => <FilterRow key={id} label={provider(id).name} active={maker === id} theme={theme}
              onPress={() => { haptic("selection"); setMaker(id); }} />)}
          </ScrollView>
          <SheetRow label={`Show ${matching.length} ${matching.length === 1 ? "model" : "models"}`} tone="primary"
            onPress={() => { haptic("tap"); setView("search"); }} />
        </>
      ) : <>
      {appliesNextTurn ? <Text style={[styles.note, { color: theme.muted }]}>A different model applies from the next turn.</Text> : null}
      {freeStartMaxUsd !== undefined ? (
        <Text style={[styles.note, { color: theme.muted }]}>Your free start pays for Auto and the models marked Free start. Other models need pay as you go.</Text>
      ) : null}
      {view === "search" ? (
        <>
          <View style={[styles.search, { backgroundColor: theme.fill }]}>
            <Icon name="search" size={15} color={theme.muted} />
            <TextInput
              style={[styles.searchInput, { color: theme.foreground }]}
              value={query}
              onChangeText={setQuery}
              placeholder={`Search ${named.length} models`}
              placeholderTextColor={theme.muted}
              autoCorrect={false}
              autoCapitalize="none"
              accessibilityLabel="Search models"
            />
          </View>
          <View style={styles.summary}>
            <Text style={[styles.noteText, { color: theme.muted }]} accessibilityLiveRegion="polite">
              {matching.length} {matching.length === 1 ? "model" : "models"}
            </Text>
            <Pressable style={styles.done} onPress={() => { haptic("tap"); setView("filters"); }} accessibilityRole="button">
              <Text style={[styles.noteText, { color: theme.foreground }]}>Filters{maker || capabilities.length ? ` · ${Number(!!maker) + capabilities.length}` : ""}</Text>
            </Pressable>
          </View>
          <ScrollView style={{ maxHeight: height * 0.48 }} keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag">
            {groups.map(([id, list]) => (
              <View key={id}>
                <Text style={[styles.group, { color: theme.tertiary }]} accessibilityRole="header">
                  {provider(id).name}
                </Text>
                {list.map((m) => (
                  <ModelRow key={m.id} model={m} free={free(m)} selected={m.id === selected} theme={theme} onPress={() => { haptic("tap"); setDetail(m); }} />
                ))}
              </View>
            ))}
            {groups.length ? null : <Text style={[styles.none, { color: theme.muted }]}>
              {query.trim() ? `No models match “${query.trim()}”. Try a model name or maker.` : "No models match these filters."}
            </Text>}
          </ScrollView>
        </>
      ) : (
        <>
          {auto ? (
            <ModelRow
              model={auto}
              detail={freeStartMaxUsd !== undefined ? "Free start · Picks a model for each question" : "Picks a model for each question"}
              selected={selected === AUTO}
              theme={theme}
              onPress={() => pick(AUTO)}
            />
          ) : null}
          <Text style={[styles.group, { color: theme.tertiary }]} accessibilityRole="header">
            Latest
          </Text>
          {shortList(
            latest.filter((m) => m.id !== AUTO),
            selected,
            (m) => !!m.default_for?.includes("chat"),
          ).map((m) => (
            <ModelRow key={m.id} model={m} free={free(m)} selected={m.id === selected} theme={theme} onPress={() => pick(m.id)} />
          ))}
          <View style={[styles.rule, { backgroundColor: theme.separator }]} />
          <Pressable
            android_ripple={ripple(theme)}
            style={({ pressed }) => [styles.row, pressed && pressedFill(theme)]}
            onPress={() => { haptic("tap"); setView("search"); }}
            accessibilityRole="button"
            accessibilityLabel={`Search all ${named.length} models`}
          >
            <View style={styles.mark}>
              <Icon name="products" size={16} color={theme.muted} />
            </View>
            <Text style={[styles.label, styles.grow, { color: theme.foreground }]}>{named.length ? `Search all ${named.length} models` : "Search models"}</Text>
            <Icon name="forward" size={13} color={theme.tertiary} />
          </Pressable>
        </>
      )}
      </>}
    </Sheet>
  );
}

/**
 * A model's row: its mark, its name, what it is (that the free start pays for it, the organization's default, its
 * context), checked when chosen.
 */
function ModelRow({ model, detail, free, selected, theme, onPress }: { model: Model; detail?: string; free?: boolean; selected: boolean; theme: Theme; onPress: () => void }) {
  const line =
    detail ??
    [model.allowed === false ? "Restricted by policy" : free ? "Free start" : undefined, model.default_for?.includes("chat") ? "Your organization's default" : undefined, contextLabel(model.context_window)].filter(Boolean).join(" · ");
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, selected && !ANDROID && { backgroundColor: theme.rowPressed }, pressed && pressedFill(theme)]}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityState={{ selected }}
      accessibilityLabel={[model.name, line].filter(Boolean).join(", ")}
    >
      <View style={styles.mark}>
        <ProviderMark model={model.id} size={18} />
      </View>
      <View style={styles.grow}>
        <Text style={[styles.label, { color: theme.foreground }]} numberOfLines={1}>
          {model.name}
        </Text>
        {line ? (
          <Text style={[styles.detail, { color: theme.tertiary }]} numberOfLines={1}>
            {line}
          </Text>
        ) : null}
      </View>
      {selected ? <Icon name="check" size={16} color={theme.primary} /> : null}
    </Pressable>
  );
}

const CAPABILITIES = {
  vision: { label: "Vision", matches: (model: Model) => model.input?.includes("image") === true },
  reasoning: { label: "Reasoning", matches: (model: Model) => !!model.reasoning && model.reasoning.mode !== "none" },
  longContext: { label: "Long context", matches: (model: Model) => (model.context_window ?? 0) >= 128_000 },
};
type Capability = keyof typeof CAPABILITIES;
const CAPABILITY_KEYS: Capability[] = ["vision", "reasoning", "longContext"];

function FilterRow({ label, active, theme, onPress }: { label: string; active: boolean; theme: Theme; onPress: () => void }) {
  return (
    <Pressable android_ripple={ripple(theme)} style={({ pressed }) => [styles.row, pressed && pressedFill(theme)]}
      accessibilityRole="checkbox" accessibilityState={{ checked: active }} onPress={onPress}>
      <Text style={[styles.label, styles.grow, { color: theme.foreground }]}>{label}</Text>
      {active ? <Icon name="check" size={16} color={theme.primary} /> : null}
    </Pressable>
  );
}

function ModelDetails({ model, free, onBack, onPick }: { model: Model; free: boolean; onBack: () => void; onPick: () => void }) {
  const theme = useTheme();
  const allowed = model.allowed !== false;
  const context = contextLabel(model.context_window);
  const maxOutput = contextLabel(model.max_output_tokens);
  const reasoning = model.reasoning;
  return (
    <>
      <SheetRow label="Back to search" onPress={onBack} />
      <View style={styles.modelDetail}>
        <View style={styles.modelTitle}>
          <ProviderMark model={model.id} size={24} />
          <View style={styles.grow}>
            <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header">{model.name}</Text>
            <Text style={[styles.detail, { color: theme.muted }]}>{provider(model.owned_by).name}</Text>
          </View>
        </View>
        {model.note ? <Text style={[styles.noteText, { color: theme.muted }]}>{model.note}</Text> : null}
        {context ? <Text style={[styles.noteText, { color: theme.foreground }]}>Context window · {context} tokens</Text> : null}
        {maxOutput ? <Text style={[styles.noteText, { color: theme.foreground }]}>Maximum output · {maxOutput} tokens</Text> : null}
        {model.input?.length ? <Text style={[styles.noteText, { color: theme.foreground }]}>Inputs · {model.input.map((input) => input === "image" ? "Images" : "Text").join(", ")}</Text> : null}
        {reasoning?.efforts.length ? <Text style={[styles.noteText, { color: theme.foreground }]}>Effort · {reasoning.efforts.join(", ")}</Text> : null}
        {free && allowed ? <Text style={[styles.noteText, { color: theme.foreground }]}>Covered by free start</Text> : null}
        {allowed ? null : <Text style={[styles.noteText, { color: theme.muted }]}>This model is restricted by policy. Ask an administrator for access.</Text>}
      </View>
      {allowed ? <SheetRow label={`Use ${model.name}`} tone="primary" onPress={onPick} /> : null}
    </>
  );
}

/** Models by provider, in Nexus UI's order of providers. */
function byProvider(models: readonly Model[]): [string, Model[]][] {
  const groups = new Map<string, Model[]>();
  for (const m of models) {
    const id = providerOf(m.id);
    groups.set(id, [...(groups.get(id) ?? []), m]);
  }
  return [...groups].sort(([a], [b]) => providerRank(a) - providerRank(b) || a.localeCompare(b));
}

const ANDROID = Platform.OS === "android";
// The board's sheet (P32·F): its title and Done 20 pt in; rows 52 pt with an 18 pt mark, the name 16 pt and what it
// is 13 pt under it; the chosen row filled, checked. Android keeps Material's list items, 24 dp in.
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
  mark: { width: 22, alignItems: "center" },
  grow: { flex: 1 },
  label: { ...text.base, fontFamily: font.regular },
  detail: { ...text.caption, fontFamily: font.regular },
  rule: { height: StyleSheet.hairlineWidth, marginHorizontal: INSET, marginVertical: 6 },
  none: { ...text.sm, fontFamily: font.regular, paddingHorizontal: INSET, paddingVertical: 16 },
  summary: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingHorizontal: INSET },
  modelDetail: { gap: 10, paddingHorizontal: INSET, paddingBottom: 12 },
  modelTitle: { flexDirection: "row", alignItems: "center", gap: 12 },
  noteText: { ...text.sm, fontFamily: font.regular },
  note: { ...text.sm, fontFamily: font.regular, paddingHorizontal: INSET, paddingBottom: 8 },
});
