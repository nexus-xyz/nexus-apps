import { Stack } from "expo-router";
import type { ComponentProps, ReactNode } from "react";
import { ActivityIndicator, Platform, Pressable, StyleSheet, Text, View } from "react-native";
import { Switch } from "@/controls";
import { Icon } from "@/icon";
import { Field, LargeTitle, Row, Section } from "@/list";
import { font, text, useTheme } from "@/theme";

/**
 * The admin page system on the phone (brand v5's P105·D), for the People and Controls pages: Settings drills into a
 * page, never tabs; the page keeps its title, large in the list under Back, and one line that says what it is for; its
 * one primary action is + in the bar; an item opens full screen as a card of facts with its one action at the foot;
 * editors are sheets.
 */

const ANDROID = Platform.OS === "android";

/** A page's head: the title large in the list (the bar keeps only Back, named for where you came from), the purpose under it, and + in the bar. */
export function AdminHead({ title, purpose, add, more }: { title: string; purpose?: string; add?: { label: string; onPress: () => void }; more?: { label: string; onPress: () => void } }) {
  const theme = useTheme();
  const action = add ?? more;
  return (
    <View style={styles.head}>
      <Stack.Screen
        options={{
          title,
          headerTitle: "",
          headerBackButtonDisplayMode: "default",
          headerRight: action
            ? () => (
                <Pressable onPress={action.onPress} accessibilityRole="button" accessibilityLabel={action.label} hitSlop={12} style={ANDROID ? styles.headerButton : undefined}>
                  <Icon name={add ? "plus" : "more"} color={theme.foreground} />
                </Pressable>
              )
            : undefined,
        }}
      />
      <LargeTitle>{title}</LargeTitle>
      {purpose ? <Text style={[styles.purpose, { color: theme.muted }]}>{purpose}</Text> : null}
    </View>
  );
}

/** One fact of an item (P105·D's "Rule", "Applies to"): its name small above it, the value in the body's size. */
export function Fact({ label, value, valueTone, href, onPress }: { label: string; value: string; valueTone?: "danger"; href?: ComponentProps<typeof Row>["href"]; onPress?: () => void }) {
  const theme = useTheme();
  return (
    <Row
      label={
        <View style={styles.fact}>
          <Text style={[styles.factLabel, { color: theme.tertiary }]}>{label}</Text>
          <Text style={[styles.factValue, { color: valueTone === "danger" ? theme.danger : theme.foreground }]}>{value}</Text>
        </View>
      }
      accessibilityLabel={`${label}: ${value}`}
      href={href}
      onPress={onPress}
    />
  );
}

/** The item's one action, at the foot in a card of its own, centred. */
export function FootAction({ label, tone, onPress, disabled }: { label: string; tone?: "danger"; onPress: () => void; disabled?: boolean }) {
  return (
    <Section>
      <Row label={label} tone={tone ?? "primary"} center onPress={onPress} disabled={disabled} />
    </Section>
  );
}

/** Something that waits for you (P105's notice): one quiet line, an amber mark, and where to act. Never a coloured banner. */
export function Notice({ label, detail, action, href }: { label: string; detail?: string; action?: string; href: ComponentProps<typeof Row>["href"] }) {
  const theme = useTheme();
  return (
    <Section>
      <Row leading={<Icon name="flag" size={16} color={theme.mark} />} label={label} detail={detail} value={action} href={href} />
    </Section>
  );
}

/**
 * An editor in a sheet (P105: editors as sheets), as Keys' Create key draws one: its title, its fields, one full-width
 * button, and a line under it that turns red when what you typed was refused.
 */
export function SheetForm({
  title,
  children,
  action,
  onSubmit,
  disabled,
  busy,
  note,
  error,
}: {
  title: string;
  children: ReactNode;
  action: string;
  onSubmit: () => void;
  disabled?: boolean;
  busy?: boolean;
  note?: string;
  error?: string | null;
}) {
  const theme = useTheme();
  const off = disabled || busy;
  return (
    <View style={styles.form}>
      <Text style={[styles.formTitle, { color: theme.foreground }]} accessibilityRole="header">
        {title}
      </Text>
      {children}
      <Pressable
        style={({ pressed }) => [styles.button, { backgroundColor: theme.primary }, off && styles.idle, pressed && styles.pressed]}
        onPress={onSubmit}
        disabled={off}
        accessibilityRole="button"
        accessibilityState={{ disabled: !!off, busy: !!busy }}
      >
        {busy ? <ActivityIndicator color={theme.onPrimary} /> : <Text style={[styles.buttonText, { color: theme.onPrimary }]}>{action}</Text>}
      </Pressable>
      {error || note ? <Text style={[styles.note, { color: error ? theme.danger : theme.tertiary }]}>{error || note}</Text> : null}
    </View>
  );
}

/** A sheet's text field, on the fill. */
export function SheetField(props: ComponentProps<typeof Field>) {
  const theme = useTheme();
  return <Field {...props} style={[styles.field, { backgroundColor: theme.fill }, props.style]} />;
}

/** A sheet's switch, its label on the left. */
export function SheetSwitch({ label, value, onChange }: { label: string; value: boolean; onChange: (on: boolean) => void }) {
  const theme = useTheme();
  return (
    <View style={styles.switchRow}>
      <Text style={[styles.switchLabel, { color: theme.foreground }]}>{label}</Text>
      <Switch value={value} onValueChange={onChange} accessibilityLabel={label} />
    </View>
  );
}

// The board's measures (P105·D): the purpose 4 pt under the title; a fact's name 12 pt over its value. The sheet is
// Keys' (P68·K): its title 20 pt, fields and one button 50 pt high.
const styles = StyleSheet.create({
  head: { gap: 4 },
  purpose: { ...text.secondary, fontFamily: font.regular },
  headerButton: { width: 48, height: 48, alignItems: "center", justifyContent: "center" },
  fact: { gap: 1 },
  factLabel: { fontSize: 12, lineHeight: 16, fontFamily: font.regular },
  factValue: { ...text.body, fontFamily: font.regular },
  form: { paddingHorizontal: 20, paddingTop: 8, paddingBottom: 8, gap: 12 },
  formTitle: { fontFamily: font.medium, fontSize: 20, lineHeight: 25 },
  field: { minHeight: 50, paddingHorizontal: 14, borderRadius: 12, borderCurve: "continuous" },
  switchRow: { flexDirection: "row", alignItems: "center", gap: 12, minHeight: 44 },
  switchLabel: { ...text.body, fontFamily: font.regular, flex: 1 },
  button: { minHeight: 50, borderRadius: 25, alignItems: "center", justifyContent: "center", paddingHorizontal: 24, paddingVertical: 10 },
  buttonText: { ...text.body, fontFamily: font.medium },
  note: { ...text.caption, fontFamily: font.regular },
  idle: { opacity: 0.4 },
  pressed: { opacity: 0.7 },
});
