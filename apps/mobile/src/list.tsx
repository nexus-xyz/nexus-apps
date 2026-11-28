import { Stack, router, type Href } from "expo-router";
import { Children, isValidElement, useState, type ComponentProps, type ReactNode } from "react";
import { ActivityIndicator, Image, Platform, Pressable, RefreshControl, ScrollView, StyleSheet, Text, View, useWindowDimensions, type TextInputProps } from "react-native";
import { TextInput } from "@/controls";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { pressedFill, ripple } from "@/press";
import { SkeletonRows } from "@/skeleton";
import { ACCESSIBILITY_FONT_SCALE, isAccessibilitySize } from "@/text-size";
import { font, mono, text, useTheme } from "@/theme";

export { Segmented } from "@/segmented";

/**
 * Settings and the pages it opens, as one grouped list (brand v5's P21): a section's title, its rows in one rounded card
 * on the grouped ground, a footnote under it. A row has its icon on a grey tile, never coloured, its label on the left
 * and its value on the right, as iOS Settings, and a muted second line. A page's one number that matters is its `Hero`,
 * the first row of its card.
 */

type IconName = ComponentProps<typeof Icon>["name"];

/** A settings page: the list's margins, pull to refresh, and taps that reach a button while the keyboard is up. */
export function Page({ children, onRefresh, refreshing = false }: { children: ReactNode; onRefresh?: () => void; refreshing?: boolean }) {
  const theme = useTheme();
  return (
    <ScrollView
      style={{ backgroundColor: theme.grouped }}
      // Under the navigation bar, so iOS's large title collapses as the list scrolls.
      contentInsetAdjustmentBehavior="automatic"
      contentContainerStyle={styles.page}
      keyboardShouldPersistTaps="handled"
      refreshControl={onRefresh ? <RefreshControl refreshing={refreshing} onRefresh={onRefresh} /> : undefined}
    >
      {/* The bar on the grouped ground too, so the page has one ground from the top. */}
      <Stack.Screen options={{ headerStyle: { backgroundColor: theme.grouped }, contentStyle: { backgroundColor: theme.grouped } }} />
      {children}
    </ScrollView>
  );
}

/**
 * A section: its title, its rows in one card with a hairline between them, then its footnote. `caps` sets the title as
 * the settings map's group labels are (P94·B): small capitals, tracked out.
 */
export function Section({ title, caps, header, footer, footerTone, children }: {
  title?: string;
  caps?: boolean;
  /** In the title's place: a part of Settings with its mark and your role (`settings.tsx`). */
  header?: ReactNode;
  footer?: ReactNode;
  footerTone?: "danger";
  children?: ReactNode;
}) {
  const theme = useTheme();
  const rows = Children.toArray(children).filter(isValidElement);
  return (
    <View style={styles.section}>
      {header ?? (title ? (
        <Text style={[styles.title, caps && styles.caps, { color: caps ? theme.tertiary : theme.muted }]} accessibilityRole="header">
          {title}
        </Text>
      ) : null)}
      {rows.length ? (
        <View style={[styles.group, { backgroundColor: theme.card }]}>
          {rows.map((row, n) => (
            <View key={row.key ?? n}>
              {/* The hairline starts where the text does, after the icon's tile. Settings' rows (a bare icon) have
                  none, as the drawer's rows have none: the card and the row's height keep them apart. */}
              {n > 0 && !isBare(row.props) ? <View style={[styles.separator, separatorInset(row.props), { backgroundColor: theme.separator }]} /> : null}
              {row}
            </View>
          ))}
        </View>
      ) : null}
      {footer ? <Footnote tone={footerTone}>{footer}</Footnote> : null}
    </View>
  );
}

