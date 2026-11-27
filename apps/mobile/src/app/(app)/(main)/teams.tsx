import type { CurrentTenantResponse, PendingTeamMember, TeamTreeResponse } from "@nexus/kit";
import { useFocusEffect } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { useCore } from "@/auth";
import { CreateTeam } from "@/create-team";
import { Empty, Footnote, Loading, Page, Row, Section } from "@/list";
import { AdminHead } from "@/people-controls";
import { people, teamLine } from "@/people-controls-words";
import { flattenTeams } from "@/teams";

/**
 * Teams (brand v5's P71c·T5 on the P105·D system): the tree as one list under the organization and its headcount, each
 * team with its people and its lead, indented under its parent, drilling into the team. For owners and admins, + creates
 * a team in a sheet, and the people pending until they join are one row that drills in (`not-joined.tsx`).
 */
export default function Teams() {
  const core = useCore();
  const [tree, setTree] = useState<TeamTreeResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [pending, setPending] = useState<PendingTeamMember[]>([]);
  const [creating, setCreating] = useState(false);

  const load = useCallback(() => {
    core.teamTree().then(
      (t) => {
        setTree(t);
        setError(null);
      },
      (e: Error) => setError(e.message),
    );
  }, [core]);
  useFocusEffect(load);
  useEffect(() => {
    core.me().then(setMe, () => undefined);
  }, [core]);
  const manage = !!me?.permissions?.includes("teams.manage");
  useEffect(() => {
    if (manage) core.pendingTeamMembers().then((r) => setPending(r.pending), () => undefined);
  }, [core, manage, tree]);

  const all = tree ? flattenTeams(tree.tree) : [];
  const headcount = (tree?.tree ?? []).reduce((sum, t) => sum + t.headcount, 0);
  const org = me?.tenant.name ?? "";
  const waiting = new Set(pending.map((p) => p.email.toLowerCase())).size;

  return (
    <Page onRefresh={load}>
      <AdminHead
        title="Teams"
        purpose={`How ${org || "your organization"} is shaped: its teams, their people and who leads them.`}
        add={manage ? { label: "New team", onPress: () => setCreating(true) } : undefined}
      />
      {error && !tree ? <Footnote tone="danger">{error}</Footnote> : null}
      {!tree && !error ? <Loading /> : null}

      {manage && waiting ? (
        <Section>
          <Row label="Not joined yet" value={String(waiting)} href="/not-joined" />
        </Section>
      ) : null}

      {tree ? (
        <Section
          title={[org, people(headcount)].filter(Boolean).join(" · ")}
          footer={tree.default ? undefined : "You have no default team: your spend is unassigned until you name one."}
        >
          {all.length === 0 ? <Empty>No teams yet. {manage ? "Create the first one with +." : "An owner or admin of your organization creates them."}</Empty> : null}
          {all.map((team) => (
            <Row key={team.id} indent={team.depth - 1} label={team.name} detail={teamLine(team)} href={{ pathname: "/team/[slug]", params: { slug: team.slug } }} />
          ))}
        </Section>
      ) : null}

      <CreateTeam visible={creating} onClose={() => setCreating(false)} onCreated={load} />
    </Page>
  );
}
