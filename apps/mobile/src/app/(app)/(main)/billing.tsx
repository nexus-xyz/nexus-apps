import { BILLING_TERMS, TRY_AGAIN, allowanceRows, billingCutoff, billingStaleNote, planOf, v3Plan, type BillingResponse, type CurrentTenantResponse, type Receipt } from "@nexus/kit";
import { useFocusEffect } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { Alert, AppState, Linking, StyleSheet, Text, View } from "react-native";
import { AskOwner } from "@/ask-owner";
import { useAuth, useCore } from "@/auth";
import {
  ALWAYS_FREE,
  RECEIPTS_SHOWN,
  afterTheFactNoticeOf,
  freeLine,
  freeStartLeft,
  moreReceipts,
  promotionalLine,
  monthLabel,
  noBilling,
  openReceipts,
  paygLine,
  paymentLabel,
  pastDueStopped,
  setting,
  showsUsageLimit,
  topupLine,
  topupOffAlert,
  topupOffLine,
  usageLimitLine,
  usd,
} from "@/billing";
import { ReceiptRow, useReceiptOpener } from "@/billing-receipts";
import { Blank, Footnote, LargeTitle, Loading, Page, Row, Section, Share } from "@/list";
import { payAsYouGoPitch, webBillingActions } from "@/storefront";
import { font, text, useTheme } from "@/theme";
import { useWebBilling } from "@/use-storefront";

/**
 * Billing for owners (pricing v2.0), as brand v5's P40·D draws it on the phone, read first: under the organization's
 * name, how it pays (Free, with what is left of its free start, or Pay as you go, with its credits and what the card
 * paid this month), then its Nexus credits with the auto top-up as set, its monthly usage limit and its payment method,
 * then its latest receipts, with "All receipts" for the rest (`receipts.tsx`). A refresh that fails keeps what loaded,
 * under a grey note with Try again (brand v5 P44; red is only for refusals). Billing dates are UTC, as on every surface.
 * The usage limit row says the charge's phase in the kit's words; a charge an owner must act on also gets
 * an amber card.
 *
 * What an owner changes here follows the store. Every storefront: the changes that buy nothing, auto top-up
 * off and a lower usage limit. The US App Store only (`storefront.ts`): rows that open Nexus UI's Billing in Safari,
 * outside the app, for the rest. Anywhere else no screen points to a purchase outside the app.
 */
