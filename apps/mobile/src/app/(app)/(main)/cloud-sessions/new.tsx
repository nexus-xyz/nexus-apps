import { CoreError, newIdempotencyKey, safeUrl, type Connection, type CreateSession, type ModelEntry } from "@nexus/kit";
import { cloudAgentForModel, defaultCloudModel, groupCloudModels, isValidRepo, normalizeRepoInput } from "@nexus/kit/cloud-session-helpers";
import { router, Stack, useIsFocused } from "expo-router";
import * as WebBrowser from "expo-web-browser";
import { useEffect, useMemo, useRef, useState } from "react";
import { Keyboard, ScrollView, useWindowDimensions } from "react-native";
import { useCore } from "@/auth";
import { cloudError } from "@/cloud-sessions";
import { clientIdentity, cloudDraft, forgetCloudDraft } from "@/connection-intent";
import { haptic } from "@/haptics";
import { Field, Footnote, Loading, Page, Row, Section } from "@/list";
import { ProviderMark } from "@/provider-mark";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { useCloudSessions } from "@/use-cloud-sessions";

export default function NewCloudSession() {
  const core = useCore();
  return <CloudForm key={clientIdentity(core)} />;
}

function CloudForm() {
  const core = useCore();
  const focused = useIsFocused();
  const availability = useCloudSessions(core, focused);
  const { height } = useWindowDimensions();
  const [catalog, setCatalog] = useState<ModelEntry[]>();
  const [catalogError, setCatalogError] = useState("");
  const [catalogAttempt, setCatalogAttempt] = useState(0);
  const [repo, setRepo] = useState("");
  const [handoff] = useState(() => cloudDraft(core));
  useEffect(() => { forgetCloudDraft(core, handoff); }, [core, handoff]);
  const [prompt, setPrompt] = useState(handoff?.text ?? "");
  const [selection, setSelection] = useState("");
  const [picker, setPicker] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const [githubAction, setGithubAction] = useState<"connect" | "choose">();
  const [connecting, setConnecting] = useState(false);
  const [accounts, setAccounts] = useState<Connection[]>();
  const [githubAccount, setGithubAccount] = useState<string>();
  const connect = async () => {
    if (connecting) return;
    setConnecting(true);
    try {
      const url = safeUrl((await core.startOAuth("github", "user")).authorizeUrl);
      if (!url) throw new Error("Could not open GitHub authorization.");
      await WebBrowser.openBrowserAsync(url);
    } catch (error) {
      if (active.current) setError(cloudError(error));
    } finally {
      if (active.current) setConnecting(false);
    }
  };
  const pending = useRef(false);
  const last = useRef<{ signature: string; id: string } | null>(null);
  const active = useRef(false);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  const ready = availability.result?.state === "ready";
  useEffect(() => {
    if (!ready) return;
    let current = true;
    void core.models({ status: "supported" }).then(
      ({ data }) => { if (current) setCatalog(data); },
      (error: unknown) => { if (current) setCatalogError(cloudError(error)); },
    );
    return () => { current = false; };
  }, [core, ready, catalogAttempt]);
  const groups = useMemo(() => groupCloudModels(catalog ?? []), [catalog]);
  const model = groups.flatMap(({ models }) => models).find(({ id }) => id === selection) ?? defaultCloudModel(groups);
  const chooseGithubAccount = async () => {
    setGithubAction("choose");
    try {
      const { connections } = await core.connections();
      if (active.current) setAccounts(connections.filter((item) =>
        item.connector === "github" && item.scope === "user" && item.status === "active" && item.kind === "oauth2" && !item.suspended,
      ));
    } catch (error) { if (active.current) setError(cloudError(error)); }
  };
  const submit = async () => {
    if (pending.current || !ready || !model) return;
    const repository = normalizeRepoInput(repo);
    const agent = cloudAgentForModel(model);
    if (!isValidRepo(repository)) { haptic("error"); setError("Enter a GitHub repository as owner/name."); return; }
    if (!agent || !prompt.trim()) return;
    const body: CreateSession = { repo: repository, agent, model: model.id, prompt: prompt.trim() };
    const signature = JSON.stringify(body);
    if (last.current?.signature !== signature) last.current = { signature, id: newIdempotencyKey() };
    pending.current = true;
    setSubmitting(true);
    setError("");
    setGithubAction(undefined);
    Keyboard.dismiss();
    haptic("send");
    try {
      const session = await core.createCloudSession(body, { idempotencyKey: last.current.id, githubAccount });
      if (!active.current) return;
      handoff?.started?.();
      haptic("success");
      router.replace({ pathname: "/cloud-sessions/[id]", params: { id: session.id } });
    } catch (error) {
      if (!active.current) return;
      setError(cloudError(error));
      haptic("error");
      if (error instanceof CoreError) {
        if (error.code === "github_connection_required" || error.code === "credentials_required" || error.code === "connection_required" || error.code === "connection_expired") {
          setGithubAction("connect");
        } else if (error.code === "account_required") {
          await chooseGithubAccount();
        }
      }
    } finally {
      pending.current = false;
      if (active.current) setSubmitting(false);
    }
  };
  return <Page>
    <Stack.Screen options={{ title: "New Cloud session" }} />
    {!availability.result ? <Loading /> : null}
    {availability.result?.state === "disabled" ? <Footnote>This Gateway does not have Cloud sessions enabled.</Footnote> : null}
    {availability.result?.state === "error" ? <Section footer={availability.result.error} footerTone="danger">
      <Row label="Retry" onPress={availability.refresh} />
    </Section> : null}
    {ready ? <>
      <Section title="GitHub repository">
        <Field value={repo} onChangeText={setRepo} placeholder="owner/name" accessibilityLabel="GitHub repository"
          autoCorrect={false} autoCapitalize="none" editable={!submitting} maxLength={320} />
        <Row label="GitHub account" detail={accounts?.find((account) => account.account === githubAccount)?.name ?? "Automatic"}
          disabled={submitting || connecting} onPress={() => { haptic("tap"); void chooseGithubAccount(); }} chevron />
      </Section>
      <Section title="Model">
        <Row label={model?.name ?? "Choose a model"} leading={model ? <ProviderMark model={model.id} /> : undefined}
          detail={groups.find(({ agent }) => agent === (model && cloudAgentForModel(model)))?.agentLabel}
          disabled={!groups.length || submitting} onPress={() => { haptic("tap"); Keyboard.dismiss(); setPicker(true); }} chevron />
      </Section>
      {catalogError ? <Section footer={catalogError} footerTone="danger">
        <Row label="Retry loading models" onPress={() => { setCatalogError(""); setCatalogAttempt((n) => n + 1); }} />
      </Section> : catalog && !groups.length ? <Footnote>No models are available for Cloud sessions in this organization.</Footnote> : null}
      <Section title="Prompt">
        <Field value={prompt} onChangeText={setPrompt} accessibilityLabel="Cloud session prompt" placeholder="What should the agent do?"
          editable={!submitting} multiline textAlignVertical="top" style={{ minHeight: 112 }} maxLength={100_000} />
      </Section>
      {githubAction === "choose" && accounts ? <Section title="GitHub account">
        <Row label="Automatic" selected={!githubAccount}
          onPress={() => { haptic("selection"); setGithubAccount(undefined); setGithubAction(undefined); }} />
        {!accounts.length ? <Row label="Connect GitHub" disabled={connecting}
          onPress={() => { haptic("tap"); void connect(); }} /> : null}
        {accounts.map((account) => <Row key={account.account} label={account.name} selected={account.account === githubAccount}
          onPress={() => { haptic("selection"); setGithubAccount(account.account); setGithubAction(undefined); }} />)}
      </Section> : null}
      <Section footer={error || undefined} footerTone="danger">
        {githubAction === "connect" ? <Row label={connecting ? "Opening GitHub…" : "Connect GitHub"} tone="primary" disabled={connecting}
          onPress={() => { haptic("tap"); void connect(); }} /> : null}
        <Row label={submitting ? "Starting…" : "Start in Cloud"} tone="primary" disabled={submitting || connecting || !repo.trim() || !prompt.trim() || !model}
          onPress={() => void submit()} />
      </Section>
      <Row label="Cloud sessions" href="/cloud-sessions" disabled={submitting || connecting} />
    </> : null}
    <Sheet visible={picker} onClose={() => setPicker(false)}>
      <ScrollView style={{ maxHeight: height * 0.62 }} keyboardShouldPersistTaps="handled">
        {groups.map(({ agent, agentLabel, models }) => <Section key={agent} header={<SheetTitle>{agentLabel}</SheetTitle>}>
          {models.map((choice) => <SheetRow key={choice.id} label={choice.name} tone={choice.id === model?.id ? "primary" : "normal"}
            onPress={() => { haptic("selection"); setSelection(choice.id); setPicker(false); }} />)}
        </Section>)}
      </ScrollView>
    </Sheet>
  </Page>;
}
