import type { CurrentTenantResponse, TeamDetail, TeamMember, UsageReport } from "@nexus/kit";
import { useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { spentText } from "@/analytics";
import { useCore } from "@/auth";
import { Blank, Empty, Loading, Page, Row, Section } from "@/list";
import { AdminHead, SheetField, SheetForm, SheetSwitch } from "@/people-controls";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { useToast } from "@/toast";

type Picked = { kind: "member"; member: TeamMember } | { kind: "pending"; id: string; email: string };

/**
 * A team's people (P71c·T5's People, on P105·D): each member with their role in it, whether it is their default team
 * and their 30-day spend, then the people who have not joined yet. Tap a person for what you may do: make it your
 * default, or for owners and admins remove them or cancel a pending place. + adds a person in a sheet.
 */
export default function TeamPeople() {
  const core = useCore();
  const toast = useToast();
  const { slug } = useLocalSearchParams<{ slug: string }>();
  const [team, setTeam] = useState<TeamDetail | null>(null);
  const [usage, setUsage] = useState<UsageReport | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [picked, setPicked] = useState<Picked | null>(null);
  const [adding, setAdding] = useState(false);

  const load = useCallback(() => {
    core.team(slug).then(
      (t) => {
        setTeam(t);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    // Members of other teams cannot see its usage (403): it is optional.
    core.usage({ team: slug, groupBy: "person" }).then(setUsage, () => setUsage(null));
  }, [core, slug]);
  useEffect(load, [load]);
  useEffect(() => {
    core.me().then(setMe, () => undefined);
  }, [core]);

  if (!team) return error ? <Blank title="Couldn't load this team" body={error} tone="danger" /> : <Loading alone />;
  const manage = !!me?.permissions?.includes("teams.manage");
  const self = me?.user?.id;
  const act = (work: () => Promise<unknown>, done: string) => {
    setPicked(null);
    work().then(
      () => {
        toast({ text: done });
        load();
      },
      (e: Error) => toast({ text: e.message }),
    );
  };

  return (
    <Page onRefresh={load}>
      <AdminHead title="People" purpose={`In ${team.name}, with what each spent in the last 30 days.`} add={manage ? { label: "Add a person", onPress: () => setAdding(true) } : undefined} />
      <Section>
        {team.memberList.length === 0 && team.pendingList.length === 0 ? (
          <Empty>No one in this team yet. {manage ? "Add people with +." : "An owner or admin adds people."}</Empty>
        ) : null}
        {team.memberList.map((member) => {
          const spent = usage?.groups.find((g) => g.key === member.userId);
          const can = manage || (member.userId === self && !member.isDefault);
          return (
            <Row
              key={member.userId}
              label={`${member.name || member.email || member.userId}${member.userId === self ? " (you)" : ""}`}
              detail={[member.role === "lead" ? "Lead" : "Member", member.isDefault ? "their default team" : undefined, member.name ? member.email : undefined].filter(Boolean).join(" · ")}
              value={spent ? spentText(spent) : undefined}
              monoValue
              onPress={can ? () => setPicked({ kind: "member", member }) : undefined}
            />
          );
        })}
        {team.pendingList.map((entry) => (
          <Row
            key={entry.id}
            label={entry.email}
            tone="muted"
            detail={`Not joined yet · ${entry.role === "lead" ? "Lead" : "Member"}${entry.home ? " · this team when they join" : ""}`}
            onPress={manage ? () => setPicked({ kind: "pending", id: entry.id, email: entry.email }) : undefined}
          />
        ))}
      </Section>

      <Sheet visible={!!picked} onClose={() => setPicked(null)}>
        {picked?.kind === "member" ? (
          <>
            <SheetTitle>{picked.member.name || picked.member.email || picked.member.userId}</SheetTitle>
            {picked.member.userId === self && !picked.member.isDefault ? (
              <SheetRow label="Make this my default team" onPress={() => act(() => core.setDefaultTeam(team.slug), `${team.name} is now your default team`)} />
            ) : null}
            {manage ? (
              <SheetRow
                label={`Remove from ${team.name}`}
                tone="danger"
                onPress={() => act(() => core.removeTeamMember(team.slug, picked.member.userId), `Removed ${picked.member.email ?? picked.member.userId} from ${team.name}`)}
              />
            ) : null}
          </>
        ) : picked?.kind === "pending" ? (
          <>
            <SheetTitle>{picked.email}</SheetTitle>
            <SheetRow label="Cancel their place" tone="danger" onPress={() => act(() => core.cancelPendingTeamMembers({ id: picked.id }), `${picked.email} won't join ${team.name}`)} />
          </>
        ) : null}
        <SheetRow label="Close" tone="muted" onPress={() => setPicked(null)} />
      </Sheet>
      <AddMember visible={adding} onClose={() => setAdding(false)} team={team} onAdded={load} />
    </Page>
  );
}

/** Add a person by email, as a member or a lead, optionally as their default team; someone not yet in the organization joins when they sign up. */
function AddMember({ visible, onClose, team, onAdded }: { visible: boolean; onClose: () => void; team: TeamDetail; onAdded: () => void }) {
  const core = useCore();
  const toast = useToast();
  const [email, setEmail] = useState("");
  const [lead, setLead] = useState(false);
  const [home, setHome] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const typed = email.trim().toLowerCase();

  const add = () => {
    if (!typed.includes("@")) return setError("Enter the person's email.");
    setBusy(true);
    setError(null);
    core
      .addTeamMember(team.slug, { email: typed, role: lead ? "lead" : "member", ...(home ? { home: true } : {}) })
      .then((added) => {
        setEmail("");
        setHome(false);
        toast({
          text: added.pending
            ? `${added.entry.email} joins ${team.name} when they sign up`
            : `Added ${added.member.email ?? added.member.userId} to ${team.name}${added.member.isDefault ? ", their default team" : ""}`,
        });
        onAdded();
        onClose();
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };

  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetForm title={`Add to ${team.name}`} action="Add" onSubmit={add} disabled={!typed} busy={busy} note="Someone not in your organization yet joins when they sign up with that email." error={error}>
        <SheetField
          value={email}
          onChangeText={setEmail}
          placeholder="name@company.com"
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="email-address"
          accessibilityLabel="Email"
          returnKeyType="done"
          onSubmitEditing={add}
          autoFocus
        />
        <SheetSwitch label="Lead of the team" value={lead} onChange={setLead} />
        <SheetSwitch label="Make this their default team" value={home} onChange={setHome} />
      </SheetForm>
    </Sheet>
  );
}
