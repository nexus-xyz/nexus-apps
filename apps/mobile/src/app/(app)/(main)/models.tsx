import type { CurrentTenantResponse, ModelEntry, ModelPolicyView, ProviderKeySummary } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { useCore } from "@/auth";
import { Footnote, Loading, Page, Row, Section } from "@/list";
import { AUTO } from "@/meta";
import { AdminHead, Notice } from "@/people-controls";
import { provider } from "@/providers";

/**
 * Models (brand v5's P95·F on the P105·D system): which model each place starts on, then the catalogue, which drills in
 * (`model-catalog.tsx`, where a tap tests a model against your policies). A provider with no key is a notice on top
 * that opens Providers. Left out until the Gateway has them: the defaults for Slack and the Assistant, what Auto
 * prefers and how plans run in it, aliases and fallbacks. The defaults change with the policy, on a computer.
 */
export default function Models() {
  const core = useCore();
  const [models, setModels] = useState<ModelEntry[] | null>(null);
  const [policy, setPolicy] = useState<ModelPolicyView | null>(null);
  const [keys, setKeys] = useState<ProviderKeySummary[] | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    core.models({ status: "all" }).then(
      (r) => {
        setModels(r.data);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    core.policy("organization").then(setPolicy, () => undefined);
    core.listProviderKeys().then((r) => setKeys(r.providers), () => undefined);
    core.me().then(setMe, () => undefined);
  }, [core]);
  useEffect(load, [load]);

  const supported = (models ?? []).filter((m) => m.status === "supported");
  const keyed = new Set((keys ?? []).filter((k) => !k.unused).map((k) => k.provider));
  const missing = keys ? [...new Set(supported.map((m) => m.owned_by))].filter((p) => !keyed.has(p)) : [];
  const canAddKeys = !!me?.permissions?.includes("provider_keys.manage");
  const name = (id?: string) => (!id ? "—" : id === AUTO ? "Auto" : (models?.find((m) => m.id === id)?.name ?? id));
  const defaults = policy?.effective.defaults;

  return (
    <Page onRefresh={load}>
      <AdminHead title="Models" purpose={`Which model each place starts on, and the models ${me?.tenant.name ?? "your organization"} can call.`} />
      {missing.length ? (
        <Notice
          label={`No ${missing.map((p) => provider(p).name).join(" or ")} key yet`}
          detail={`Those models are refused until ${canAddKeys ? "you add one" : "an owner or admin adds one"}.`}
          action={canAddKeys ? "Add" : undefined}
          href="/providers"
        />
      ) : null}

      <Section title="Defaults" footer="Hard limits are in Policy.">
        {defaults ? <Row label="New chats" value={name(defaults.chat.model)} /> : <Loading bare rows={3} />}
        {defaults ? <Row label="Claude Code" value={name(defaults.claude.model)} /> : null}
        {defaults ? <Row label="Codex" value={name(defaults.codex.model)} /> : null}
      </Section>

      {error && !models ? <Footnote tone="danger">The catalog could not be loaded. {error}</Footnote> : null}
      <Section>
        <Row label="Catalogue" value={models ? String(models.length) : undefined} href="/model-catalog" />
        <Row label="Policy" href="/policy" />
      </Section>
    </Page>
  );
}
