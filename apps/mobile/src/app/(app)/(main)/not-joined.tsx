import type { CurrentTenantResponse, PendingPerson } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { useCore } from "@/auth";
import { Blank, Empty, Loading, Page, Row, Section } from "@/list";
import { AdminHead } from "@/people-controls";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { useToast } from "@/toast";

/**
 * Not joined yet (P71e·F's first rows, on P105·D): the people added to a team by an email that has not joined the
 * organization, one row each with their teams, drilled into from Members and Teams. Tap someone to cancel their place in
 * a team. Owners and admins only, as Gateway answers it.
 */
export default function NotJoined() {
  const core = useCore();
  const toast = useToast();
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [people, setPeople] = useState<PendingPerson[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [picked, setPicked] = useState<PendingPerson | null>(null);

  const load = useCallback(() => {
    core.me().then(setMe, () => undefined);
    Promise.all([core.pendingPeople(), core.members()]).then(
      ([pending, members]) => {
        // Compared lower-cased: the member list and the team lists may not agree on case.
        const joined = new Set(members.members.map((m) => m.email?.toLowerCase()).filter(Boolean));
        setPeople(pending.people.filter((p) => !joined.has(p.email.toLowerCase())));
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
  }, [core]);
  useEffect(load, [load]);

  const cancel = (person: PendingPerson, team: PendingPerson["teams"][number]) => {
    setPicked(null);
    core.cancelPendingTeamMembers({ email: person.email, team: team.slug }).then(
      () => {
        toast({ text: `${person.email} won't join ${team.name}` });
        load();
      },
      (e: Error) => toast({ text: e.message }),
    );
  };

  if (!people) return error ? <Blank title="Couldn't load who is waiting" body={error} tone="danger" /> : <Loading alone />;
  return (
    <Page onRefresh={load}>
      <AdminHead title="Not joined yet" purpose={`Added to a team by email, not in ${me?.tenant.name ?? "the organization"} yet. They join their teams when they sign up.`} />
      <Section footer={people.length ? "Tap someone to cancel their place in a team." : undefined}>
        {people.length === 0 ? <Empty>No one is waiting to join.</Empty> : null}
        {people.map((p) => (
          <Row
            key={p.email}
            label={p.email}
            detail={`${p.teams.map((t) => `${t.name}${t.role === "lead" ? " (lead)" : ""}`).join(", ")}${p.home ? ` · joins ${p.home} as their default` : ""}`}
            onPress={() => setPicked(p)}
            accessibilityHint="Cancels their place in a team"
          />
        ))}
      </Section>
      <Sheet visible={!!picked} onClose={() => setPicked(null)}>
        <SheetTitle>{picked?.email}</SheetTitle>
        {picked?.teams.map((t) => <SheetRow key={t.slug} label={`Cancel in ${t.name}`} tone="danger" onPress={() => cancel(picked, t)} />)}
        <SheetRow label="Close" tone="muted" onPress={() => setPicked(null)} />
      </Sheet>
    </Page>
  );
}