// iOS sets a row's icon on a tile; Android keeps Material's bare leading icon, and so does a `bareIcon` row.
const TILE = Platform.OS !== "android";
const isBare = (props: unknown) => !!(props as Partial<RowProps> | undefined)?.bareIcon;
const separatorInset = (props: unknown) => {
  const row = props as Partial<RowProps> | undefined;
  return TILE && row?.icon ? styles.tiledSeparator : null;
};

/** A short line under a card or at the top of a page: what it means, or where to do what the phone does not. */
export function Footnote({ children, tone }: { children: ReactNode; tone?: "danger" }) {
  const theme = useTheme();
  // iOS's standard line breaking keeps a lone word off a footnote's last line.
  return <Text style={[styles.footnote, { color: tone === "danger" ? theme.danger : theme.tertiary }]} lineBreakStrategyIOS="standard">{children}</Text>;
}

type Tone = "danger" | "primary" | "muted";
type RowProps = {
  label: ReactNode;
  /** The muted line under the label. */
  detail?: ReactNode;
  detailTone?: Tone | "tertiary";
  /** On the right, faint: the setting's value, a figure, a status. */
  value?: string;
  valueTone?: Tone | "foreground" | "tertiary";
  icon?: IconName;
  /** The icon bare, 20 pt, with no tile, on a 44 pt row: the settings map's rows (P94·B). Android's are always bare. */
  bareIcon?: boolean;
  /** Before the label instead of an icon: a provider's mark, an initial, a swatch. */
  leading?: ReactNode;
  /** After the value: a switch, a spinner. */
  accessory?: ReactNode;
  /** A page it opens, with Back: shows the chevron. */
  href?: Href;
  onPress?: () => void;
  /** A long press: the row's menu on Android (`row-menu.android.tsx`). */
  onLongPress?: () => void;
  /** The chevron without `href`. */
  chevron?: boolean;
  /** A choice, checked. */
  selected?: boolean;
  /** `primary` is an action: the primary's colour, in Medium, as Nexus UI's buttons, because the colour is a metal. */
  tone?: Tone;
  monoLabel?: boolean;
  monoValue?: boolean;
  /** Centred, as a card's one action (Sign out). */
  center?: boolean;
  disabled?: boolean;
  /** Levels deep in a tree: each indents the row. */
  indent?: number;
  /** Under the row's head, full width: a share bar, a test's result, a change's fields. */
  children?: ReactNode;
} & Pick<ComponentProps<typeof Pressable>, "accessibilityLabel" | "accessibilityHint" | "accessibilityState" | "accessibilityRole" | "accessibilityActions" | "onAccessibilityAction">;