export default function Billing() {
  const core = useCore();
  const { uiLink } = useAuth();
  const web = useWebBilling();
  const theme = useTheme();
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [billing, setBilling] = useState<BillingResponse | null | undefined>();
  const [receipts, setReceipts] = useState<Receipt[] | undefined>();
  const [planCore, setPlanCore] = useState<typeof core | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [receiptsError, setReceiptsError] = useState<string | null>(null);
  // A receipt that did not open (its Stripe page is minted on the tap) says so under the list.
  const { opener, error: linkError } = useReceiptOpener(core);
  // A change that failed (auto top-up off), or a web page that did not open.
  const [changeError, setChangeError] = useState<string | null>(null);
  const owner = !!me?.permissions?.includes("org.billing");

  const load = useCallback(() => {
    core.me().then(setMe, (e: Error) => setError(e.message));
  }, [core]);
  // Again on coming back: from the usage limit's screen, or to the app from Nexus UI in Safari.
  useFocusEffect(load);
  useEffect(() => {
    const subscription = AppState.addEventListener("change", (state) => state === "active" && load());
    return () => subscription.remove();
  }, [load]);
  useEffect(() => {
    if (!owner) return;
    core
      .billing()
      .catch((e: unknown) => (noBilling(e) ? null : Promise.reject(e)))
      .then(
        (b) => {
          setBilling(b);
          setError(null);
          if (!b) return;
          core.receipts().then(
            (r) => {
              setReceipts(r.receipts);
              setReceiptsError(null);
            },
            (e: Error) => setReceiptsError(e.message),
          );
        },
        (e: Error) => setError(e.message),
      );
  }, [core, owner, me]);

  useEffect(() => {
    if (!owner || !web || !uiLink || !billing) return;
    let active = true;
    void core.billingPlans().then(
      (catalogue) => { if (active) setPlanCore(catalogue.plans.some(v3Plan) ? core : null); },
      () => { if (active) setPlanCore(null); },
    );
    return () => { active = false; };
  }, [billing, core, owner, web, uiLink]);

  if (me && !owner) {
    // A member who is not an owner: whose Billing is, where their own limit is, the owners and one tap to ask.
    return (
      <Page onRefresh={load}>
        <LargeTitle>Billing</LargeTitle>
        <View style={styles.member}>
          <Text style={[styles.body, { color: theme.foreground }]}>
            Billing is for {me.tenant.name}&apos;s owners. Your own limit shows in Usage.
          </Text>
          <AskOwner block="billing" theme={theme} layout="block" />
        </View>
      </Page>
    );
  }
  if (error && billing === undefined) {
    return (
      <Page onRefresh={load}>
        <Blank title="Billing didn't load" body={error} />
        <Section>
          <Row label={TRY_AGAIN} tone="primary" onPress={load} accessibilityRole="button" />
        </Section>
      </Page>
    );
  }
  if (billing === undefined) return <Loading alone />;
  if (billing === null) return <Blank title="Billing isn't on yet" body="Nexus doesn't take payments here yet, so nothing is charged." />;

  const plan = planOf(me?.tenant.kind === "personal", billing);
  // A paid plan only where Gateway sells plans (`NEXUS_PLANS`); it pays as it goes on top, as Nexus UI shows.
  const payg = plan.kind === "payg" || plan.kind === "paid";
  const payment = paymentLabel(billing);
  const atf = billing.afterTheFact;
  // The charge's phase decides what the row and the card say (kit's `afterTheFactNotice`).
  const pay = afterTheFactNoticeOf(billing);
  const limitLine = usageLimitLine(atf, pay);
  const promotional = promotionalLine(billing);
  const due = receipts ? openReceipts(receipts) : 0;
  const topupOff = topupOffLine(billing.autoTopup);
  const topupAlert = topupOffAlert(billing.autoTopup);
  const account = me?.tenant.kind === "personal" ? "Personal" : me?.tenant.name;
  const actions = web && uiLink ? webBillingActions(billing, payg, due, planCore === core) : [];

  // Off charges nothing, so every storefront has it; it asks first, as Nexus UI turns it off at once.
  const turnOffTopup = () =>
    Alert.alert(`Turn off ${BILLING_TERMS.autoTopup.toLowerCase()}?`, `${BILLING_TERMS.credits} are no longer bought when they run low.`, [
      { text: "Cancel", style: "cancel" },
      {
        text: "Turn off",
        style: "destructive",
        onPress: () => {
          setChangeError(null);
          core.setAutoTopup({ enabled: false }).then(load, (e: Error) => setChangeError(`${BILLING_TERMS.autoTopup} didn't change. ${e.message}`));
        },
      },
    ]);
  // Safari, outside the app, as guideline 3.1.1(a) asks of a US link; never an in-app browser.
  const openWeb = (path: string) => {
    if (!uiLink) return;
    setChangeError(null);
    Linking.openURL(uiLink(path)).catch(() => setChangeError("Nexus on the web didn't open. Try again."));
  };

  return (
    <Page onRefresh={load}>
      <LargeTitle>Billing</LargeTitle>

      {error ? (
        <View style={styles.stale}>
          <Footnote>{billingStaleNote(error)}</Footnote>
          <Section>
            <Row label={TRY_AGAIN} tone="primary" onPress={load} accessibilityRole="button" />
          </Section>
        </View>
      ) : null}

      {/* Amber, as every billing warning on every surface (brand v5 P73): a dot in the warning colour, text in grey. */}
      {billing.pastDue ? (
        <View style={[styles.alert, { backgroundColor: theme.card }]} accessibilityRole="alert">
          <View style={styles.alertHead}>
            <View style={[styles.dot, { backgroundColor: theme.waiting }]} />
            <Text style={[styles.strong, { color: theme.foreground }]}>The invoice for {monthLabel(billing.pastDue.period)} is unpaid.</Text>
          </View>
          <Text style={[styles.body, { color: theme.muted }]}>
            {billing.pastDue.blockedFrom === null
              ? "Stripe is still trying to charge the card."
              : `Model calls ${pastDueStopped(billing.pastDue) ? "stopped" : "stop"} on ${billingCutoff(billing.pastDue.blockedFrom)}${pastDueStopped(billing.pastDue) ? " and start again once it is paid" : " unless it is paid"}.`}
          </Text>
        </View>
      ) : null}

      {/* A charge an owner must act on. It is an alert only while it stops calls the card pays; read-only, as Billing is. */}
      {pay?.action ? (
        <View style={[styles.alert, { backgroundColor: theme.card }]} accessibilityRole={pay.stopsCalls ? "alert" : "summary"}>
          <View style={styles.alertHead}>
            <View style={[styles.dot, { backgroundColor: theme.waiting }]} />
            <Text style={[styles.strong, { color: theme.foreground }]}>{pay.title}.</Text>
          </View>
          {pay.body ? <Text style={[styles.body, { color: theme.muted }]}>{pay.body}.</Text> : null}
        </View>
      ) : null}

      {topupAlert ? (
        <View style={[styles.alert, { backgroundColor: theme.card }]} accessibilityRole="alert">
          <View style={styles.alertHead}>
            <View style={[styles.dot, { backgroundColor: theme.waiting }]} />
            <Text style={[styles.strong, { color: theme.foreground }]}>{topupAlert.title}</Text>
          </View>
          <Text style={[styles.body, { color: theme.muted }]}>{topupAlert.body}</Text>
        </View>
      ) : null}

      <Section title={account}>
        <Summary
          title={plan.kind === "paid" ? plan.name : payg ? "Pay as you go" : "Free"}
          line={payg ? paygLine(billing) : freeLine(billing)}
          left={payg ? undefined : freeStartLeft(billing.freeStart)}
        />
      </Section>

      {billing.planAllowance && <Section title="Monthly allowances">
        {allowanceRows(billing.planAllowance).map(({ label, detail }) => <Row key={label} label={label} detail={detail} />)}
      </Section>}

      {/* No way to pay yet: where the store allows a link, what pay as you go costs and one tap to set it
          up, in Safari. */}
      {!payg && actions.length ? (
        <Section title={BILLING_TERMS.payAsYouGo} footer={`${payAsYouGoPitch(billing.pricing)} Opens Nexus on the web in Safari.`}>
          {actions.map((action) => (
            <Row key={action.label} label={action.label} tone="primary" chevron onPress={() => openWeb(action.path)} accessibilityRole="link" accessibilityHint="Opens Nexus on the web in Safari" />
          ))}
        </Section>
      ) : null}

      {/* Every storefront: what keeps working with no way to pay. Neither row is a purchase. */}
      {!payg ? (
        <Section title="Always free">
          {ALWAYS_FREE.map((way) => (
            <Row key={way.href} label={way.label} detail={way.detail} href={way.href} />
          ))}
        </Section>
      ) : null}

      <Section
        footer={
          <>
            {linkError ? <Text style={{ color: theme.danger }}>{linkError} </Text> : null}
            {changeError ? <Text style={{ color: theme.danger }}>{changeError} </Text> : null}
            {billing.mode === "test" ? " Stripe test mode: no real charges." : ""}
            {` ${BILLING_TERMS.datesInUtc}`}
          </>
        }
      >
        {promotional && <Row label={BILLING_TERMS.promotionalCredit} value={promotional} monoValue detail="Referral and promotional rewards" />}
        {payg ? (
          <Row
            label={BILLING_TERMS.credits}
            value={billing.credits ? usd(billing.credits.remainingUsd) : "None"}
            monoValue
            valueTone="foreground"
            detail={topupOff ?? topupLine(billing.autoTopup)}
            detailTone={topupOff ? "primary" : undefined}
          />
        ) : null}
        {payg && billing.autoTopup?.enabled ? <Row label={`Turn off ${BILLING_TERMS.autoTopup.toLowerCase()}`} tone="primary" onPress={turnOffTopup} /> : null}
        {/* Also after paying after the fact was turned off with a failed charge or unpaid usage, when `planOf` says Free.
            While it is on, the row opens `usage-limit.tsx` to lower it: a lower limit buys nothing. */}
        {showsUsageLimit(plan, pay) ? (
          <Row
            label={BILLING_TERMS.usageLimit}
            value={atf?.enabled ? setting(atf.limitUsd) : "Off"}
            monoValue={!!atf?.enabled}
            detail={limitLine.text}
            detailTone={limitLine.problem ? "primary" : undefined}
            href={atf?.enabled ? "/usage-limit" : undefined}
            accessibilityHint={atf?.enabled ? "Lowers the monthly limit" : undefined}
          />
        ) : null}
        {payg || payment ? (
          <Row label="Payment" value={payment ?? "None"} />
        ) : null}
      </Section>

      {/* The US App Store only (`storefront.ts`): each row opens its part of Nexus UI's Billing in Safari. */}
      {payg && actions.length ? (
        <Section title="On the web" footer="Opens Nexus on the web in Safari.">
          {actions.map((action) => (
            <Row key={action.label} label={action.label} tone="primary" chevron onPress={() => openWeb(action.path)} accessibilityRole="link" accessibilityHint="Opens Nexus on the web in Safari" />
          ))}
        </Section>
      ) : null}

      {payg || receipts?.length ? (
        <Section title="Receipts">
          {receipts?.slice(0, RECEIPTS_SHOWN).map((r) => <ReceiptRow key={r.id} receipt={r} onPress={opener(r)} />)}
          {/* Every receipt, on its own screen, when this list shows only the latest. */}
          <Row
            label={moreReceipts(receipts) ? "All receipts" : "Receipts"}
            value={receipts ? (due ? `${due} open` : receipts.length ? "All paid" : "None") : undefined}
            valueTone={due ? "foreground" : undefined}
            detail={receiptsError ? `Didn't load. ${receiptsError}` : undefined}
            href={moreReceipts(receipts) ? "/receipts" : undefined}
          />
        </Section>
      ) : null}
    </Page>
  );
}

