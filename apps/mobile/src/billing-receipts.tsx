import type { CoreClient, Receipt } from "@nexus/kit";
import { useState } from "react";
import { receiptLine, receiptLink, usd } from "@/billing";
import { Row } from "@/list";
import { open } from "@/markdown";

/**
 * Opens a receipt outside the app as `receiptLink` allows: Stripe's receipt page, minted on the tap, or a settled month's
 * invoice. A link that did not open says so (`error`), under the list that shows it.
 */
export function useReceiptOpener(core: Pick<CoreClient, "receipt">) {
  const [error, setError] = useState<string | null>(null);
  const opener = (r: Receipt) => {
    const link = receiptLink(r);
    if (link === "receipt") {
      return () => {
        setError(null);
        core.receipt(r.id).then((answer) => open(answer.url), (e: Error) => setError(`The receipt did not open. ${e.message}`));
      };
    }
    if (link === "invoice" && r.url) {
      const url = r.url;
      return () => open(url);
    }
    return undefined;
  };
  return { opener, error };
}

/** One receipt: what it paid for, its day and state, its amount, and a chevron when it opens outside the app. */
export function ReceiptRow({ receipt, onPress }: { receipt: Receipt; onPress?: () => void }) {
  return (
    <Row
      label={receipt.description}
      detail={receiptLine(receipt)}
      detailTone={receipt.status === "failed" ? "danger" : undefined}
      value={usd(receipt.amountUsd)}
      monoValue
      valueTone="foreground"
      chevron={!!onPress}
      onPress={onPress}
      accessibilityRole={onPress ? "link" : undefined}
    />
  );
}
