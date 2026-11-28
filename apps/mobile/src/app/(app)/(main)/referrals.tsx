import { CoreError, type ReferralList, type ReferredPerson, referralRewardText } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { Pressable, Share, StyleSheet, Text, View } from "react-native";
import { referralOffer, joinedLine, referredEmail } from "@/account";
import { useAuth, useCore } from "@/auth";
import { invite } from "@/invite";
import { Blank, Empty, Footnote, Loading, Page, Row, Section } from "@/list";
import { Meter } from "@/meter";
import { canText, composeText } from "@/text-message";
import { font, mono, text, useTheme } from "@/theme";

const day = (iso: string) => new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short" });

/**
 * Referrals, in Account (brand v5's P68·F on the phone, Nexus UI's `/referrals`): the offer, your link with Text a
 * friend (Messages, where the person picks the contacts) and Share link (the system's share sheet), the
 * invitations used while sign-up is invite-only (in the danger tone once none is left),
 * then the people who joined
 * through it, oldest first as the Gateway sends them, masked. Gateway supplies the program pause, lifetime reward progress and each reward's
 * state; the offer and sharing follow those values.
 */
export default function Referrals() {
  const core = useCore();
  const { ui } = useAuth();
  const theme = useTheme();
  const [first, setFirst] = useState<ReferralList | null>(null);
  const [people, setPeople] = useState<ReferredPerson[]>([]);
  const [cursor, setCursor] = useState<string | undefined>();
  const [error, setError] = useState<{ text: string; missing?: boolean } | null>(null);
  const [paging, setPaging] = useState(false);
  const [texts, setTexts] = useState(false);
  useEffect(() => {
    canText().then(setTexts);
  }, []);

  const load = useCallback(() => {
    core.referrals().then(
      (list) => {
        setFirst(list);
        setPeople(list.referred);
        setCursor(list.nextCursor);
        setError(null);
      },
      (e: Error) => setError({ text: e.message, missing: e instanceof CoreError && e.status === 404 }),
    );
  }, [core]);
  useEffect(load, [load]);
  const more = () => {
    if (!cursor || paging) return;
    setPaging(true);
    core
      .referrals(cursor)
      .then((page) => {
        setFirst(page);
        setPeople((now) => [...now, ...page.referred]);
        setCursor(page.nextCursor);
      }, (e: Error) => setError({ text: e.message }))
      .finally(() => setPaging(false));
  };

  if (!first) {
    if (error?.missing) return <Blank title="Referrals are not available yet" body="This Gateway does not have referrals. Check back after it updates." />;
    return error ? <Blank title="Couldn't load your referrals" body={error.text} tone="danger" /> : <Loading alone />;
  }
  const offer = referralOffer(first, ui);
  const link = offer.link;
  const invitation = invite(link, texts);
  // The message alone: iOS would add `url` to it, and the link would go twice.
  const share = () => invitation && Share.share({ message: invitation.message });
  // Messages opens with no recipient: the person picks the contacts there. If it fails to open, the share sheet does.
  const textFriend = () => invitation && composeText(invitation.message).catch(share);

  return (
    <Page onRefresh={load}>
      {/* The offer (P68·F): a card with the title, the terms, the link in mono and one button under it. */}
      <View style={[styles.hero, offer.usedUp ? { backgroundColor: theme.dangerFaint, borderWidth: StyleSheet.hairlineWidth, borderColor: theme.danger } : { backgroundColor: theme.card }]}>
        <Text style={[styles.heroTitle, { color: theme.foreground }]}>{offer.title}</Text>
        <Text style={[styles.heroBody, { color: theme.muted }]}>{offer.body}</Text>
        {offer.invites ? (
          <View>
            <View style={styles.countRow}>
              <Text style={[styles.count, { color: theme.foreground }]}>{offer.invites.text}</Text>
              {offer.invites.left ? <Text style={[styles.countLeft, { color: offer.usedUp ? theme.danger : theme.tertiary }]}>{offer.invites.left}</Text> : null}
            </View>
            {offer.invites.share !== null ? <Meter theme={theme} spent={offer.invites.share} reserved={0} label="Invitations used" danger={offer.usedUp} /> : null}
          </View>
        ) : null}
        {offer.progress ? <Text style={[styles.note, {color:theme.tertiary}]}>{offer.progress}</Text> : null}
        {link ? (
          <>
            <Text style={[styles.link, { color: theme.foreground, backgroundColor: theme.fill }]} selectable numberOfLines={1} ellipsizeMode="middle">
              {link.replace(/^https?:\/\//, "")}
            </Text>
            {invitation?.text ? (
              <Pressable style={({ pressed }) => [styles.button, { backgroundColor: theme.primary }, pressed && styles.pressed]} onPress={textFriend} accessibilityRole="button" accessibilityHint="Opens Messages with your link. You pick who gets it.">
                <Text style={[styles.buttonText, { color: theme.onPrimary }]}>Text a friend</Text>
              </Pressable>
            ) : null}
            <Pressable style={({ pressed }) => [styles.button, invitation?.text ? { backgroundColor: theme.fill } : { backgroundColor: theme.primary }, pressed && styles.pressed]} onPress={share} accessibilityRole="button">
              <Text style={[styles.buttonText, { color: invitation?.text ? theme.foreground : theme.onPrimary }]}>Share link</Text>
            </Pressable>
          </>
        ) : null}
      </View>
      {error ? <Footnote tone="danger">{error.text}</Footnote> : null}
      <Section title={people.length ? joinedLine(people.length, !!cursor) : "People who joined"}>
        {people.length === 0 ? <Empty>Nobody has joined through your link yet.</Empty> : null}
        {people.map((person, n) => (
          <Row key={`${person.joinedAt}-${n}`} label={referredEmail(person)} tone={person.email ? undefined : "muted"} value={day(person.joinedAt)} detail={referralRewardText(person, first)} />
        ))}
        {cursor ? <Row label={paging ? "Loading…" : "Show more"} tone="primary" onPress={more} disabled={paging} /> : null}
      </Section>
    </Page>
  );
}

// The board's card (P68·F): 16 pt in, radius 16, the title 20 pt, the link in mono on the fill, a 50 pt button.
const styles = StyleSheet.create({
  hero: { marginHorizontal: 16, marginBottom: 8, padding: 16, gap: 10, borderRadius: 16, borderCurve: "continuous" },
  heroTitle: { fontFamily: font.medium, fontSize: 20, lineHeight: 25 },
  heroBody: { ...text.secondary, fontFamily: font.regular },
  link: { fontFamily: mono, fontSize: 14, lineHeight: 20, paddingHorizontal: 12, paddingVertical: 10, borderRadius: 10, overflow: "hidden" },
  button: { minHeight: 50, borderRadius: 25, alignItems: "center", justifyContent: "center" },
  buttonText: { ...text.body, fontFamily: font.medium },
  note: { ...text.caption, fontFamily: font.regular, textAlign: "center" },
  countRow: { flexDirection: "row", alignItems: "baseline", justifyContent: "space-between", gap: 8 },
  count: { ...text.secondary, fontFamily: font.medium, fontVariant: ["tabular-nums"] },
  countLeft: { ...text.caption, fontFamily: font.medium, fontVariant: ["tabular-nums"] },
  pressed: { opacity: 0.7 },
});
