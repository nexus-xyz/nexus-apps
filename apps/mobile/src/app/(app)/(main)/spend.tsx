import { BILLING_TERMS, usdText, type ModuleManifest, type PersonBudget, type SpendEntry, type SpendResponse, type UsageReport } from "@nexus/kit";
import { Stack, useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import { count, spentText } from "@/analytics";
import { useCore, useStored } from "@/auth";
import { Empty, Footnote, Hero, LargeTitle, Loading, Page, Row, Section, Share } from "@/list";
import { ProviderMark } from "@/provider-mark";
import { Meter } from "@/meter";
import { budgetCard, costOf, creditCard, leftForYou, limitCard, productRows, spendSource, spentLabel, thisMonth } from "@/spend";
import { useTheme } from "@/theme";

/**
 * Usage, as brand v5's P71f·E draws it on the phone: the title in the list, what the organization spent this period
 * with its budget's meter, then this month's teams, each a tap into Analytics (its child teams, or its people). Under
 * them, as Nexus UI's Spend page: your own limit this month, the Nexus credit when the organization has one, spend by
 * product and the recent calls. A member sees only what is left for them. The board's forecast is left out: Gateway
 * reports none, and the phone does not guess one; so is each team's top model, which needs a report per team.
 * Managing other people's limits is Nexus UI's Spend limits page, still to come here.
 */
export default function Spend() {
  const core = useCore();
  const cache = useStored<SpendResponse>("cache", "spend");
  const [spend, setSpend] = useState<SpendResponse | null>(() => cache.load() ?? null);
  const [mine, setMine] = useState<PersonBudget | null>(null);
  const [manifests, setManifests] = useState<ModuleManifest[]>([]);
  const [teams, setTeams] = useState<UsageReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    // Each on its own: an API key with no person has no limit, and a Gateway with no modules has no manifests, so
    // those two failing is expected and leaves their section out. Only the spend itself reports an error.
    Promise.all([
      core.spend().then(
        (s) => {
          setSpend(s);
          cache.save(s);
        },
        (e: Error) => setError(e.message),
      ),
      core.personBudget().then(setMine, () => setMine(null)),
      core.modules().then((r) => setManifests(r.modules), () => undefined),
      // This month's top-level teams; none for a Gateway or a caller that has no team report.
      core.usage({ groupBy: "team", depth: "1", ...thisMonth() }).then(setTeams, () => setTeams(null)),
    ]).finally(() => setLoading(false));
  }, [core, cache]);
  useFocusEffect(load);

  return (
    <Page onRefresh={load} refreshing={loading && !!spend}>
      <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>Usage</LargeTitle>
      {/* Shown over what loaded before too, so a failed refresh does not pass for fresh figures. */}
      {error ? <Footnote tone="danger">{spend ? `Couldn't refresh: ${error}` : error}</Footnote> : null}
      {!spend && !error ? <Loading /> : null}
      {spend ? (
        <>
          <Budget spend={spend} />
          {teams ? <Teams report={teams} /> : null}
          {mine ? <Limit person={mine} /> : null}
          {spend.subsidy ? <Credit spend={spend} /> : null}
          <ByProduct spend={spend} manifests={manifests} />
          <Recent calls={spend.recent} manifests={manifests} />
        </>
      ) : null}
    </Page>
  );
}

function Budget({ spend }: { spend: SpendResponse }) {
  const theme = useTheme();
  // A member sees only what is left for them, never the organization's budget, spent or reserved.
  const mine = leftForYou(spend);
  const own = spend.scope === "own" ? (mine ? "The calls are yours only." : "The calls are yours only; the budget is the organization's.") : undefined;
  if (mine) {
    return (
      <Section footer={own}>
        <Hero label={mine.label} value={usdText(mine.left)} note={mine.note} />
        {mine.subsidized !== undefined ? <Row label={BILLING_TERMS.paidByFreeStart} detail="Paid for your calls, not from your budget" value={usdText(mine.subsidized)} /> : null}
      </Section>
    );
  }
  const card = budgetCard(spend);
  return (
    <Section footer={own}>
      <Hero label={spentLabel(spend.budgetPeriod)} value={card.spent} note={card.percent}>
        <Meter theme={theme} {...card.meter} label="Budget used" />
      </Hero>
      <Row label="Remaining" value={card.remaining} monoValue valueTone="foreground" />
      <Row label="Budget" value={card.budget} monoValue />
      <Row label="Reserved" detail="Held for calls in flight" value={card.reserved} monoValue />
    </Section>
  );
}

