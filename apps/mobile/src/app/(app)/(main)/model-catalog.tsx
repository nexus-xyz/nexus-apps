import type { CurrentTenantResponse, ModelEntry, PolicyTestResponse } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { useCore } from "@/auth";
import { Busy, Footnote, Loading, Page, Row, Section } from "@/list";
import { usd } from "@/meta";
import { ruleRef } from "@/model-policy";
import { AdminHead } from "@/people-controls";
import { ProviderMark } from "@/provider-mark";
import { font, mono, text, useTheme } from "@/theme";
import { CustomizeTabs, useCustomize } from "@/customize-tabs";

/**
 * The catalogue, drilled into from Models (P95·F's Catalogue, on P105·D): every model Nexus names, supported (Gateway
 * calls the provider, priced and metered) or unavailable. Tap a supported model to test your organization's policy.
 */
export default function ModelCatalog() {
  const customize = useCustomize();
  const core = useCore();
  const [models, setModels] = useState<ModelEntry[] | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [results, setResults] = useState<Record<string, PolicyTestResponse | string>>({});

  const load = useCallback(() => {
    core.models({ status: "all" }).then((r) => setModels(r.data), (e: Error) => setError(e.message));
    core.me().then(setMe, () => undefined);
  }, [core]);
  useEffect(load, [load]);

  const test = (model: string) => {
    if (testing === model) return;
    if (results[model] !== undefined) return setResults(({ [model]: _, ...rest }) => rest);
    setTesting(model);
    core
      .testPolicy({ model })
      .then((r) => setResults((all) => ({ ...all, [model]: r })), (e: Error) => setResults((all) => ({ ...all, [model]: e.message })))
      .finally(() => setTesting(null));
  };

  const head = customize ? <CustomizeTabs active="/model-catalog" /> : <AdminHead title="Catalogue" purpose="Every model Nexus names, in USD per million tokens. Tap one to test it against your policies." />;
  if (!models) {
    return (
      <Page>
        {head}
        {error ? <Footnote tone="danger">The catalog could not be loaded. {error}</Footnote> : <Loading />}
      </Page>
    );
  }
  const supported = models.filter((m) => m.status === "supported");
  const soon = models.filter((m) => m.status === "coming_soon");

  return (
    <Page onRefresh={load}>
      {head}
      <Section>
        <Row label="Supported" detail="Called on your organization's provider keys" value={String(supported.length)} />
        <Row label="Allowed for you" detail={me?.team ? `In your default team, ${me.team.slug}` : "Under your organization's policies"} value={String(supported.filter((m) => m.allowed).length)} />
        <Row label="Unavailable" detail="Model families Nexus cannot run" value={String(soon.length)} />
      </Section>

      <Section footer="Auto runs your organization's default model. Owners and admins set the allowed models in Nexus for desktop or on the web.">
        <Model id="nexus/auto" name="Auto" note="Your organization's default model" onPress={() => test("nexus/auto")} result={results["nexus/auto"]} testing={testing === "nexus/auto"} />
        {[...supported, ...soon].map((m) => (
          <Model
            key={m.id}
            id={m.id}
            name={m.name}
            note={m.status === "coming_soon" ? undefined : m.pricing ? `${usd(m.pricing.input)} in · ${usd(m.pricing.output)} out` : undefined}
            status={m.status === "coming_soon" ? "Unavailable" : m.allowed ? "Allowed" : "Blocked"}
            blocked={m.status === "supported" && !m.allowed}
            onPress={m.status === "supported" ? () => test(m.id) : undefined}
            result={results[m.id]}
            testing={testing === m.id}
          />
        ))}
      </Section>
    </Page>
  );
}

function Model({
  id,
  name,
  note,
  status,
  blocked,
  onPress,
  result,
  testing,
}: {
  id: string;
  name: string;
  note?: string;
  status?: string;
  blocked?: boolean;
  onPress?: () => void;
  result?: PolicyTestResponse | string;
  testing: boolean;
}) {
  const theme = useTheme();
  return (
    <Row
      leading={<ProviderMark model={id} size={18} />}
      label={name}
      tone={onPress || id === "nexus/auto" ? undefined : "muted"}
      detail={
        <>
          <Text style={styles.mono}>{id}</Text>
          {note ? ` · ${note}` : ""}
        </>
      }
      value={status}
      valueTone={blocked ? "danger" : undefined}
      accessory={testing ? <Busy /> : undefined}
      onPress={onPress}
      accessibilityHint={onPress ? "Tests it against your policies" : undefined}
      accessibilityState={onPress ? { expanded: result !== undefined, busy: testing } : undefined}
    >
      {typeof result === "string" ? <Text style={[styles.body, { color: theme.danger }]}>{result}</Text> : null}
      {result && typeof result !== "string" ? (
        <View style={[styles.result, { backgroundColor: result.allowed ? theme.primaryFaint : theme.dangerFaint }]}>
          <Text style={[styles.strong, { color: result.allowed ? theme.primary : theme.danger }]}>
            {result.allowed ? "Allowed" : "Refused"} <Text style={styles.mono}>{result.model ?? result.requested}</Text>
            {result.resolvedFrom ? <Text style={[styles.meta, { color: theme.muted }]}> ({result.resolvedFrom})</Text> : null}
          </Text>
          <Text style={[styles.body, { color: theme.foreground }]}>{result.reason}</Text>
          <Text style={[styles.meta, { color: theme.muted }]}>Rule: {ruleRef(result.rule)}</Text>
          {result.warnings.map((w) => (
            <Text key={w} style={[styles.meta, { color: theme.muted }]}>
              {w}
            </Text>
          ))}
        </View>
      ) : null}
    </Row>
  );
}

const styles = StyleSheet.create({
  strong: { ...text.sm, fontFamily: font.medium },
  body: { ...text.sm, fontFamily: font.regular },
  meta: { ...text.xs, fontFamily: font.regular },
  mono: { fontFamily: mono, fontSize: 12, lineHeight: 17 },
  result: { borderRadius: 12, borderCurve: "continuous", padding: 10, gap: 4 },
});
