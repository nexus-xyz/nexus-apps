import {
  vaultApprovalLabel,
  vaultGrantLabel,
  vaultLimit,
  vaultSpent,
  vaultWhen,
  type AgentClient,
  type AgentVaultChanged,
  type AgentVaultView,
  type AgentVaultWrite,
  type VaultAsk,
} from "@nexus/kit";
import { useEffect, useState } from "react";
import { Alert } from "react-native";
import { useAuth } from "@/auth";
import { Blank, Footnote, Loading, Page, Row, Section } from "@/list";
import { useToast } from "@/toast";
import { VaultGrantSheet } from "@/vault-grant-sheet";
import { cardText, CHANGE_ON_DESKTOP, decidedText, failedText, LIVES_ON_DESKTOP, loginSite, managerLine, pageOf } from "@/vault-words";

const zone = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
};

type Page = { view: AgentVaultView; unlinked: boolean } | "failed" | undefined;

/**
 * Account › Assistant › Passwords and payments (A8): the asks that wait, each opening the grant sheet; the
 * sources linked on the person's desktop and the logins Nexus may ask to use, each with its site, masked
 * account, source, grant and last use. The logins live in the Nexus desktop app, and so do their grants: the desktop
 * sends an index and the phone only draws it, with no Revoke. The Nexus card with its month and its limits per merchant, and Freeze. With no
 * password manager or card linked, or the vault off at the agent API, it says that nothing is linked. Every read goes
 * through the kit's readers, which refuse a body shaped like a secret: the page has no password or card number to show.
 */
export default function Vault() {
  const { agent } = useAuth();
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  return <VaultPage agent={agent} />;
}

function VaultPage({ agent }: { agent: AgentClient }) {
  const toast = useToast();
  const [page, setPage] = useState<Page>();
  const [reads, setReads] = useState(0);
  const [working, setWorking] = useState(false);
  const [asking, setAsking] = useState<VaultAsk | null>(null);
  const [now] = useState(Date.now);
  const [timeZone] = useState(zone);

  useEffect(() => {
    const controller = new AbortController();
    agent.vault(controller.signal).then(
      (read) => !controller.signal.aborted && setPage(pageOf(read)),
      () => !controller.signal.aborted && setPage("failed"),
    );
    return () => controller.abort();
  }, [agent, reads]);

  /** Runs one write, keeps the vault it answers, and says what happened. */
  const write = (run: () => Promise<AgentVaultWrite>, said: (changed: AgentVaultChanged) => string) => {
    if (working) return;
    setWorking(true);
    run().then(
      (result) => {
        setWorking(false);
        if (result.kind === "failed") return Alert.alert("That was not done", failedText(result));
        setPage({ view: result.changed.vault, unlinked: false });
        toast({ text: said(result.changed), haptic: "success" });
      },
      () => {
        setWorking(false);
        Alert.alert("That was not done", "That did not save. Try again in a moment.");
      },
    );
  };

  if (page === undefined) return <Loading alone />;
  if (page === "failed") return <Blank title="Passwords and payments did not open" body="Try again in a moment." tone="danger" />;
  const { view, unlinked } = page;
  const manager = managerLine(view, unlinked);
  const card = view.card;

  return (
    <Page onRefresh={() => setReads((count) => count + 1)}>
      <Footnote>Give access without giving secrets. Every use is in the log.</Footnote>
      {view.asks.length ? (
        <Section title="Waiting for you">
          {view.asks.map((ask) => (
            <Row key={ask.id} label={`Use your ${ask.login.name} login?`} detail={`${ask.site} · ${ask.task}`} detailTone="tertiary" chevron onPress={() => setAsking(ask)} />
          ))}
        </Section>
      ) : null}
      <Section title="Passwords" footer={`${manager.line} ${LIVES_ON_DESKTOP} ${CHANGE_ON_DESKTOP}`}>
        <Row label={manager.title} icon="keys" />
        {view.logins.map((login) => (
          <Row key={login.id} label={login.name} detail={`${loginSite(login)} · ${vaultGrantLabel(login.grant, timeZone)}`} detailTone="tertiary" value={vaultWhen(login.lastUsed, now, timeZone)} disabled={!login.grant} />
        ))}
      </Section>
      <Section
        title="Payments"
        footer={card ? (view.applePay ? "Apple Pay works too: Face ID each time, never saved." : undefined) : "Nexus can’t pay for anything. A payment it starts opens on the merchant’s own page, for you to finish."}
      >
        {card ? (
          <>
            <Row label={`Nexus card · virtual ••${card.last4}`} detail={card.state === "frozen" ? "Frozen" : undefined} detailTone="tertiary" value={vaultSpent(card)} icon="wallet" />
            {view.merchants.map((merchant) => (
              <Row key={merchant.id} label={merchant.merchant} detail={vaultLimit(merchant)} detailTone="tertiary" value={vaultApprovalLabel(merchant.approval)} />
            ))}
            <Row
              label={card.state === "frozen" ? "Unfreeze" : "Freeze"}
              tone="primary"
              disabled={working}
              onPress={() => write(() => agent.vaultWrite({ frozen: card.state !== "frozen" }), cardText)}
            />
          </>
        ) : (
          <Row label="No Nexus card is linked" icon="wallet" />
        )}
      </Section>
      <Section>
        <Row label="Activity" detail="Every password used and every payment" detailTone="tertiary" href="/vault-activity" />
      </Section>
      <VaultGrantSheet
        ask={asking}
        working={working}
        timeZone={timeZone}
        onClose={() => setAsking(null)}
        onDecide={(ask, decision) => {
          setAsking(null);
          write(() => agent.vaultWrite({ ask: ask.id, decision }), (changed) => decidedText(ask, changed));
        }}
      />
    </Page>
  );
}
