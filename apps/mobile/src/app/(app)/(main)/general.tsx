import type { CurrentTenantResponse, MemberSummary, OrgDomain } from "@nexus/kit";
import { Stack } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { useCore } from "@/auth";
import { addressOf, domainLine, ENROLLMENT, ownerName } from "@/general";
import { Blank, Empty, LargeTitle, Loading, Page, Row, Section } from "@/list";

const day = (iso: string) => new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

/**
 * General, as brand v5's P71c·G4 draws it on the phone, read only: the organization's name and its address in Nexus UI,
 * the email domains people join by and how (owners and admins), and its owner. Renaming, domains, transferring and
 * deleting stay on a computer, which the foot says; so the board's chevron on Name is left out, and so is the count
 * of people who joined by a domain, which Gateway does not keep (it reports the invitations still waiting instead).
 */
export default function General() {
  const core = useCore();
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [domains, setDomains] = useState<OrgDomain[] | null>(null);
  const [members, setMembers] = useState<MemberSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    core.me().then(
      (r) => {
        setMe(r);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    // Owners and admins, on the Rust gateway: anyone else has no Joining section.
    core.domains().then((r) => setDomains(r.domains), () => setDomains(null));
    core.members().then((r) => setMembers(r.members), () => setMembers(null));
  }, [core]);
  useEffect(load, [load]);

  if (!me) return error ? <Blank title="General did not load" body={error} tone="danger" /> : <Loading alone />;
  const personal = me.tenant.kind === "personal";
  const name = personal ? "Personal" : me.tenant.name;
  const owner = members ? ownerName(members) : undefined;
  return (
    <Page onRefresh={load}>
      <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>General</LargeTitle>
      <Section title={name}>
        <Row label="Name" value={me.tenant.name} />
        <Row label="Address" value={addressOf(me.tenant)} monoValue />
        <Row label="Created" value={day(me.tenant.createdAt)} />
      </Section>
      {!personal && domains ? (
        <Section title="Joining" footer="People with a verified email on a domain join as it says.">
          {domains.length ? null : <Empty>No domains. People join by invitation.</Empty>}
          {domains.map((d) => (
            <Row key={d.id} label={d.domain} detail={domainLine(d)} value={ENROLLMENT[d.enrollmentMode]} />
          ))}
        </Section>
      ) : null}
      {!personal && owner ? (
        <Section title="Ownership" footer={`Rename, transfer or delete ${name} on a computer.`}>
          <Row label="Owner" value={owner} />
        </Section>
      ) : (
        <Section footer={personal ? "Your personal organization: you are its one member." : `Rename, transfer or delete ${name} on a computer.`} />
      )}
    </Page>
  );
}
