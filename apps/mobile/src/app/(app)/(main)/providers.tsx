import type { CurrentTenantResponse, ProviderKeySummary, ProviderView } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { useAuth, useCore } from "@/auth";
import { Empty, Footnote, Loading, Page, Row, Section, Segmented } from "@/list";
import { AdminHead, SheetField, SheetForm } from "@/people-controls";
import { keyEnd, keyState } from "@/people-controls-words";
import { provider as providerInfo } from "@/providers";
import { Sheet } from "@/sheet";
import { useToast } from "@/toast";

/** A key's placeholder, for the providers whose key shape people know. */
const PLACEHOLDER: Record<string, string> = { anthropic: "sk-ant-…", openai: "sk-…" };
/** The providers Gateway calls directly, when it cannot list its own. */
const FALLBACK = ["anthropic", "openai"];

/**
 * Providers (brand v5's P26·D on the P105·D system): the organization's own provider keys, each with its end and the
 * account it belongs to, and its state on the right when it does not work; each opens the key (`provider/[id].tsx`),
 * where owners and admins replace, turn off or remove it. + adds a key in a sheet. It was the provider half of Keys.
 * Left out: the design's order under the list ("your key, then your plan, then Nexus credits"), which the Gateway does
 * not report, and team and personal keys, which are set on desktop and the web.
 */
export default function Providers() {
  const core = useCore();
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [keys, setKeys] = useState<ProviderKeySummary[] | null>(null);
  const [views, setViews] = useState<ProviderView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  const load = useCallback(() => {
    core.me().then(setMe, () => undefined);
    core.listProviderKeys().then(
      (r) => {
        setKeys(r.providers);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    // The providers this Gateway calls; an older Gateway does not list them.
    core.listProviders().then((r) => setViews(r.data), () => setViews([]));
  }, [core]);
  useEffect(load, [load]);

  const canManage = !!me?.permissions?.includes("provider_keys.manage");
  const name = (id: string) => views.find((v) => v.id === id)?.name ?? providerInfo(id).name;
  const choices = views.length ? views.filter((v) => v.byok && v.status === "active").map((v) => v.id) : FALLBACK;
  const { orgName } = useAuth();
  // The name the sign-in knows until Gateway answers; a lower-case stand-in, since it sits mid-sentence.
  const org = me?.tenant.name ?? orgName ?? "your organization";

  return (
    <Page onRefresh={load}>
      <AdminHead
        title="Providers"
        purpose={`Whose account serves each call: ${org}'s own keys with the model makers.`}
        add={canManage && choices.length ? { label: "Add a provider key", onPress: () => setAdding(true) } : undefined}
      />
      {error && !keys ? <Footnote tone="danger">The provider keys could not be loaded. {error}</Footnote> : null}
      {!keys && !error ? <Loading /> : null}
      {keys && keys.length === 0 ? (
        <Section title="In effect now" footer={canManage ? "Add a key with +." : "Ask an owner or admin to add one."}>
          <Empty>No provider keys yet. Model calls are refused until there is a key for the model's provider.</Empty>
        </Section>
      ) : null}
      {keys?.length ? (
        <Section
          footer={`Bring your own key. It is used for ${org} only; calls still count against your budget and spend limits.${!canManage && me ? " Only owners and admins can add or replace a provider key." : ""}`}
        >
          {keys.map((k) => {
            const state = keyState(k);
            return (
              <Row
                key={k.provider}
                label={name(k.provider)}
                detail={`${keyEnd(k.last4)} · ${org}`}
                value={state?.text}
                valueTone={state?.danger ? "danger" : undefined}
                href={{ pathname: "/provider/[id]", params: { id: k.provider } }}
              />
            );
          })}
        </Section>
      ) : null}
      <AddKey visible={adding} onClose={() => setAdding(false)} choices={choices} name={name} keys={keys ?? []} onSaved={load} />
    </Page>
  );
}

/** Add, or replace, the organization's key for one provider, in a sheet. Gateway checks it with the provider first. */
function AddKey({ visible, onClose, choices, name, keys, onSaved }: { visible: boolean; onClose: () => void; choices: string[]; name: (id: string) => string; keys: ProviderKeySummary[]; onSaved: () => void }) {
  const core = useCore();
  const toast = useToast();
  const [picked, setPicked] = useState<string | undefined>();
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const provider = picked && choices.includes(picked) ? picked : choices[0];
  const replacing = keys.some((k) => k.provider === provider && !k.unused);

  const save = () => {
    if (!provider) return;
    setBusy(true);
    setError(null);
    core
      .addProviderKey(provider, key.trim())
      .then((saved) => {
        setKey("");
        toast({ text: `Saved the ${name(provider)} key ending ${saved.last4}`, icon: "keys" });
        onSaved();
        onClose();
      })
      .catch((e: Error) => setError(e.message))
      .finally(() => setBusy(false));
  };

  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetForm
        title={provider ? `${replacing ? "Replace" : "Add"} the ${name(provider)} key` : "Add a key"}
        action="Save key"
        onSubmit={save}
        disabled={!key.trim() || !provider}
        busy={busy}
        note="Nexus keeps it encrypted and shows only its end."
        error={error}
      >
        {choices.length > 1 ? <Segmented label="Provider" options={choices.map((id) => ({ id, label: name(id) }))} value={provider ?? ""} onChange={setPicked} /> : null}
        <SheetField
          monoText
          value={key}
          onChangeText={setKey}
          placeholder={(provider && PLACEHOLDER[provider]) || "API key"}
          secureTextEntry
          autoCapitalize="none"
          autoCorrect={false}
          textContentType="none"
          autoComplete="off"
          accessibilityLabel={`${provider ? name(provider) : "Provider"} API key`}
        />
      </SheetForm>
    </Sheet>
  );
}
