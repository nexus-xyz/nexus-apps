import { amount, amountOrNull, usdText, type TeamBudget } from "@nexus/kit";
import { useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { useCore } from "@/auth";
import { Blank, Footnote, Loading, Page, Section, Segmented } from "@/list";
import { AdminHead, Fact, FootAction, SheetField, SheetForm } from "@/people-controls";
import { perPeriod, shortDay, thisPeriod, typedAmount } from "@/people-controls-words";
import { Sheet } from "@/sheet";
import { useToast } from "@/toast";

type Period = "day" | "week" | "month";
const PERIODS: { id: Period; label: string }[] = [
  { id: "day", label: "Day" },
  { id: "week", label: "Week" },
  { id: "month", label: "Month" },
];

/**
 * A team's budget (brand v5's P71b·F, one item on P105·D): the cap and its period, what the team spent of it and what
 * is left until it resets, and what happens at 100 %: new calls stop, the only mode the Gateway has. Its one action,
 * Change budget, is a sheet for owners and admins (`?edit=1` opens it, from Budgets' +). Left out: the design's At 80 %
 * warning, which the Gateway does not send, and the forecast of when the team reaches its cap.
 */
export default function TeamBudgetPage() {
  const core = useCore();
  const toast = useToast();
  const { slug, edit } = useLocalSearchParams<{ slug: string; edit?: string }>();
  const [budget, setBudget] = useState<TeamBudget | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [typed, setTyped] = useState("");
  const [period, setPeriod] = useState<Period>("month");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(() => {
    core.teamBudget(slug).then(
      (b) => {
        setBudget(b);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
  }, [core, slug]);
  useEffect(load, [load]);

  const cap = budget?.scope === "cap" ? budget.budget : null;
  const limit = cap ? amountOrNull(cap.limitUsdDecimal, cap.limitUsd) : null;
  const open = useCallback(() => {
    setTyped(cap?.limitUsdDecimal ?? (cap?.limitUsd !== undefined ? String(cap.limitUsd) : ""));
    setPeriod(cap?.period ?? "month");
    setProblem(null);
    setEditing(true);
  }, [cap]);
  // From Budgets' +: after the push, so this sheet does not open while Budgets' picker is still closing.
  const [asked, setAsked] = useState(edit === "1");
  useEffect(() => {
    if (!asked || !budget?.canManage) return;
    const timer = setTimeout(() => {
      setAsked(false);
      open();
    }, 500);
    return () => clearTimeout(timer);
  }, [asked, budget, open]);

  const save = () => {
    const value = typedAmount(typed);
    if (!value) return setProblem("Enter an amount above $0, such as 15000.");
    setBusy(true);
    setProblem(null);
    core
      .setTeamBudget(slug, { amountUsdDecimal: value, period, mode: "hard_stop" })
      .then((change) => {
        setEditing(false);
        toast({ text: `${change.team.name}: ${usdText(amount(change.after.limitUsdDecimal, change.after.limitUsd))} ${perPeriod(change.after.period)}` });
        load();
      })
      .catch((e: Error) => setProblem(e.message))
      .finally(() => setBusy(false));
  };

  if (!budget) return error ? <Blank title="Couldn't load this budget" body={error} tone="danger" /> : <Loading alone />;
  const bound = budget.scope === "own" ? budget.budget : null;
  return (
    <Page onRefresh={load}>
      <AdminHead title={budget.team.name} purpose="Its budget, what it spent, and what happens when it runs out." />
      {cap && limit !== null ? (
        <Section>
          <Fact label="Budget" value={`${usdText(limit)} ${perPeriod(cap.period)}`} />
          {cap.spentUsdDecimal !== undefined || cap.spentUsd !== undefined ? (
            <Fact label="Spent" value={`${usdText(amount(cap.spentUsdDecimal, cap.spentUsd))} ${thisPeriod(cap.period)} · resets ${shortDay(cap.resetsAt)}`} />
          ) : null}
          <Fact label="Left" value={usdText(amount(cap.remainingUsdDecimal, cap.remainingUsd))} valueTone={amount(cap.remainingUsdDecimal, cap.remainingUsd) <= 0 ? "danger" : undefined} />
          <Fact label="At 100 %" value="Stop new calls" />
        </Section>
      ) : bound ? (
        <Section title="In effect now">
          <Fact label="Left for this team" value={`${usdText(amount(bound.remainingUsdDecimal, bound.remainingUsd))} until ${shortDay(bound.resetsAt)}`} />
        </Section>
      ) : (
        <Section title="In effect now">
          <Fact label="Budget" value="None of its own" />
        </Section>
      )}
      {budget.canManage ? <FootAction label={limit !== null ? "Change budget" : "Set a budget"} onPress={open} /> : <Footnote>Only owners and admins change a team's budget.</Footnote>}

      <Sheet visible={editing} onClose={() => setEditing(false)}>
        <SheetForm title={`${budget.team.name}'s budget`} action="Save" onSubmit={save} disabled={!typed.trim()} busy={busy} note="At 100 % its new calls stop until the period resets." error={problem}>
          <SheetField
            monoText
            value={typed}
            onChangeText={setTyped}
            placeholder="Amount in USD, e.g. 15000"
            keyboardType="decimal-pad"
            accessibilityLabel="Budget in US dollars"
            autoFocus
          />
          <Segmented label="Every" options={PERIODS} value={period} onChange={setPeriod} />
        </SheetForm>
      </Sheet>
    </Page>
  );
}
