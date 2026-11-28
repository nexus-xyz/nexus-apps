import type { CurrentTenantResponse, ModelPolicyView, OrgSettings } from "@nexus/kit";
import { useFocusEffect, useLocalSearchParams, type Href } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { useCore, useStored } from "@/auth";
import { Footnote, Loading, Page, Row, Section } from "@/list";
import { nameOf, useModels } from "@/models";
import { AdminHead } from "@/people-controls";
import { hasRules, policyItems, type PolicyItem } from "@/policy-words";

/**
 * Policy (brand v5's P71a·H on the P105·D system): the organization's rules as sentences in sections, Models, then
 * Providers, then Tools and agents, each opening the rule (`policy-rule.tsx`). With nothing of its own, In effect now
 * says what applies instead. The Gateway's own rules above it follow, when it has any. Opened from a team (`?team=`),
 * the rules set on that team. Left out: the design's Requests, since the Gateway has no list of pending approvals to
 * read, and the rule's Change, Why it applies and History: rules change on a computer.
 */
export default function Policy() {
  const core = useCore();
  const models = useModels(core);
  const { team } = useLocalSearchParams<{ team?: string }>();
  const cache = useStored<ModelPolicyView>("cache", team ? `policy:${team}` : "policy");
  const [view, setView] = useState<ModelPolicyView | null>(() => cache.load() ?? null);
  const [settings, setSettings] = useState<OrgSettings | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  useFocusEffect(
    useCallback(() => {
      (team ? core.policy("team", team) : core.policy("organization")).then(
        (v) => {
          setView(v);
          cache.save(v);
          setError(null);
        },
        (e: Error) => setError(e.message),
      );
    }, [core, cache, team]),
  );
  useEffect(() => {
    core.me().then(setMe, () => undefined);
    if (!team) core.orgSettings().then(setSettings, () => undefined);
  }, [core, team]);

  if (!view && !error) return <Loading alone />;
  const name = (id: string) => nameOf(models, id);
  const org = me?.tenant.name ?? "your organization";
  const scope = view?.team?.name ?? (team ? team : org);
  const own = view && hasRules(view.rule) ? policyItems(view.rule, name) : null;
  const above = view?.inherited.filter((i) => i.level === "deployment") ?? [];
  const at = view?.updatedAt;
  const open = (item: PolicyItem, from: "here" | "gateway"): Href => ({
    pathname: "/policy-rule",
    params: { title: item.title, text: item.text, scope: from === "gateway" ? "Everyone on this Gateway" : scope, from, ...(from === "here" && at ? { at } : {}) },
  });
  // People's own Claude and ChatGPT plans, outside Gateway's checks: the organization's setting, read here.
  const tools = settings?.agentSubscriptions ? [{ title: "Subscription mode", text: "People may run Claude Code and Codex on their own plans" }] : [];

  return (
    <Page>
      <AdminHead title="Policy" purpose={team ? `The rules set on ${scope}, on top of ${org}'s.` : `What people in ${org} may use, and how.`} />
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      {view && !own ? (
        <Section title="In effect now">
          {team ? <Row label={`${org}'s rules apply`} detail="Nothing is set on this team of its own" href="/policy" /> : null}
          {team ? null : <Row label={above.length ? "Every model your Gateway allows" : "Every model Nexus offers is allowed"} />}
          {settings && !team ? <Row label={`Chats are kept ${settings.chatRetentionDays} days`} /> : null}
          {tools.map((t) => (
            <Row key={t.text} label={t.text} />
          ))}
        </Section>
      ) : null}
      {own ? (
        <>
          <Section title="Models">
            {own.models.map((item) => (
              <Row key={item.text} label={item.text} detail={team ? scope : undefined} href={open(item, "here")} />
            ))}
          </Section>
          {own.where.length ? (
            <Section title="Providers">
              {own.where.map((item) => (
                <Row key={item.text} label={item.text} detail={team ? scope : undefined} href={open(item, "here")} />
              ))}
            </Section>
          ) : null}
          {tools.length ? (
            <Section title="Tools and agents">
              {tools.map((t) => (
                <Row key={t.text} label={t.text} />
              ))}
            </Section>
          ) : null}
        </>
      ) : null}
      {above.map((i, n) => {
        const items = policyItems(i.rule, name);
        return (
          <Section key={n} title="Set by your Gateway">
            {[...items.models, ...items.where].map((item) => (
              <Row key={item.text} label={item.text} href={open(item, "gateway")} />
            ))}
          </Section>
        );
      })}
      <Footnote>{[...(view?.notes ?? []), "Change rules on a computer."].join(" ")}</Footnote>
    </Page>
  );
}