/** This month's top-level teams, the costliest first; a team with child teams opens them in Analytics, one without its people. */
function Teams({ report }: { report: UsageReport }) {
  return (
    <Section title="Teams" footer="This month, UTC. Each team includes the teams under it.">
      {report.groups.length === 0 ? <Empty>No team has spent anything this month yet.</Empty> : null}
      {report.groups.map((g) => {
        const open = g.slug && !g.deleted && !g.direct ? (g.hasChildren ? { under: g.slug } : { team: g.slug }) : undefined;
        return (
          <Row
            key={g.key}
            label={g.label}
            detail={[`${count(g.calls)} ${g.calls === 1 ? "call" : "calls"}`, g.deleted ? "deleted team" : undefined, g.direct ? "the team itself" : undefined].filter(Boolean).join(" · ")}
            value={spentText(g)}
            monoValue
            valueTone="foreground"
            href={open ? { pathname: "/analytics", params: open } : undefined}
            accessibilityHint={open ? (open.under ? "Opens its child teams in Analytics" : "Opens its people in Analytics") : undefined}
          />
        );
      })}
      <Row label="Analytics" detail="Any range, by team, person, model or product" href="/analytics" />
    </Section>
  );
}

function Limit({ person }: { person: PersonBudget }) {
  const theme = useTheme();
  const from = person.source === "override" ? "Set for you" : person.source === "default" ? "The organization's default per person" : undefined;
  const card = limitCard(person);
  if (!card) {
    return (
      <Section title="Your limit this month">
        <Empty>No limit of your own; only the organization's budget applies.</Empty>
      </Section>
    );
  }
  return (
    <Section title="Your limit this month">
      <Hero label="Remaining" value={card.remaining} note={`of your ${card.limit} limit`}>
        <Meter theme={theme} {...card.meter} label="Your limit used" />
      </Hero>
      <Row label="Limit" detail={from} value={card.limit} />
      <Row label="Spent" value={card.spent} />
      <Row label="Reserved" value={card.reserved} />
      {person.atLimit ? <Empty tone="danger">You have reached your limit: your next call will be refused until the 1st.</Empty> : null}
    </Section>
  );
}

function Credit({ spend }: { spend: SpendResponse }) {
  const theme = useTheme();
  const s = spend.subsidy;
  const card = creditCard(spend);
  if (!s || !card) return null;
  return (
    <Section title={BILLING_TERMS.freeStart} footer="From Nexus, not your budget.">
      <Hero label="Left" value={card.remaining} note={`of ${card.granted} from Nexus, to start with`}>
        <Meter theme={theme} {...card.left} label="Free start left" text={`${card.remaining} of ${card.granted} left`} />
      </Hero>
      <Row label="Used" detail={`${card.subsidized} of calls paid by the free start`} value={card.spent} />
      {card.reserved ? <Row label="Reserved" detail="Held for calls in flight" value={card.reserved} /> : null}
      <Row label="Pays model calls" detail={s.usableForModelCalls ? "When you have no key of your own" : "Your own key is needed here"} value={s.usableForModelCalls ? "Yes" : "No"} />
    </Section>
  );
}

function ByProduct({ spend, manifests }: { spend: SpendResponse; manifests: ModuleManifest[] }) {
  const rows = productRows(spend.byProduct);
  return (
    <Section title="By product and capability">
      {rows.length === 0 ? <Empty>No calls this month yet.</Empty> : null}
      {rows.map((row) => {
        const name = spendSource(row.product, row.capability, manifests);
        return (
          <Row
            key={`${row.product}/${row.capability ?? ""}`}
            label={name.product}
            detail={[name.capability, `${row.calls} ${row.calls === 1 ? "call" : "calls"}`].filter(Boolean).join(" · ")}
            value={row.cost}
            valueTone="foreground"
            accessibilityLabel={`${name.product}${name.capability ? `, ${name.capability}` : ""}: ${row.cost}, ${Math.round(row.share * 100)} percent, ${row.calls} calls`}
          >
            <Share share={row.share} />
          </Row>
        );
      })}
    </Section>
  );
}

const STATUS: Record<SpendEntry["status"], string> = { settled: "Settled", failed: "Failed", refused: "Refused", released: "Released" };

function Recent({ calls, manifests }: { calls: SpendEntry[]; manifests: ModuleManifest[] }) {
  return (
    <Section title="Recent calls">
      {calls.length === 0 ? <Empty>No calls yet.</Empty> : null}
      {calls.map((call) => {
        const name = spendSource(call.product, call.capability, manifests);
        const status = call.overspendPrevented ? `${STATUS[call.status]} · ${call.overspendPrevented.level === "person" ? "person limit" : "organization budget"}` : STATUS[call.status];
        return (
          <Row
            key={call.id}
            leading={<ProviderMark model={call.model} size={16} />}
            label={call.model}
            // Red only for what went wrong, as Nexus UI's status pill: a released hold is not an error.
            detailTone={call.status === "failed" || call.status === "refused" ? "danger" : undefined}
            detail={[name.product, status, new Date(call.at).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }), call.fundedBy === "nexus_subsidy" ? BILLING_TERMS.paidByFreeStart : undefined]
              .filter(Boolean)
              .join(" · ")}
            value={usdText(costOf(call))}
            valueTone="foreground"
          />
        );
      })}
    </Section>
  );
}
