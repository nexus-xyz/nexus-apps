import { amount, amountOrNull, usdText, type CurrentTenantResponse, type ModelPolicyView, type TeamBudget, type TeamDetail, type UsageReport } from "@nexus/kit";
import { useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { spentText } from "@/analytics";
import { useCore } from "@/auth";
import { CreateTeam } from "@/create-team";
import { Blank, Loading, Page, Row, Section } from "@/list";
import { AdminHead, Fact, FootAction } from "@/people-controls";
import { insideLine, people, perPeriod, thisPeriod } from "@/people-controls-words";
import { hasRules, policyItems } from "@/policy-words";
import { useToast } from "@/toast";

/**
 * A team (brand v5's P71c·T5, one item on P105·D): where it sits, its leads, its people, its budget and what it spent,
 * then its people and the rules set on it, each drilling in, and the teams under it. Its one action is Make my default.
 * Owners and admins add a child team in a sheet; people are added and removed under People (`team-people/[slug].tsx`).
 * Renaming, moving and deleting a team stay on desktop and the web.
 */
export default function Team() {
  const core = useCore();
  const toast = useToast();
  const { slug } = useLocalSearchParams<{ slug: string }>();
  const [team, setTeam] = useState<TeamDetail | null>(null);
  const [usage, setUsage] = useState<UsageReport | null>(null);
  const [budget, setBudget] = useState<TeamBudget | null>(null);
  const [policy, setPolicy] = useState<ModelPolicyView | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const load = useCallback(() => {
    core.team(slug).then(
      (t) => {
        setTeam(t);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    // Each of these is refused to someone outside the team or below admin: optional, and left out then.
    core.usage({ team: slug, groupBy: "person" }).then(setUsage, () => setUsage(null));
    core.teamBudget(slug).then(setBudget, () => setBudget(null));
    core.policy("team", slug).then(setPolicy, () => setPolicy(null));
  }, [core, slug]);
  useEffect(load, [load]);
  useEffect(() => {
    core.me().then(setMe, () => undefined);
  }, [core]);

  if (!team) return error ? <Blank title="Couldn't load this team" body={error} tone="danger" /> : <Loading alone />;
  const manage = !!me?.permissions?.includes("teams.manage");
  const leads = team.memberList.filter((m) => m.role === "lead").map((m) => m.name || m.email || m.userId);
  const parent = team.path[team.path.length - 1];
  const cap = budget?.scope === "cap" ? budget.budget : null;
  const limit = cap ? amountOrNull(cap.limitUsdDecimal, cap.limitUsd) : null;
  const spent = cap && (cap.spentUsdDecimal !== undefined || cap.spentUsd !== undefined) ? `${usdText(amount(cap.spentUsdDecimal, cap.spentUsd))} ${thisPeriod(cap.period)}` : usage ? `${spentText(usage.totals)} in 30 days` : null;
  const own = policy && hasRules(policy.rule) ? policyItems(policy.rule, (id) => id) : null;
  const rules = own ? own.models.length + own.where.length : 0;
  const makeDefault = () =>
    core.setDefaultTeam(team.slug).then(
      () => {
        toast({ text: `${team.name} is now your default team` });
        load();
      },
      (e: Error) => toast({ text: e.message }),
    );

  return (
    <Page onRefresh={load}>
      <AdminHead title={team.name} purpose={[team.level, team.description].filter(Boolean).join(" · ") || undefined} />
      <Section>
        <Fact
          label="Inside"
          value={insideLine(me?.tenant.name ?? "", team.path)}
          href={parent ? { pathname: "/team/[slug]", params: { slug: parent.slug } } : undefined}
        />
        <Fact label={leads.length > 1 ? "Leads" : "Lead"} value={leads.join(", ") || "No lead yet"} />
        <Fact label="People" value={`${team.members}${team.pending ? `, and ${team.pending} not joined yet` : ""}`} />
        <Fact label="You" value={[team.myRole ? (team.myRole === "lead" ? "Lead" : "Member") : "Not in it", team.isDefault ? "your default team" : undefined].filter(Boolean).join(" · ")} />
        {limit !== null && cap ? (
          <Fact label="Budget" value={`${usdText(limit)} ${perPeriod(cap.period)}`} href={{ pathname: "/team-budget/[slug]", params: { slug: team.slug } }} />
        ) : budget?.canManage ? (
          <Fact label="Budget" value="None of its own" href={{ pathname: "/team-budget/[slug]", params: { slug: team.slug } }} />
        ) : null}
        {spent ? <Fact label="Spent" value={spent} /> : null}
      </Section>

      <Section>
        <Row label="People" value={String(team.members + team.pending)} href={{ pathname: "/team-people/[slug]", params: { slug: team.slug } }} />
        {policy ? <Row label="Rules set here" value={String(rules)} href={{ pathname: "/policy", params: { team: team.slug } }} /> : null}
      </Section>

      {team.children.length || manage ? (
        <Section title="Teams under it">
          {team.children.map((child) => (
            <Row
              key={child.id}
              label={child.name}
              detail={[people(child.members + child.pending), child.childCount ? `${child.childCount} under it` : undefined].filter(Boolean).join(" · ")}
              href={{ pathname: "/team/[slug]", params: { slug: child.slug } }}
            />
          ))}
          {manage ? <Row label="Add a team under it" tone="primary" onPress={() => setCreating(true)} /> : null}
        </Section>
      ) : null}

      {team.myRole && !team.isDefault ? <FootAction label="Make my default" onPress={makeDefault} /> : null}
      <CreateTeam visible={creating} onClose={() => setCreating(false)} parent={{ slug: team.slug, name: team.name }} onCreated={load} />
    </Page>
  );
}
