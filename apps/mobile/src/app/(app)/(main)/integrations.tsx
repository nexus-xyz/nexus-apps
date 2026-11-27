import type { AgentWriteSetting, ChatWriteSetting, Connection, ConnectorAvailability, ConnectorSummary, TeamSummary } from "@nexus/kit";
import * as WebBrowser from "expo-web-browser";
import { Stack, useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import { connectorLabel } from "@/activity";
import { useAuth, useCore, useStored } from "@/auth";
import { haptic } from "@/haptics";
import { ConnectorMark } from "@/connector-mark";
import { accountServices, accountsLine, byOffer, canDo, canDoLine, connectorAction, integrationsOn, PREVIEW_NOTE, whoLine } from "@/integrations";
import { Empty, Footnote, LargeTitle, Loading, Page, Row, Section } from "@/list";
import { CustomizeTabs, useCustomize } from "@/customize-tabs";

/**
 * Integrations, as brand v5's P11·C draws it on the phone: the title in the list, then the connectors this Gateway
 * offers, each with who may use it in the organization (owners and admins read the organization's choice; a member
 * whether it is on for them) and what it waits on from you (Connect, Sign in again). A connector opens its own page
 * (`integration/[name]`). Customize also links directly to Nexus on the web to connect an account. Requests and MCP
 * servers are left out: Gateway has no requests to use a connector and no MCP servers to list yet.
 * The connectors this Gateway does not offer follow under "In preview", as Nexus UI and desktop list them.
 * With `EXPO_PUBLIC_NEXUS_INTEGRATIONS=on` (Assistant board A6), it starts with your accounts: one row per
 * service, its accounts inside, and what Nexus can do with it, in Nexus UI's words.
 */
const A6 = integrationsOn(process.env.EXPO_PUBLIC_NEXUS_INTEGRATIONS);

export default function Integrations() {
  const customize = useCustomize();
  const core = useCore();
  const { orgName, uiLink } = useAuth();
  const cache = useStored<ConnectorSummary[]>("cache", "connectors");
  const [connectors, setConnectors] = useState<ConnectorSummary[] | null>(() => cache.load() ?? null);
  const [availability, setAvailability] = useState<ConnectorAvailability[]>([]);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [teams, setTeams] = useState<TeamSummary[]>([]);
  // A6: the organization's agent writes and your own setting; undefined when off or not loaded, so no write is claimed.
  const [chat, setChat] = useState<ChatWriteSetting | undefined>();
  const [mine, setMine] = useState<AgentWriteSetting | undefined>();
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    core.connectors().then(
      (r) => {
        setConnectors(r.connectors);
        cache.save(r.connectors);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    // Each on its own, and each may be refused (a member reads no availability; an older Gateway has no connections).
    core.connectorAvailability().then((r) => setAvailability(r.connectors), () => setAvailability([]));
    core.connections().then((r) => setConnections(r.connections), () => setConnections([]));
    core.teams().then((r) => setTeams(r.teams), () => undefined);
    if (A6) {
      core.chatWrites().then(setChat, () => setChat(undefined));
      core.agentWrites().then(setMine, () => setMine(undefined));
    }
  }, [core, cache]);
  useFocusEffect(load);
  const connect = async (name: string) => {
    if (!uiLink) return;
    haptic("tap");
    try {
      await WebBrowser.openBrowserAsync(uiLink(`/settings/connectors?open=${encodeURIComponent(name)}`));
      load();
    } catch {
      haptic("error");
      setError("Nexus on the web didn’t open. Try again.");
    }
  };
  if (!connectors && !error && !customize) return <Loading alone />;
  const { offered, preview } = byOffer(connectors ?? []);
  const teamName = (slug: string) => teams.find((t) => t.slug === slug)?.name ?? slug;
  return (
    <Page onRefresh={load}>
      <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      {customize ? <CustomizeTabs active="/integrations" /> : <LargeTitle>Integrations</LargeTitle>}
      {!connectors && !error ? <Loading /> : null}
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      {A6 && connections.length ? (
        <Section title={accountsLine(connections)} footer={customize && uiLink ? "Manage connected accounts in Nexus on the web." : "Connect, sign in again or disconnect on a computer."}>
          {accountServices(connections, connectorLabel).map((s) => (
            <Row
              key={s.connector}
              leading={<ConnectorMark name={s.connector} />}
              label={connectorLabel(s.connector)}
              detail={s.value === "Sign in again" ? "Nexus cannot use it until you sign in again." : canDoLine(canDo(s.connector, chat, mine), orgName ?? "")}
              value={s.value}
              valueTone={s.value === "Sign in again" ? "foreground" : undefined}
              href={{ pathname: "/integration/[name]", params: { name: s.connector } }}
            />
          ))}
        </Section>
      ) : null}
      <Section title={offered.length ? `Connectors · ${offered.length}` : "Connectors"} footer={customize && uiLink ? "Connect opens the account in Nexus on the web." : "Connect accounts and choose who may use them on a computer."}>
        {connectors && !offered.length ? <Empty>No connectors on this Gateway yet.</Empty> : null}
        {offered.map((c) => {
          const action = connectorAction(c, connections);
          const openConnection = customize && uiLink && (action === "Connect" || action === "Sign in again");
          return (
            <Row
              key={c.name}
              leading={<ConnectorMark name={c.name} />}
              label={connectorLabel(c.name)}
              detail={whoLine(
                availability.find((a) => a.name === c.name),
                c.usable,
                orgName ?? "",
                teamName,
              ) ?? (c.description || undefined)}
              value={action}
              valueTone={openConnection || action === "Sign in again" ? "foreground" : undefined}
              href={openConnection ? undefined : { pathname: "/integration/[name]", params: { name: c.name } }}
              accessibilityHint={openConnection ? "Opens account authorization in Nexus on the web" : undefined}
              onPress={openConnection ? () => void connect(c.name) : undefined}
            />
          );
        })}
      </Section>
      {preview.length ? (
        <Section title={`In preview · ${preview.length}`} footer={PREVIEW_NOTE}>
          {preview.map((c) => (
            <Row
              key={c.name}
              leading={<ConnectorMark name={c.name} />}
              label={connectorLabel(c.name)}
              detail={c.description || undefined}
              href={{ pathname: "/integration/[name]", params: { name: c.name } }}
            />
          ))}
        </Section>
      ) : null}
    </Page>
  );
}
