import type { AgentClient, AgentProjectList } from "@nexus/kit";
import { useEffect, useState } from "react";
import { useAuth } from "@/auth";
import { Blank, Empty, Footnote, Loading, Page, Row, Section } from "@/list";
import { gatewayNote, NO_PROJECTS, projectHref, projectLine } from "@/project-words";

type Read = { kind: "loading" } | { kind: "off" } | { kind: "problem"; text: string } | { kind: "ready"; list: AgentProjectList };

/**
 * Projects (A14), from the Assistant's menu: the person's projects, each a name their errands share
 * or a Gateway project with repositories. A row opens the project's page. Move to project, in an errand's menu, starts
 * a project.
 */
export default function Projects() {
  const { agent } = useAuth();
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  return <ProjectsPage agent={agent} />;
}

function ProjectsPage({ agent }: { agent: AgentClient }) {
  const [read, setRead] = useState<Read>({ kind: "loading" });
  const [reads, setReads] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    agent.projects(controller.signal).then(
      (call) => {
        if (controller.signal.aborted) return;
        if (call.kind === "list") setRead({ kind: "ready", list: call.list });
        else if (call.kind === "off") setRead({ kind: "off" });
        else setRead({ kind: "problem", text: call.failure.detail || "Your projects could not be read. Try again in a moment." });
      },
      () => !controller.signal.aborted && setRead({ kind: "problem", text: "Nexus could not reach the Assistant. Try again in a moment." }),
    );
    return () => controller.abort();
  }, [agent, reads]);

  if (read.kind === "loading") return <Loading alone />;
  if (read.kind === "off") return <Blank title="Projects are not on here" body="Your organization's Assistant does not keep errands yet." />;
  if (read.kind === "problem") return <Blank title="Your projects did not open" body={read.text} tone="danger" />;
  const { projects, gateway } = read.list;
  const note = gatewayNote(gateway);
  return (
    <Page onRefresh={() => setReads((count) => count + 1)}>
      {projects.length ? (
        <Section title="Projects">
          {projects.map((project) => (
            <Row key={project.id ?? `name:${project.name}`} label={project.name} detail={projectLine(project)} detailTone="tertiary" href={projectHref(project)} />
          ))}
        </Section>
      ) : (
        <Empty>{NO_PROJECTS}</Empty>
      )}
      {note ? <Footnote>{note}</Footnote> : null}
    </Page>
  );
}
