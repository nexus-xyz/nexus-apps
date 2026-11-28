import { usdText, type CurrentTenantResponse, type ModuleManifest, type OrgSettings, type SpendResponse } from "@nexus/kit";
import { useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { useCore, useStored } from "@/auth";
import { Switch, TextInput } from "@/controls";
import { Empty, Footnote, Hero, Loading, Page, Row, Section } from "@/list";
import { Meter } from "@/meter";
import { productLine, sharingRows } from "@/overview";
import { AdminHead } from "@/people-controls";
import { productName } from "@/products";
import { budgetCard, leftForYou } from "@/spend";
import { font, mono, text, useTheme } from "@/theme";

type Product = { slug: string; name: string; capabilities: ModuleManifest[] };

/**
 * Nexus Gateway, as desktop's overview shows it, on the P105·D system (its title large under Back, its purpose under
 * it): the organization and its budget; its products, each opening its screen; subscription mode and chat retention,
 * which owners and admins change; and sharing outside, read here and changed on desktop or the web. Its provider keys
 * are Providers (P26·D); the coding agents' routes are the Coding agents screen.
 */
export default function Gateway() {
  const core = useCore();
  const theme = useTheme();
  const cache = useStored<SpendResponse>("cache", "spend");
  const [spend, setSpend] = useState<SpendResponse | null>(() => cache.load() ?? null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [products, setProducts] = useState<Product[] | null>(null);
  const [unavailable, setUnavailable] = useState<{ url: string; error: string }[]>([]);
  const [settings, setSettings] = useState<OrgSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A failed budget or products load says so, rather than passing for current figures or an empty list.
  const [spendError, setSpendError] = useState<string | null>(null);
  const [productsError, setProductsError] = useState<string | null>(null);
  const [meError, setMeError] = useState<string | null>(null);

  const load = useCallback(() => {
    core.spend().then(
      (s) => {
        setSpend(s);
        setSpendError(null);
        cache.save(s);
      },
      (e: Error) => setSpendError(e.message),
    );
    core.me().then(
      (m) => {
        setMe(m);
        setMeError(null);
      },
      (e: Error) => setMeError(e.message),
    );
    core.orgSettings().then(
      (s) => {
        setSettings(s);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    core.modules().then(
      (r) => {
        const grouped = new Map<string, Product>();
        for (const m of r.modules) {
          const entry = grouped.get(m.product.slug) ?? { ...m.product, capabilities: [] };
          entry.capabilities.push(m);
          grouped.set(m.product.slug, entry);
        }
        setProducts([...grouped.values()]);
        setUnavailable(r.unavailable);
        setProductsError(null);
      },
      (e: Error) => {
        setProducts([]);
        setProductsError(e.message);
      },
    );
  }, [core, cache]);
  useFocusEffect(load);

  const canManage = !!me?.permissions?.includes("org.settings");
  // A member sees only what is left for them, never the organization's budget, spent or reserved.
  const mine = spend ? leftForYou(spend) : undefined;
  const card = spend && !mine ? budgetCard(spend) : undefined;
  const save = (change: Partial<OrgSettings>) =>
    core.setOrgSettings(change as Record<string, unknown>).then((next) => setSettings((s) => ({ ...s, ...next })));

  return (
    <Page onRefresh={load}>
      <AdminHead title="Gateway" purpose={productLine("core", [])} />

      {meError ? <Footnote tone="danger">The organization's details did not load. {meError}</Footnote> : null}
      {spendError ? <Footnote tone="danger">{spend ? `Couldn't refresh the budget (${spendError}). These are the figures from before.` : `The budget did not load. ${spendError}`}</Footnote> : null}
      {spend ? (
        <Section>
          {mine ? (
            <Hero label={mine.label} value={usdText(mine.left)} note={mine.note} />
          ) : card ? (
            <Hero label="Remaining" value={card.remaining} note={`of ${card.budget} budget · ${card.spent} spent`}>
              <Meter theme={theme} {...card.meter} label="Budget used" />
            </Hero>
          ) : null}
          <Row label="Organization" detail={me ? (me.tenant.slug ?? me.tenant.id) : undefined} value={me?.tenant.name ?? " "} valueTone="foreground" accessibilityLabel={`Organization: ${me?.tenant.name ?? ""}`} />
        </Section>
      ) : (
        <Loading />
      )}

      <Section title="Products" footer="Each runs on your organization's keys and budget.">
        {products === null ? <Loading bare rows={2} /> : null}
        {productsError ? <Empty tone="danger">The products did not load. {productsError}</Empty> : null}
        {products?.length === 0 && !productsError ? <Empty>No products are registered.</Empty> : null}
        {products?.map((product) => (
          <Row
            key={product.slug}
            label={productName(product)}
            detail={[productLine(product.slug, product.capabilities), product.capabilities.map((m) => m.capability.name).join(" · ")].filter(Boolean).join("\n")}
            href={{ pathname: "/product/[slug]", params: { slug: product.slug } }}
          />
        ))}
        {unavailable.map((m) => (
          <Empty key={m.url} tone="danger">
            The module at {m.url} is not answering ({m.error}).
          </Empty>
        ))}
      </Section>

      {error ? (
        <Footnote tone="danger">{settings ? `Couldn't refresh the settings (${error}). These are the settings from before.` : `The organization's settings did not load. ${error}`}</Footnote>
      ) : null}
      {settings ? (
        <>
          <Subscriptions on={settings.agentSubscriptions} canManage={canManage} onChange={(on) => save({ agentSubscriptions: on })} />
          <Retention settings={settings} canManage={canManage} save={save} />
          <Section title="Sharing outside" footer={canManage ? "Change these in Nexus for desktop or on the web." : "Only owners and admins can change these."}>
            {sharingRows(settings).map((row) => (
              <Row key={row.label} label={row.label} value={row.value} accessibilityLabel={`${row.label}: ${row.value}`} />
            ))}
          </Section>
        </>
      ) : null}
    </Page>
  );
}

/** Subscription mode: Claude Code and Codex on people's own Claude and ChatGPT plans. */
function Subscriptions({ on, canManage, onChange }: { on: boolean; canManage: boolean; onChange: (on: boolean) => Promise<void> }) {
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const label = "Allow nexus claude and nexus codex --subscription";
  return (
    <Section
      title="Subscription mode"
      footer={
        failure ??
        `Claude Code and Codex on people's own Claude and ChatGPT plans. Calls go straight to Anthropic or OpenAI: Gateway reports their usage but does not check models or budgets. ${canManage ? "On by default." : "Only owners and admins can change it."}`
      }
      footerTone={failure ? "danger" : undefined}
    >
      <Row
        label={label}
        accessory={
          <Switch
            value={on}
            disabled={!canManage || saving}
            accessibilityLabel={label}
            onValueChange={(next) => {
              setSaving(true);
              setFailure(null);
              onChange(next)
                .catch((e: Error) => setFailure(e.message))
                .finally(() => setSaving(false));
            }}
          />
        }
      />
    </Section>
  );
}

function Retention({ settings, canManage, save }: { settings: OrgSettings; canManage: boolean; save: (change: Partial<OrgSettings>) => Promise<void> }) {
  const theme = useTheme();
  const [days, setDays] = useState<string | undefined>();
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const value = days ?? String(settings.chatRetentionDays);
  const valid = /^\d+$/.test(value) && Number(value) >= 1 && Number(value) <= 3650;
  const submit = () => {
    if (!valid || Number(value) === settings.chatRetentionDays) return;
    setMessage(null);
    save({ chatRetentionDays: Number(value) }).then(
      () => {
        setDays(undefined);
        setMessage({ text: "Saved. Messages older than this are deleted." });
      },
      (e: Error) => setMessage({ text: e.message, error: true }),
    );
  };
  return (
    <Section
      title="Chat retention"
      footer={[
        message?.text,
        `1 to 3650 days; ${settings.defaults.chatRetentionDays} by default. ${canManage ? "A shorter window deletes older messages at once." : "Only owners and admins can change it."}`,
      ]
        .filter(Boolean)
        .join(" ")}
      footerTone={message?.error || !valid ? "danger" : undefined}
    >
      <Row
        label="Keep messages for"
        accessory={
          <View style={styles.days}>
            <TextInput
              style={[styles.daysInput, { color: theme.foreground, backgroundColor: theme.userBubble }]}
              value={value}
              onChangeText={setDays}
              keyboardType="number-pad"
              editable={canManage}
              accessibilityLabel="Days to keep chat messages"
              returnKeyType="done"
              onSubmitEditing={submit}
            />
            <Text style={[styles.daysUnit, { color: theme.muted }]}>days</Text>
          </View>
        }
      />
      {canManage && valid && Number(value) !== settings.chatRetentionDays ? <Row label="Save" tone="primary" onPress={submit} /> : null}
    </Section>
  );
}

const styles = StyleSheet.create({
  days: { flexDirection: "row", alignItems: "center", gap: 8 },
  daysInput: { ...text.base, fontFamily: mono, minWidth: 64, minHeight: 36, borderRadius: 10, paddingHorizontal: 10, textAlign: "center" },
  daysUnit: { ...text.base, fontFamily: font.regular },
});
