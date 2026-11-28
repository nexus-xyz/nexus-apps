import { BILLING_TERMS, TRY_AGAIN, type Receipt } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { Text } from "react-native";
import { useCore } from "@/auth";
import { receiptsByMonth } from "@/billing";
import { ReceiptRow, useReceiptOpener } from "@/billing-receipts";
import { Blank, Loading, Page, Row, Section } from "@/list";
import { useTheme } from "@/theme";

/**
 * Every receipt, newest first, from Billing's "All receipts" (brand v5's P40·D). Read-only, as Billing is:
 * a payment's receipt and a settled month's invoice open outside the app, and nothing here takes a payment.
 */
export default function Receipts() {
  const core = useCore();
  const theme = useTheme();
  const [receipts, setReceipts] = useState<Receipt[] | undefined>();
  const [error, setError] = useState<string | null>(null);
  const { opener, error: linkError } = useReceiptOpener(core);

  const load = useCallback(() => {
    core.receipts().then(
      (r) => {
        setReceipts(r.receipts);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
  }, [core]);
  useEffect(load, [load]);

  if (error && receipts === undefined) {
    return (
      <Page onRefresh={load}>
        <Blank title="Receipts didn't load" body={error} />
        <Section>
          <Row label={TRY_AGAIN} tone="primary" onPress={load} accessibilityRole="button" />
        </Section>
      </Page>
    );
  }
  if (receipts === undefined) return <Loading alone />;
  if (receipts.length === 0) return <Blank title="No receipts yet" body="A payment shows here once Stripe takes it." />;
  return (
    <Page onRefresh={load}>
      {/* One section per UTC month, newest first: a long list scans by month, as iOS's lists do. */}
      {receiptsByMonth(receipts).map(({ month, receipts: rows }, index, groups) => (
        <Section
          key={month}
          title={month}
          footer={
            index === groups.length - 1 ? (
              <>
                {linkError ? <Text style={{ color: theme.danger }}>{linkError} </Text> : null}
                {BILLING_TERMS.datesInUtc}
              </>
            ) : undefined
          }
        >
          {rows.map((r) => <ReceiptRow key={r.id} receipt={r} onPress={opener(r)} />)}
        </Section>
      ))}
    </Page>
  );
}
