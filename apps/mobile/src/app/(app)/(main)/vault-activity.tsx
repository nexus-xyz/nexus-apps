import { vaultLogWhen, type AgentVaultActivity } from "@nexus/kit";
import { useEffect, useState } from "react";
import { useAuth } from "@/auth";
import { Blank, Empty, Footnote, Loading, Page, Row, Section } from "@/list";

const zone = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
};

/**
 * Passwords and payments › Activity (A8·e): every password Nexus used and every payment, with who approved
 * it and where, newest first. The log names logins and cards as the agent API masks them, never a secret.
 */
export default function VaultActivity() {
  const { agent } = useAuth();
  const [read, setRead] = useState<AgentVaultActivity | "unreachable">();
  const [reads, setReads] = useState(0);
  const [now] = useState(Date.now);
  const [timeZone] = useState(zone);

  useEffect(() => {
    if (!agent) return;
    const controller = new AbortController();
    agent.vaultActivity(controller.signal).then(
      (result) => !controller.signal.aborted && setRead(result),
      () => !controller.signal.aborted && setRead("unreachable"),
    );
    return () => controller.abort();
  }, [agent, reads]);

  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  if (read === undefined) return <Loading alone />;
  if (read === "unreachable" || read.kind === "failed") return <Blank title="The activity did not open" body="Try again in a moment." tone="danger" />;
  const entries = read.kind === "entries" ? read.entries : [];
  return (
    <Page onRefresh={() => setReads((count) => count + 1)}>
      <Footnote>Every password used and every payment, with who approved it and where.</Footnote>
      <Section>
        {entries.length ? (
          entries.map((entry, i) => (
            <Row
              key={`${entry.at}:${i}`}
              label={entry.what}
              detail={[entry.with ?? "—", entry.approved ?? "—"].join(" · ")}
              detailTone="tertiary"
              value={`${vaultLogWhen(entry.at, now, timeZone)} · ${entry.result}`}
              tone={entry.outcome === "refused" ? "danger" : undefined}
            />
          ))
        ) : (
          <Empty>Nothing yet. Each password Nexus uses and each payment shows here.</Empty>
        )}
      </Section>
    </Page>
  );
}
