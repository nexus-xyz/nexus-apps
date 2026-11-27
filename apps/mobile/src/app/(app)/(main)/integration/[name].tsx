import type { AgentWriteSetting, ChatWriteSetting, Connection, ConnectorAvailability, ConnectorDetail, TeamSummary } from "@nexus/kit";
import { Stack, useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { connectorLabel } from "@/activity";
import { useAuth, useCore } from "@/auth";
import { Fact } from "@/fact";
import { canDo, canDoLine, connectionStatus, integrationsOn, whoLine } from "@/integrations";
import { Blank, Empty, Footnote, LargeTitle, Loading, Page, Row, Section } from "@/list";

const day = (iso: string) => new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

/**
 * One connector, full screen (brand v5's P105·D "An item", from P11·C): what it is for, who may use it and when that
 * last changed, the accounts it runs with, and what it can do, each operation a read or a change. It has no action on
 * the phone: connecting, signing in again and choosing who may use it are on a computer, which its foot says.
 * With `EXPO_PUBLIC_NEXUS_INTEGRATIONS=on` (Assistant board A6), it also says what Nexus can do with your
 * accounts there, in Nexus UI's words.
 */
const A6 = integrationsOn(process.env.EXPO_PUBLIC_NEXUS_INTEGRATIONS);

export default function Integration() {
  const { name } = useLocalSearchParams<{ name: string }>();
  const core = useCore();
  const { orgName } = useAuth();
  const [connector, setConnector] = useState<ConnectorDetail | null>(null);
  const [availability, setAvailability] = useState<ConnectorAvailability | undefined>();
  const [connections, setConnections] = useState<Connection[]>([]);
  const [teams, setTeams] = useState<TeamSummary[]>([]);
  const [chat, setChat] = useState<ChatWriteSetting | undefined>();
  const [mine, setMine] = useState<AgentWriteSetting | undefined>();
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    core.describeConnector(name).then(
      (c) => {
        setConnector(c);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    core.connectorAvailability().then((r) => setAvailability(r.connectors.find((c) => c.name === name)), () => undefined);
    core.connections().then((r) => setConnections(r.connections.filter((c) => c.connector === name)), () => undefined);
    core.teams().then((r) => setTeams(r.teams), () => undefined);
    if (A6) {
      core.chatWrites().then(setChat, () => setChat(undefined));
      core.agentWrites().then(setMine, () => setMine(undefined));
    }
  }, [core, name]);
  useEffect(load, [load]);

  const title = connectorLabel(name);
  if (!connector) return error ? <Blank title={`Couldn't load ${title}`} body={error} tone="danger" /> : <Loading alone />;
  const org = orgName ?? "";
  // One this Gateway does not offer is off for everyone, whatever the organization chose: say only that.
  const offered = connector.offered !== false;
  const who = offered ? whoLine(availability, connector.usable, org, (slug) => teams.find((t) => t.slug === slug)?.name ?? slug) : undefined;
  const choice = offered ? availability : undefined;
  return (
    <Page onRefresh={load}>
      <Stack.Screen options={{ title, headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>{title}</LargeTitle>
      {connector.description ? <Footnote>{connector.description}</Footnote> : null}
      <Section>
        {who ? <Fact label="Who may use it" value={who} /> : null}
        {A6 && connections.length ? <Fact label="What Nexus can do" value={canDoLine(canDo(name, chat, mine), org)} /> : null}
        {choice?.changedAt ? <Fact label="Changed" value={day(choice.changedAt)} /> : null}
        {choice?.default ? <Fact label="Comes from" value="The connector's default" /> : null}
        {connector.stage === "preview" ? <Fact label="Release" value={offered ? "Preview" : "In preview, not offered here yet"} /> : null}
      </Section>
      {offered ? null : <Footnote>{`This Gateway does not offer ${title} yet. Its accounts are kept, and no call uses them.`}</Footnote>}
      {connections.length ? (
        <Section title="Accounts">
          {connections.map((c) => (
            <Row
              key={`${c.scope}:${c.account}`}
              label={c.name}
              detail={[c.scope === "org" ? `${org || "The organization"}'s` : "Yours", c.identity].filter(Boolean).join(" · ")}
              value={connectionStatus(c)}
              valueTone={c.status === "needs_reconnect" ? "foreground" : undefined}
            />
          ))}
        </Section>
      ) : null}
      <Section title="What it can do">
        {connector.operations.length ? null : <Empty>No operations.</Empty>}
        {connector.operations.map((op) => (
          <Row key={op.name} label={op.summary || op.name} detail={op.name} value={op.effect === "read" ? "Reads" : op.effect === "mutate" ? "Changes" : undefined} />
        ))}
      </Section>
      <Footnote>Connect it, sign in again or choose who may use it on a computer.</Footnote>
    </Page>
  );
}