/** How the organization pays, big, and its one line under it; `left`, the free start's meter (what is left, 0 to 1). */
function Summary({ title, line, left }: { title: string; line: string; left?: number }) {
  const theme = useTheme();
  return (
    <View style={styles.summary} accessible accessibilityLabel={line ? `${title}. ${line}` : title}>
      <Text style={[styles.summaryTitle, { color: theme.foreground }]}>{title}</Text>
      {line ? <Text style={[styles.summaryLine, { color: theme.muted }]}>{line}</Text> : null}
      {left !== undefined ? <View style={styles.meter}><Share share={left} /></View> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  // Brand v5 P73·G's status card: radius 14, padding 16, on the raised colour.
  alert: { borderRadius: 14, borderCurve: "continuous", padding: 16, gap: 4 },
  alertHead: { flexDirection: "row", alignItems: "center", gap: 8 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  stale: { gap: 8 },
  strong: { flex: 1, fontSize: 17, lineHeight: 22, fontFamily: font.regular },
  body: { fontSize: 15, lineHeight: 21, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
  summary: { padding: 16, gap: 6 },
  meter: { paddingTop: 6 },
  member: { paddingHorizontal: 16, gap: 8 },
  summaryTitle: { fontSize: 20, lineHeight: 25, fontFamily: font.regular },
  summaryLine: { ...text.secondary, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
});