/** A row: an icon or mark, the label and its detail, the value on the right, then a chevron when it opens a page. */
export function Row(props: RowProps) {
  const theme = useTheme();
  const { label, detail, value, icon, bareIcon, leading, accessory, href, selected, center, disabled, children } = props;
  const bare = bareIcon && TILE;
  const press = href
    ? () => {
        haptic("tap");
        router.push(href);
      }
    : props.onPress;
  const tones = { danger: theme.danger, primary: theme.primary, muted: theme.muted, foreground: theme.foreground, tertiary: theme.tertiary };
  const color = disabled ? theme.muted : tones[props.tone ?? "foreground"];
  const indent = props.indent ? { paddingLeft: 16 + props.indent * 16 } : undefined;
  // At the accessibility text sizes the value goes under the label, as in iOS's Settings: side by side, each breaks
  // its words to fit half the row.
  const stacked = isAccessibilitySize(useWindowDimensions().fontScale);
  const valueText = value ? (
    <Text
      style={[styles.value, stacked && styles.stackedValue, props.monoValue && styles.mono, { color: tones[props.valueTone ?? "tertiary"] }]}
      numberOfLines={stacked ? undefined : 2}
    >
      {value}
    </Text>
  ) : null;
  // What VoiceOver reads: the row's words, never its symbols' names ("Two people", "Forward").
  const spoken =
    props.accessibilityLabel ?? (typeof label === "string" ? [label, typeof detail === "string" ? detail : undefined, value].filter(Boolean).join(", ") : undefined);
  const body = (
    <>
      <View style={[styles.rowHead, bare && styles.bareHead]}>
        {icon && bare ? (
          // A step quieter than its label, so the label leads; a tone (danger) colours both.
          <Icon name={icon} size={20} color={props.tone || disabled ? color : theme.muted} />
        ) : icon && TILE ? (
          <View style={[styles.tile, { backgroundColor: theme.fill }]}>
            <Icon name={icon} size={16} color={color} />
          </View>
        ) : icon ? (
          <Icon name={icon} size={18} color={color} />
        ) : (
          leading
        )}
        <View style={styles.text}>
          {typeof label === "string" ? (
            <Text style={[styles.label, props.tone === "primary" && styles.action, props.monoLabel && styles.mono, { color }, center && styles.center]} numberOfLines={stacked ? undefined : 2}>
              {label}
            </Text>
          ) : (
            label
          )}
          {detail ? (
            <Text style={[styles.detail, bare && styles.bareDetail, { color: tones[props.detailTone ?? "muted"] }]} numberOfLines={stacked ? undefined : 4}>
              {detail}
            </Text>
          ) : null}
          {stacked ? valueText : null}
        </View>
        {stacked ? null : valueText}
        {accessory}
        {selected ? <Icon name="check" size={16} color={theme.primary} /> : null}
        {href || props.chevron ? <Icon name="forward" size={13} color={theme.tertiary} /> : null}
      </View>
      {children}
    </>
  );
  if (!press && !props.onLongPress) {
    return (
      <View style={[styles.row, bare && styles.bareRow, indent]} accessible={!children && !accessory} accessibilityLabel={spoken}>
        {body}
      </View>
    );
  }
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.row, bare && styles.bareRow, indent, pressed && !disabled && pressedFill(theme)]}
      onPress={press}
      onLongPress={props.onLongPress}
      disabled={disabled}
      accessibilityActions={props.accessibilityActions}
      onAccessibilityAction={props.onAccessibilityAction}
      accessibilityRole={props.accessibilityRole ?? "button"}
      accessibilityLabel={spoken}
      accessibilityHint={props.accessibilityHint}
      accessibilityState={{ ...(selected !== undefined ? { selected } : {}), ...(disabled ? { disabled } : {}), ...props.accessibilityState }}
    >
      {body}
    </Pressable>
  );
}

/** A list's large title in its content, where iOS's own does not draw (`(main)/_layout.tsx`): 34 pt, tracked in. */
export function LargeTitle({ children }: { children: string }) {
  const theme = useTheme();
  return (
    // 34 pt already: it grows to the first accessibility size, as far as a one-word title still fits the line.
    <Text style={[styles.largeTitle, { color: theme.foreground }]} accessibilityRole="header" numberOfLines={1} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
      {children}
    </Text>
  );
}

/** A text field as a row of its card, as iOS's forms draw one. */
export function Field({ monoText, style, ...input }: TextInputProps & { monoText?: boolean }) {
  const theme = useTheme();
  return <TextInput placeholderTextColor={theme.muted} {...input} style={[styles.field, monoText && styles.mono, { color: theme.foreground }, style]} />;
}

/** The card's one figure, big: what it is, then what it is out of; a meter goes under it. */
export function Hero({ label, value, note, children }: { label: string; value: string; note?: string; children?: ReactNode }) {
  const theme = useTheme();
  return (
    <View style={styles.hero}>
      <View accessible accessibilityLabel={`${label}: ${value}${note ? `, ${note}` : ""}`}>
        <Text style={[styles.detail, { color: theme.muted }]}>{label}</Text>
        <Text style={[styles.heroValue, { color: theme.foreground }]} adjustsFontSizeToFit numberOfLines={1}>
          {value}
        </Text>
        {note ? <Text style={[styles.detail, { color: theme.muted }]}>{note}</Text> : null}
      </View>
      {children}
    </View>
  );
}

