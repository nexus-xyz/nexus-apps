import { amount, amountOrNull, usdText, type CurrentTenantResponse, type MemberSummary, type PersonBudget } from "@nexus/kit";
import { useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { useCore } from "@/auth";
import { Blank, Footnote, Loading, Page, Section } from "@/list";
import { usd } from "@/meta";
import { AdminHead, Fact } from "@/people-controls";
import { ROLE, shortDay } from "@/people-controls-words";

/**
 * A person (brand v5's P71e·F, one item on P105·D): their role, email, when they joined, this month's spend against
 * their limit and what they have spent through Nexus. Left out: the design's home team and Signed in, which the
 * Gateway's member list does not carry, and Recent activity, since Audit on the phone does not open on one person.
 * Its one action, Remove, changes the auth provider and Gateway together from Nexus UI's server, so it stays on desktop
 * and the web, with a role change and an ownership transfer.
 */
export default function Member() {
  const core = useCore();
  const { id } = useLocalSearchParams<{ id: string }>();
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [member, setMember] = useState<MemberSummary | null | undefined>();
  const [budget, setBudget] = useState<PersonBudget | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    core.me().then(setMe, () => undefined);
    core.members().then(
      (r) => {
        setMember(r.members.find((m) => m.userId === id) ?? null);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    // Owners and admins see anyone's limit, a member their own: anyone else's is refused, and the row left out.
    core.personBudget(id).then(setBudget, () => setBudget(null));
  }, [core, id]);
  useEffect(load, [load]);

  if (member === undefined) return error ? <Blank title="Couldn't load this person" body={error} tone="danger" /> : <Loading alone />;
  if (member === null) return <Blank title="Not a member" body="This person is not in your organization any more." />;

  const name = member.name || member.email || member.userId;
  const spent = budget ? amount(budget.spentUsdDecimal, budget.spentUsd) : null;
  const limit = budget ? amountOrNull(budget.limitUsdDecimal, budget.limitUsd) : null;
  const manage = !!me?.permissions?.includes("members.manage");
  return (
    <Page onRefresh={load}>
      <AdminHead title={name} purpose={member.userId === me?.user?.id ? "You, in this organization." : undefined} />
      <Section>
        <Fact label="Role" value={ROLE[member.role]} />
        {member.name && member.email ? <Fact label="Email" value={member.email} /> : null}
        <Fact label="Joined" value={shortDay(member.joinedAt)} />
      </Section>
      <Section>
        {spent !== null ? (
          <Fact
            label="This month"
            value={limit === null ? `${usdText(spent)} · no limit` : `${usdText(spent)} of ${usdText(limit)}${budget?.atLimit ? " · at the limit" : ""}`}
            valueTone={budget?.atLimit ? "danger" : undefined}
          />
        ) : null}
        {member.spentUsd !== undefined ? <Fact label="Spent through Nexus" value={`${usd(member.spentUsd)}${member.calls !== undefined ? ` · ${member.calls} calls` : ""}`} /> : null}
      </Section>
      {manage ? <Footnote>Change their role, remove them or transfer ownership in Nexus for desktop or on the web.</Footnote> : null}
    </Page>
  );
}
