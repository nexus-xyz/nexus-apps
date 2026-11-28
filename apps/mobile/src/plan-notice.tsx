import type { BillingBlock, PayLineParts } from "@nexus/kit";
import { router, type Href } from "expo-router";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { AskOwner } from "@/ask-owner";
import { Icon } from "@/icon";
import { pressedFill, ripple } from "@/press";
import { font, text, type Theme } from "@/theme";

/**
 * A billing notice in the chat: used-up Nexus usage above the composer, or an explained refusal in an
 * answer's place. Its title, what to do, and the phone's page that shows it (Billing, or API keys). Never a purchase
 * link: Billing is where an owner switches to pay as you go.
 */
export function PlanNotice({ title, body, open, act, ask, theme }: { title: string; body: string; open: { href: Href; label: string }; act?: boolean; ask?: BillingBlock; theme: Theme }) {
  // A member who is not an owner: the owners and "Ask an owner" first, the page that still helps after.
  if (ask) {
    return (
      <View style={[styles.notice, { backgroundColor: theme.card, borderColor: theme.separator }]} accessibilityLabel={`${title}. ${body}`}>
        <View style={styles.head}>
          {act ? <View style={[styles.dot, { backgroundColor: theme.waiting }]} /> : null}
          <Text style={[styles.title, { color: theme.foreground }]}>{title}</Text>
        </View>
        <Text style={[styles.body, { color: theme.muted }]}>{body}</Text>
        <AskOwner block={ask} theme={theme} />
        <Pressable onPress={() => router.push(open.href)} accessibilityRole="link" hitSlop={8} style={styles.open}>
          <Text style={[styles.openText, { color: theme.muted }]}>{open.label}</Text>
          <Icon name="forward" size={11} color={theme.muted} />
        </Pressable>
      </View>
    );
  }
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.notice, { backgroundColor: theme.card, borderColor: theme.separator }, pressed && pressedFill(theme)]}
      onPress={() => router.push(open.href)}
      accessibilityRole="button"
      accessibilityLabel={`${title}. ${body}`}
      accessibilityHint={open.label}
    >
      <View style={styles.head}>
        {act ? <View style={[styles.dot, { backgroundColor: theme.waiting }]} /> : null}
        <Text style={[styles.title, { color: theme.foreground }]}>{title}</Text>
      </View>
      <Text style={[styles.body, { color: theme.muted }]}>{body}</Text>
      <View style={styles.open}>
        <Text style={[styles.openText, { color: theme.foreground }]}>{open.label}</Text>
        <Icon name="forward" size={11} color={theme.muted} />
      </View>
    </Pressable>
  );
}

/**
 * Free and what is left of the free start, in a capsule that opens Billing. The capsule has the card's colour, so
 * it reads over an answer scrolled under the composer.
 */
/**
 * The quiet pill above the composer. With `parts` (the pay line), the share used sets itself apart: a short amber meter
 * takes the dot's place and the figure is in full ink, as on the web.
 */
export function PlanLine({ label, parts, act, theme }: { label: string; parts?: PayLineParts; act?: boolean; theme: Theme }) {
  const meter = parts?.usedPercent;
  return (
    <Pressable
      android_ripple={ripple(theme)}
      style={({ pressed }) => [styles.line, { backgroundColor: theme.card, borderColor: theme.separator }, pressed && pressedFill(theme)]}
      onPress={() => router.push("/billing")}
      accessibilityRole="button"
      accessibilityHint="Opens billing"
      accessibilityLabel={label}
      hitSlop={6}
    >
      {meter !== undefined ? (
        <View style={[styles.meter, { backgroundColor: theme.separator }]}>
          <View style={[styles.meterFill, { width: `${meter}%`, backgroundColor: theme.waiting }]} />
        </View>
      ) : act ? (
        <View style={[styles.lineDot, { backgroundColor: theme.waiting }]} />
      ) : null}
      <Text style={[styles.lineText, { color: theme.muted }]} numberOfLines={1}>
        {parts ? (
          <>
            {parts.label}
            {parts.used ? <>{" · "}<Text style={[styles.used, { color: theme.foreground }]}>{parts.used}</Text></> : null}
            {parts.when ? ` · ${parts.when}` : null}
          </>
        ) : label}
      </Text>
      <Icon name="forward" size={10} color={theme.muted} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  // Brand v5's composer notice (P73·E): radius 14, padding 14, a 1 px edge; the title 16/22 at 400, the body 15/20.
  notice: { borderRadius: 14, borderCurve: "continuous", borderWidth: 1, padding: 14, gap: 4 },
  head: { flexDirection: "row", alignItems: "center", gap: 8 },
  // An 8 px amber dot before the title when someone must act on a payment (P73·H); 6 px in a line.
  dot: { width: 8, height: 8, borderRadius: 4 },
  lineDot: { width: 6, height: 6, borderRadius: 3 },
  title: { fontSize: 16, lineHeight: 22, fontFamily: font.regular, flexShrink: 1 },
  body: { fontSize: 15, lineHeight: 20, fontFamily: font.regular },
  open: { flexDirection: "row", alignItems: "center", gap: 4, marginTop: 4 },
  openText: { ...text.sm, fontFamily: font.medium },
  line: {
    flexDirection: "row",
    alignItems: "center",
    alignSelf: "center",
    gap: 4,
    minHeight: 30,
    paddingHorizontal: 12,
    borderRadius: 15,
    borderWidth: StyleSheet.hairlineWidth,
    marginBottom: 6,
  },
  lineText: { ...text.xs, fontFamily: font.medium, flexShrink: 1 },
  // The share used: a 24 x 4 track, the figure in tabular digits so it does not jiggle as it moves.
  meter: { width: 24, height: 4, borderRadius: 2, overflow: "hidden" },
  meterFill: { height: 4, borderRadius: 2 },
  used: { fontVariant: ["tabular-nums"] },
});