/** Content in a card that is not a row (a control, a chart, a command), with the rows' margins. */
export function Inset({ children }: { children: ReactNode }) {
  return <View style={styles.inset}>{children}</View>;
}

/** A thin bar: a share of the whole. */
export function Share({ share }: { share: number }) {
  const theme = useTheme();
  return (
    <View style={[styles.track, { backgroundColor: theme.fill }]}>
      <View style={[styles.fill, { width: `${Math.max(share * 100, share > 0 ? 1 : 0)}%`, backgroundColor: theme.primary }]} />
    </View>
  );
}

/**
 * A name's first letter on a tile: an organization (square) or a person (round). With a `logo` (an organization's
 * `imageUrl`), the logo on a white tile instead, as Settings' organization row sets it; the letter again if it fails.
 */
export function Initial({ name, size = 30, round, onFill, tint, logo }: {
  name: string;
  size?: number;
  round?: boolean;
  /** On a row already in the fill (the chosen one): the tile takes the card's colour, or it vanishes into the row. */
  onFill?: boolean;
  /** The tile's colour where the fill does not suit the ground (a sheet's glass: `theme.glassFill`). */
  tint?: string;
  logo?: string;
}) {
  const theme = useTheme();
  const [failed, setFailed] = useState<string | null>(null);
  const radius = round ? size / 2 : size * 0.28;
  if (logo && failed !== logo) {
    return (
      // White, as the desktop sets it: a logo made for a white ground stays visible in dark.
      <View style={[styles.initial, styles.logoTile, { width: size, height: size, borderRadius: radius, borderColor: theme.separator }]}>
        <Image source={{ uri: logo }} style={{ width: size, height: size }} onError={() => setFailed(logo)} accessibilityIgnoresInvertColors />
      </View>
    );
  }
  return (
    <View style={[styles.initial, { width: size, height: size, borderRadius: radius, backgroundColor: tint ?? (onFill ? theme.card : theme.fill) }]}>
      {/* The letter is sized to its tile, which does not grow with the text size: scaled, it overflows the tile. */}
      <Text style={[styles.initialText, { color: theme.foreground, fontSize: size * 0.44, lineHeight: size * 0.6 }]} allowFontScaling={false}>{name.trim().charAt(0).toUpperCase() || "?"}</Text>
    </View>
  );
}

/**
 * Loading (brand v5's P44): the list's shape in the fill, after 300 ms, with words past 3 s (`skeleton.tsx`). `alone`
 * when it is the whole screen, so it sits on the page's ground and margins; `bare` inside a section's card.
 */
export function Loading({ alone, bare, rows }: { alone?: boolean; bare?: boolean; rows?: number }) {
  const body = <SkeletonRows rows={rows} bare={bare} />;
  return alone ? <Page>{body}</Page> : body;
}

/** Nothing to list, or what went wrong, in the card's place or as its one row. */
export function Empty({ children, tone }: { children: ReactNode; tone?: "danger" }) {
  const theme = useTheme();
  return <Text style={[styles.empty, { color: tone === "danger" ? theme.danger : theme.muted }]}>{children}</Text>;
}

/**
 * A page with nothing to show (not set up, not yours to see, failed to load): a title and a line, centred. `action` is
 * the one button that fixes it, such as Connect phone or Retry: a page that can be fixed is never a dead end.
 */
