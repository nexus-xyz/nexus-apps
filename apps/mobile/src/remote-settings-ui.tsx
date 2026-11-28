import type { ComputerEffort, ComputerModelChoice, ComputerRouteChoices } from "@nexus/kit/computer-types";
import { Keyboard, Pressable, StyleSheet, Text, View } from "react-native";
import { haptic } from "./haptics";
import { Icon } from "./icon";
import { font, text, useTheme } from "./theme";
import { ComposerPicker, composerControls } from "./composer-controls";
import { CHROME_FONT_SCALE } from "./text-size";

/** Labels stay readable in sheets; the compact composer always keeps a 44 pt touch target. */
export function SettingsChip({ label, value, displayValue = value, leading, disabled, onPress }: { label: string; value: string; displayValue?: string; leading?: React.ReactNode; disabled?: boolean; onPress: () => void }) {
  const theme = useTheme();
  return <ComposerPicker onPress={() => { Keyboard.dismiss(); haptic("selection"); onPress(); }} disabled={disabled} label={`${label}: ${value}`} style={styles.chip}>
    {leading}<Text numberOfLines={1} maxFontSizeMultiplier={CHROME_FONT_SCALE} style={[composerControls.pickerText, { color: theme.muted }]}>{displayValue}</Text><Icon name="chevron" size={12} color={theme.muted} />
  </ComposerPicker>;
}
export function RemoteChoice({ label, detail, selected, disabled, onPress }: { label: string; detail?: string | null; selected?: boolean; disabled?: boolean; onPress: () => void }) {
  const theme = useTheme();
  return <Pressable onPress={() => { haptic("selection"); onPress(); }} disabled={disabled} accessibilityRole={selected === undefined ? "button" : "radio"} accessibilityLabel={[label, detail].filter(Boolean).join(". ")} accessibilityState={{ checked: selected, disabled }} style={({ pressed }) => [styles.choice, { backgroundColor: pressed ? theme.fill : "transparent", opacity: disabled ? 0.5 : 1 }]}>
    <View style={styles.words}><Text style={[styles.label, { color: theme.foreground }]}>{label}</Text>{detail ? <Text style={[styles.detail, { color: theme.muted }]}>{detail}</Text> : null}</View>
    <View style={styles.check}>{selected ? <Icon name="check" size={18} color={theme.foreground} /> : null}</View>
  </Pressable>;
}
export function RemoteModels({ models, selected, disabled, onPick }: { models: readonly ComputerModelChoice[]; selected: string | null; disabled?: boolean; onPick: (model: string | null) => void }) {
  return <><RemoteChoice label="Computer default" selected={selected === null} disabled={disabled} onPress={() => onPick(null)} />
    {models.map((model) => <RemoteChoice key={model.id} label={model.name} detail={model.name !== model.id ? model.id : null} selected={model.id === selected} disabled={disabled} onPress={() => onPick(model.id)} />)}
    {selected !== null && !models.some((model) => model.id === selected) ? <SettingsNote>{selected} is no longer available. Choose a model or refresh the computer’s settings.</SettingsNote> : null}
  </>;
}
/** `selected` is undefined when the current level is one this phone cannot show: no choice is marked. */
export function RemoteEfforts({ efforts, selected, disabled, onPick }: { efforts: readonly ComputerEffort[]; selected: ComputerEffort | null | undefined; disabled?: boolean; onPick: (effort: ComputerEffort | null) => void }) {
  return <><RemoteChoice label="Agent default" selected={selected === null} disabled={disabled} onPress={() => onPick(null)} />
    {efforts.map((effort) => <RemoteChoice key={effort} label={effortLabel(effort)} selected={selected === effort} disabled={disabled} onPress={() => onPick(effort)} />)}
  </>;
}
export function RemoteRoutes({ routes, selected, disabled, onPick }: { routes: readonly ComputerRouteChoices[]; selected?: { via: string; payer: string }; disabled?: boolean; onPick: (route: ComputerRouteChoices) => void }) {
  return <>{routes.map((route) => <RemoteChoice key={`${route.via}:${route.payer}`} label={route.label} detail={[route.detail, route.state === "unknown" ? "Availability could not be confirmed on the computer." : route.state === "unavailable" ? "Unavailable on this computer." : null].filter(Boolean).join(" ")} selected={route.via === selected?.via && route.payer === selected.payer} disabled={disabled || route.state !== "usable"} onPress={() => onPick(route)} />)}
    {!routes.length ? <SettingsNote>No payment choices were reported by this computer. Refresh its saved projects.</SettingsNote> : null}
  </>;
}
export function SettingsNote({ children, danger }: { children: React.ReactNode; danger?: boolean }) { const theme = useTheme(); return <Text accessibilityLiveRegion="polite" style={[styles.note, { color: danger ? theme.danger : theme.muted }]}>{children}</Text>; }
export function SettingsAction({ label, disabled, onPress }: { label: string; disabled?: boolean; onPress: () => void }) { const theme = useTheme(); return <Pressable disabled={disabled} onPress={onPress} style={[styles.action, { opacity: disabled ? 0.4 : 1 }]} accessibilityRole="button" accessibilityState={{ disabled }}><Text style={[styles.label, { color: theme.foreground }]}>{label}</Text></Pressable>; }
export function effortLabel(effort: ComputerEffort | null) { return effort ? effort[0].toUpperCase() + effort.slice(1) : "Agent default"; }
/** The label of a level the computer runs that this phone does not know. */
export const COMPUTER_EFFORT = "Set on the computer";
const styles = StyleSheet.create({
  chip: { maxWidth: 180 },
  choice: { minHeight: 56, flexDirection: "row", alignItems: "center", paddingHorizontal: 20, paddingVertical: 12, gap: 12 }, words: { flex: 1, gap: 4 }, label: { ...text.body, fontFamily: font.medium }, detail: { ...text.secondary, fontFamily: font.regular }, check: { width: 20 },
  note: { ...text.secondary, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 12 }, action: { minHeight: 48, paddingHorizontal: 20, alignItems: "center", justifyContent: "center" },
});
