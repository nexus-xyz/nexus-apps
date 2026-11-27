import type { CurrentTenantResponse, ProviderKeyChange, ProviderKeySummary, ProviderView } from "@nexus/kit";
import { router, useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { useAuth, useCore } from "@/auth";
import { Blank, Footnote, Loading, Page, Section } from "@/list";
import { AdminHead, Fact, FootAction, SheetField, SheetForm } from "@/people-controls";
import { checkedLine, keyEnd, shortDay } from "@/people-controls-words";
import { provider as providerInfo } from "@/providers";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { useToast } from "@/toast";

/**
 * One provider key (P26·D's row, opened full screen on P105·D): its end, when it was added, what the provider answered
 * when it was last checked, when a call last used it and whether the policy lets calls through that provider. Its one
 * action, Replace key, is a sheet; turning it off or on and removing it are under ⋯. Owners and admins only change it.
 */
export default function ProviderKey() {
  const core = useCore();
  const { orgName } = useAuth();
  const toast = useToast();
  const { id } = useLocalSearchParams<{ id: string }>();
  const [key, setKey] = useState<ProviderKeySummary | null>(null);
  const [view, setView] = useState<ProviderView | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [replacing, setReplacing] = useState(false);
  const [more, setMore] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const load = useCallback(() => {
    core.me().then(setMe, () => undefined);
    core.providerKey(id).then(
      (k) => {
        setKey(k);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
    core.listProviders().then((r) => setView(r.data.find((v) => v.id === id) ?? null), () => setView(null));
  }, [core, id]);
  useEffect(load, [load]);

  const name = view?.name ?? providerInfo(id).name;
  const canManage = !!me?.permissions?.includes("provider_keys.manage");
  const done = (change: ProviderKeyChange, text: string) => {
    toast({ text: change.changed ? text : `Already so: ${text.toLowerCase()}`, icon: "keys" });
    load();
  };
  const change = (work: () => Promise<ProviderKeyChange>, text: string) => {
    setMore(false);
    work().then((c) => done(c, text), (e: Error) => toast({ text: e.message }));
  };
  // The sheet asks before it removes, step by step in place: iOS drops an alert presented while a sheet is closing.
  const remove = () => {
    setMore(false);
    setConfirming(false);
    core.removeProviderKey(id).then(
      () => {
        toast({ text: `Removed the ${name} key` });
        router.back();
      },
      (e: Error) => toast({ text: e.message }),
    );
  };
  const replace = () => {
    setBusy(true);
    setProblem(null);
    core
      .replaceProviderKey(id, typed.trim())
      .then((c) => {
        setReplacing(false);
        setTyped("");
        done(c, `Replaced the ${name} key`);
      })
      .catch((e: Error) => setProblem(e.message))
      .finally(() => setBusy(false));
  };

  if (!key) return error ? <Blank title={`Couldn't load the ${name} key`} body={error} tone="danger" /> : <Loading alone />;
  const off = key.state === "disabled";
  return (
    <Page onRefresh={load}>
      <AdminHead title={name} purpose={`${me?.tenant.name ?? orgName ?? "Your organization"}'s own ${name} key.`} more={canManage ? { label: "More", onPress: () => setMore(true) } : undefined} />
      <Section>
        <Fact label="Key" value={keyEnd(key.last4)} />
        <Fact label="Added" value={shortDay(key.createdAt)} />
        {checkedLine(key) ? <Fact label="Checked" value={checkedLine(key)!} valueTone={key.state === "rejected" ? "danger" : undefined} /> : null}
        <Fact label="Last used" value={key.lastUsedOn ? shortDay(key.lastUsedOn) : "Not yet"} />
        {view?.enabled !== undefined ? <Fact label="Policy" value={view.enabled ? `Calls may go through ${name}` : `${name} is off in Policy`} /> : null}
        {key.versions && key.versions.length > 1 ? <Fact label="Keys stored" value={`${key.versions.length}, the newest in use`} /> : null}
      </Section>
      {key.unused ? <Footnote>{key.notice ?? `${name} is no longer used`}. The saved key is kept but never used.</Footnote> : null}
      {canManage ? <FootAction label="Replace key" onPress={() => setReplacing(true)} /> : <Footnote>Only owners and admins can change a provider key.</Footnote>}

      <Sheet visible={replacing} onClose={() => setReplacing(false)}>
        <SheetForm title={`Replace the ${name} key`} action="Save key" onSubmit={replace} disabled={!typed.trim()} busy={busy} note="Gateway checks it with the provider before it is used." error={problem}>
          <SheetField
            monoText
            value={typed}
            onChangeText={setTyped}
            placeholder="New key"
            secureTextEntry
            autoCapitalize="none"
            autoCorrect={false}
            textContentType="none"
            autoComplete="off"
            accessibilityLabel={`New ${name} API key`}
          />
        </SheetForm>
      </Sheet>
      <Sheet
        visible={more}
        onClose={() => {
          setMore(false);
          setConfirming(false);
        }}
      >
        {confirming ? (
          <>
            <SheetTitle>{`Remove the ${name} key? Calls to ${name}'s models are refused until there is a key again.`}</SheetTitle>
            <SheetRow label="Remove key" tone="danger" onPress={remove} />
            <SheetRow label="Keep it" tone="muted" onPress={() => setConfirming(false)} />
          </>
        ) : (
          <>
            <SheetTitle>{`${name} key ${keyEnd(key.last4)}`}</SheetTitle>
            {off ? (
              <SheetRow label="Turn on" onPress={() => change(() => core.enableProviderKey(id), `Turned on the ${name} key`)} />
            ) : (
              <SheetRow label="Turn off" onPress={() => change(() => core.disableProviderKey(id), `Turned off the ${name} key`)} />
            )}
            <SheetRow label="Remove key" tone="danger" onPress={() => setConfirming(true)} />
            <SheetRow label="Close" tone="muted" onPress={() => setMore(false)} />
          </>
        )}
      </Sheet>
    </Page>
  );
}
