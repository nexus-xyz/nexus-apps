import { cloudSessionTitle } from "@nexus/kit/cloud-session-helpers";
import { Stack, useIsFocused } from "expo-router";
import { useCore } from "@/auth";
import { cloudStatus } from "@/cloud-sessions";
import { Empty, Footnote, Loading, Page, Row, Section } from "@/list";
import { useCloudSessions } from "@/use-cloud-sessions";

export default function CloudSessions() {
  const core = useCore();
  const focused = useIsFocused();
  const { result, loading, refresh } = useCloudSessions(core, focused);
  return <Page onRefresh={refresh} refreshing={loading && !!result}>
    <Stack.Screen options={{ title: "Cloud sessions" }} />
    {!result ? <Loading /> : null}
    {result?.state === "disabled" ? <Footnote>This Gateway does not have Cloud sessions enabled.</Footnote> : null}
    {result?.state === "error" ? <Section footer={result.error} footerTone="danger"><Row label="Retry" onPress={refresh} /></Section> : null}
    {result?.state === "ready" ? <>
      <Section><Row label="New Cloud session" icon="plus" tone="primary" href="/cloud-sessions/new" /></Section>
      <Section title="Recent">
        {result.sessions.map((session) => <Row key={session.id} label={cloudSessionTitle(session)} detail={session.repo}
          value={cloudStatus(session)} href={{ pathname: "/cloud-sessions/[id]", params: { id: session.id } }} />)}
      </Section>
      {!result.sessions.length ? <Empty>Your Cloud sessions will appear here.</Empty> : null}
    </> : null}
  </Page>;
}