export function Blank({ title, body, tone, action }: { title: string; body?: string; tone?: "danger"; action?: { label: string; onPress: () => void } }) {
  const theme = useTheme();
  return (
    <View style={styles.blank}>
      <Text style={[styles.blankTitle, { color: theme.foreground }]}>{title}</Text>
      {body ? <Text style={[styles.blankBody, { color: tone === "danger" ? theme.danger : theme.muted }]}>{body}</Text> : null}
      {action ? (
        <Pressable onPress={() => { haptic("tap"); action.onPress(); }} style={[styles.blankAction, { backgroundColor: theme.fill }]} accessibilityRole="button">
          <Text style={[styles.blankActionText, { color: theme.foreground }]}>{action.label}</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

/** A spinner on the right of a row. */
export function Busy() {
  const theme = useTheme();
  return <ActivityIndicator color={theme.muted} />;
}

const styles = StyleSheet.create({
  // The boards' rhythm: 22 pt between groups, a title 7 pt over its card.
  page: { paddingHorizontal: 16, paddingTop: 8, paddingBottom: 48, gap: 22 },
  section: { gap: 7 },
  title: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 16 },
  // The settings map's group label (P94·B): 12 pt small capitals, tracked 0.06em.
  caps: { fontSize: 11.5, lineHeight: 16, letterSpacing: 0.7, textTransform: "uppercase" },
  group: { borderRadius: 12, borderCurve: "continuous", overflow: "hidden" },
  separator: { height: StyleSheet.hairlineWidth, marginLeft: 16 },
  // The row's 16, the tile's 30 and the 12 after it.
  tiledSeparator: { marginLeft: 58 },
  footnote: { ...text.caption, fontFamily: font.regular, paddingHorizontal: 16 },
  row: { minHeight: 46, paddingHorizontal: 16, paddingVertical: 11, gap: 10, justifyContent: "center" },
  rowHead: { flexDirection: "row", alignItems: "center", gap: 12 },
  // The settings map's row (P94·B): 44 pt, the bare icon 14 pt from its label.
  bareRow: { minHeight: 44, paddingVertical: 10 },
  bareHead: { gap: 14 },
  tile: { width: 30, height: 30, borderRadius: 8, borderCurve: "continuous", alignItems: "center", justifyContent: "center" },
  text: { flex: 1, gap: 2 },
  center: { textAlign: "center" },
  label: { ...text.body, fontFamily: font.regular },
  action: { fontFamily: font.medium },
  detail: { ...text.secondary, fontFamily: font.regular },
  // Under a settings row's name: a caption, so the name leads and the row stays compact.
  bareDetail: { ...text.caption },
  value: { ...text.secondary, fontFamily: font.regular, flexShrink: 1, maxWidth: "55%", textAlign: "right", fontVariant: ["tabular-nums"] },
  stackedValue: { maxWidth: "100%", textAlign: "left" },
  mono: { fontFamily: mono, fontSize: 14 },
  field: { ...text.body, fontFamily: font.regular, minHeight: 46, paddingHorizontal: 16 },
  largeTitle: { ...text.largeTitle, fontFamily: font.medium, marginTop: 2 },
  hero: { padding: 16, gap: 12 },
  heroValue: { fontFamily: mono, fontSize: 32, lineHeight: 40, fontVariant: ["tabular-nums"] },
  inset: { padding: 12, gap: 10 },
  track: { height: 4, borderRadius: 2, overflow: "hidden" },
  fill: { height: "100%" },
  initial: { alignItems: "center", justifyContent: "center", borderCurve: "continuous" },
  initialText: { fontFamily: font.medium },
  logoTile: { overflow: "hidden", backgroundColor: "#ffffff", borderWidth: StyleSheet.hairlineWidth },
  empty: { ...text.sm, fontFamily: font.regular, paddingHorizontal: 16, paddingVertical: 14 },
  blank: { paddingHorizontal: 32, paddingTop: 96, gap: 6, alignItems: "center" },
  blankTitle: { ...text.lg, fontFamily: font.medium, textAlign: "center" },
  blankBody: { ...text.sm, fontFamily: font.regular, textAlign: "center" },
  blankAction: { minHeight: 44, paddingHorizontal: 20, borderRadius: 22, justifyContent: "center", marginTop: 10 },
  blankActionText: { ...text.sm, fontFamily: font.medium },
});
