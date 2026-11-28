import type { SkillLibraryEntry, SkillSummary, TeamSummary } from "@nexus/kit";
import { Stack, useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import { useArtifacts, useAuth, useCore, useStored } from "@/auth";
import { Empty, Footnote, LargeTitle, Loading, Page, Row, Section } from "@/list";
import { CustomizeTabs, useCustomize } from "@/customize-tabs";

/**
 * Skills, as brand v5's P69a·D draws it on the phone: the title in the list, yours, then the organization's library
 * (the approved skills you may load besides your own, each under the organization's or its team's name), each as its
 * command and the version a session loads, a tap to read it (`skill/[id]`). Read on the phone: skills are written and
 * shared on a computer, so the board's + is left out.
 */
export default function Skills() {
  const customize = useCustomize();
  const artifacts = useArtifacts();
  const core = useCore();
  const { orgName } = useAuth();
  // As they were last time, until the Gateway answers.
  const cache = useStored<SkillSummary[]>("cache", "skills");
  const [skills, setSkills] = useState<SkillSummary[] | null>(() => cache.load() ?? null);
  const [library, setLibrary] = useState<SkillLibraryEntry[]>([]);
  const [teams, setTeams] = useState<TeamSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  useFocusEffect(
    useCallback(() => {
      artifacts.skills().then(
        (r) => {
          setSkills(r.skills);
          cache.save(r.skills);
          setError(null);
        },
        (e: Error) => setError(e.message),
      );
      // A Gateway before the library has none: the section is left out.
      artifacts.skillLibrary().then((r) => setLibrary(r.skills), () => setLibrary([]));
      core.teams().then((r) => setTeams(r.teams), () => undefined);
    }, [artifacts, core, cache]),
  );
  if (!skills && !error && !customize) return <Loading alone />;
  const org = orgName ?? "";
  const scope = (entry: SkillLibraryEntry) => (entry.scope === "team" ? (teams.find((t) => t.id === entry.teamId)?.name ?? "A team") : org || "The organization");
  return (
    <Page>
      <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      {customize ? <CustomizeTabs active="/skills" /> : <LargeTitle>Skills</LargeTitle>}
      {!skills && !error ? <Loading /> : null}
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      <Section title="Yours">
        {skills && !skills.length ? <Empty>No skills yet. Write one on a computer, then load it in a chat with /its-name.</Empty> : null}
        {skills?.map((s) => (
          <Row key={s.id} label={`/${s.name}`} monoLabel detail={s.description || undefined} value={`v${s.latestVersion}`} monoValue href={{ pathname: "/skill/[id]", params: { id: s.id } }} />
        ))}
      </Section>
      {library.length ? (
        <Section title={org ? `${org}’s library` : "The library"}>
          {library.map((s) => (
            <Row
              key={s.id}
              label={`/${s.name}`}
              monoLabel
              detail={s.duplicateName ? `${scope(s)} · Not loaded: another skill has this name` : scope(s)}
              detailTone={s.duplicateName ? "danger" : undefined}
              value={`v${s.version}`}
              monoValue
              href={{ pathname: "/skill/[id]", params: { id: s.id } }}
            />
          ))}
        </Section>
      ) : null}
      <Footnote>Type / in any session to use one. Write and share skills on a computer.</Footnote>
    </Page>
  );
}
