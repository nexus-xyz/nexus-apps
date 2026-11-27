import type { RepositoriesResponse } from "@nexus/kit";
import { Stack, useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { useCore } from "@/auth";
import { Fact } from "@/fact";
import { Blank, Footnote, LargeTitle, Loading, Page, Row, Section } from "@/list";
import { open } from "@/markdown";

const day = (iso: string) => new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" });

/**
 * One linked repository, full screen (brand v5's P105·D "An item", from P71d·R4): its name, visibility, default branch,
 * when it was linked and through which GitHub account; and Open on GitHub. Unlinking stays on GitHub.
 */
export default function Repository() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const core = useCore();
  const [linked, setLinked] = useState<RepositoriesResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    core.repositories().then(
      (r) => {
        setLinked(r);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
  }, [core]);
  useEffect(load, [load]);

  const repo = linked?.repositories.find((r) => r.id === id);
  if (!linked) return error ? <Blank title="This repository did not load" body={error} tone="danger" /> : <Loading alone />;
  if (!repo) return <Blank title="This repository is no longer linked" body="It was unlinked on GitHub, or the App lost access to it." />;

  const name = repo.fullName.slice(repo.fullName.indexOf("/") + 1);
  const account = linked.installations.find((i) => i.id === repo.installationId)?.account;
  return (
    <Page onRefresh={load}>
      <Stack.Screen options={{ title: name, headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>{name}</LargeTitle>
      <Section>
        <Fact label="Repository" value={repo.fullName} monoValue />
        <Fact label="Visibility" value={repo.private ? "Private" : "Public"} />
        {repo.defaultBranch ? <Fact label="Default branch" value={repo.defaultBranch} monoValue /> : null}
        <Fact label="Linked" value={account ? `${day(repo.linkedAt)} · through @${account}` : day(repo.linkedAt)} />
      </Section>
      <Section>
        <Row label="Open on GitHub" chevron onPress={() => open(repo.url)} accessibilityRole="link" accessibilityHint="Opens it on GitHub" />
      </Section>
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
    </Page>
  );
}
