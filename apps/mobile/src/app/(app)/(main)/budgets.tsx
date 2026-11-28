import { amount, amountOrNull, share, usdText, type BudgetsResponse, type CurrentTenantResponse, type SpendResponse, type TeamBudget } from "@nexus/kit";
import { router, useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import { useAuth, useCore, useStored } from "@/auth";
import { Footnote, Hero, Loading, Page, Row, Section, Share } from "@/list";
import { Meter } from "@/meter";
import { AdminHead } from "@/people-controls";
import { teamBudgetLine } from "@/people-controls-words";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { budgetCard, leftForYou, spendAmounts } from "@/spend";
import { flattenTeams } from "@/teams";
import { useTheme } from "@/theme";

/** "2026-11" as "November". */
const month = (period: string) => {
  const at = new Date(`${period}-01T12:00:00Z`);
  return Number.isNaN(at.getTime()) ? period : at.toLocaleDateString([], { month: "long" });
};

/**
 * Budgets (brand v5's P71b·F on the P105·D system): the organization's limit on top, with what it spent of it, for
 * owners and admins; then the teams with a budget, each opening it (`team-budget/[slug].tsx`, where it changes); then
 * this month's limit for each person, the default first, the most spent first. + sets a budget on a team that has none.
 * A member sees their own limit. Left out: the design's "reaches $15,000 on 26 Nov", a forecast the Gateway does not
 * make. People's limits change on a computer.
 */
export default function Budgets() {
  const core = useCore();
  const theme = useTheme();
  // As they were last time, until the Gateway answers.
  const cache = useStored<BudgetsResponse>("cache", "budgets");
  const spendCache = useStored<SpendResponse>("cache", "spend");
  const [budgets, setBudgets] = useState<BudgetsResponse | null>(() => cache.load() ?? null);
  const [spend, setSpend] = useState<SpendResponse | null>(() => spendCache.load() ?? null);
  const [teams, setTeams] = useState<TeamBudget[] | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  useFocusEffect(
    useCallback(() => {
      core.budgets().then(
        (b) => {
          setBudgets(b);
          cache.save(b);
          setError(null);
        },
        (e: Error) => setError(e.message),
      );
      core.spend().then(
        (s) => {
          setSpend(s);
          spendCache.save(s);
        },
        () => undefined,
      );
      core.me().then(setMe, () => undefined);
      // Each team's cap, as far as the caller may see it; a team they may not see is left out.
      core.teamTree().then(
        (tree) =>
          Promise.all(flattenTeams(tree.tree).map((t) => core.teamBudget(t.slug).catch(() => null))).then((all) =>
            setTeams(all.filter((b): b is TeamBudget => b !== null)),
          ),
        () => setTeams([]),
      );
    }, [core, cache, spendCache]),
  );
  if (!budgets && !error) return <Loading alone />;

  const { orgName } = useAuth();
  // The name the sign-in knows until Gateway answers; a lower-case stand-in, since it sits mid-sentence.
  const org = me?.tenant.name ?? orgName ?? "your organization";
  const card = spend && !leftForYou(spend) ? budgetCard(spend) : undefined;
  const capped = (teams ?? []).filter((t) => t.scope === "cap" && teamBudgetLine(t.budget));
  const uncapped = (teams ?? []).filter((t) => t.canManage && t.scope === "cap" && !teamBudgetLine(t.budget));
  const fallback = budgets ? amountOrNull(budgets.personDefault.monthlyUsdDecimal, budgets.personDefault.monthlyUsd) : null;
  const people = [...(budgets?.people ?? [])].sort((a, b) => amount(b.spentUsdDecimal, b.spentUsd) - amount(a.spentUsdDecimal, a.spentUsd));

  return (
    <Page>
      <AdminHead
        title="Budgets"
        purpose={`How much ${org} may spend, by team and by person.`}
        add={uncapped.length ? { label: "Set a team's budget", onPress: () => setPicking(true) } : undefined}
      />
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      {card && spend && spendAmounts(spend).budget > 0 ? (
        <Section>
          <Hero label={`${org.charAt(0).toUpperCase()}${org.slice(1)}'s usage limit${budgets ? ` · ${month(budgets.period)}` : ""}`} value={card.spent} note={`of ${card.budget}`}>
            <Meter theme={theme} {...card.meter} label="Budget used" />
          </Hero>
        </Section>
      ) : null}

      {capped.length ? (
        <Section title="Teams">
          {capped.map((t) => {
            const line = teamBudgetLine(t.budget)!;
            return (
              <Row
                key={t.team.slug}
                label={t.team.name}
                detail={line.line}
                detailTone={line.atLimit ? "danger" : undefined}
                value={line.cap}
                monoValue
                href={{ pathname: "/team-budget/[slug]", params: { slug: t.team.slug } }}
              />
            );
          })}
        </Section>
      ) : null}

      {budgets ? (
        <Section title={capped.length || card ? `People · ${month(budgets.period)}` : "In effect now"} footer="Change people's limits on a computer.">
          <Row label="Each person, by default" value={fallback === null ? "No limit" : `${usdText(fallback)} a month`} />
          {teams && !capped.length ? <Row label="Teams" value="No team budgets" /> : null}
          {people.map((p) => {
            const spent = amount(p.spentUsdDecimal, p.spentUsd);
            const limit = amountOrNull(p.limitUsdDecimal, p.limitUsd);
            return (
              <Row
                key={p.userId}
                label={p.name || p.email || "Someone"}
                detail={p.atLimit ? "At the limit" : p.source === "override" ? "Their own limit" : undefined}
                detailTone={p.atLimit ? "danger" : undefined}
                value={limit === null ? `${usdText(spent)} · no limit` : `${usdText(spent)} of ${usdText(limit)}`}
              >
                {limit ? <Share share={share(spent, limit)} /> : null}
              </Row>
            );
          })}
        </Section>
      ) : null}

      <Sheet visible={picking} onClose={() => setPicking(false)}>
        <SheetTitle>Set a budget for</SheetTitle>
        {uncapped.map((t) => (
          <SheetRow
            key={t.team.slug}
            label={t.team.name}
            onPress={() => {
              setPicking(false);
              router.push({ pathname: "/team-budget/[slug]", params: { slug: t.team.slug, edit: "1" } });
            }}
          />
        ))}
        <SheetRow label="Close" tone="muted" onPress={() => setPicking(false)} />
      </Sheet>
    </Page>
  );
}
