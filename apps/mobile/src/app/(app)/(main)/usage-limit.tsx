import { BILLING_TERMS, billingDay, usageLimitResetsAt, type AfterTheFact } from "@nexus/kit";
import { router } from "expo-router";
import { useEffect, useState } from "react";
import { useCore } from "@/auth";
import { limitInput, lowerLimitProblem, setting, usd } from "@/billing";
import { Blank, Busy, Field, Footnote, Loading, Page, Row, Section } from "@/list";

/**
 * Lower the monthly usage limit, from Billing's usage limit row. A lower limit buys nothing, so every
 * storefront has it; raising it opens Nexus UI where the store allows a link (`storefront.ts`). Owners only: Gateway
 * refuses anyone else.
 */
export default function UsageLimit() {
  const core = useCore();
  const [after, setAfter] = useState<AfterTheFact | null | undefined>();
  const [limit, setLimit] = useState("");
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    core.billing().then(
      (billing) => setAfter(billing.afterTheFact ?? null),
      (e: Error) => setError(e.message),
    );
  }, [core]);

  if (after === undefined && !error) return <Loading alone />;
  if (!after?.enabled) return <Blank title="No usage limit to change" body={error ?? `${BILLING_TERMS.payAfterTheFact} is off.`} />;

  const problem = limit ? lowerLimitProblem(limit, after.limitUsd) : null;
  const save = async () => {
    if (busy || !limit || problem) return;
    setBusy(true);
    setError(undefined);
    try {
      await core.setAfterTheFact({ enabled: true, limitUsd: limitInput(limit) });
      router.back();
    } catch (e) {
      setError(e instanceof Error ? e.message : "The limit didn't change.");
      setBusy(false);
    }
  };

  return (
    <Page>
      <Section footer={`${usd(after.monthSpentUsd)} used this month. Above the limit, calls the card pays pause until ${billingDay(usageLimitResetsAt())}.`}>
        <Row label={BILLING_TERMS.usageLimit} value={setting(after.limitUsd)} monoValue />
      </Section>
      <Section title="New limit a month" footer={problem ?? "The phone only lowers the limit."} footerTone={problem ? "danger" : undefined}>
        <Field
          value={limit}
          onChangeText={setLimit}
          placeholder="$"
          keyboardType="decimal-pad"
          monoText
          autoFocus
          editable={!busy}
          accessibilityLabel="New usage limit in dollars"
          onSubmitEditing={() => void save()}
        />
      </Section>
      {error ? <Footnote tone="danger">{`The limit didn't change. ${error}`}</Footnote> : null}
      <Section>
        <Row label={busy ? "Saving…" : "Lower limit"} tone="primary" center disabled={busy || !limit || !!problem} accessory={busy ? <Busy /> : undefined} onPress={() => void save()} />
      </Section>
    </Page>
  );
}
