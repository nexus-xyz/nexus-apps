import { errandName, searchErrands, type AgentErrand } from "@nexus/kit";
import { router } from "expo-router";
import { useEffect, useState } from "react";
import { View } from "react-native";
import { errandGroups, errandLine, historyLine, inProject } from "@/agent-errands";
import { useAuth } from "@/auth";
import { errandHref } from "@/drawer-places";
import { useErrandMenu } from "@/errand-menu";
import { ErrandShape } from "@/errand-shape";
import { Blank, Empty, Field, Loading, Page, Row, Section } from "@/list";
import { RowMenu } from "@/row-menu";
import { useTheme } from "@/theme";

type Read = { kind: "loading" } | { kind: "off" } | { kind: "problem"; text: string } | { kind: "ready"; errands: AgentErrand[]; history: AgentErrand[] };

/**
 * Errands (A12), from the Assistant's menu: the open errands in their groups, Needs you then Running, and under
 * them the History, the closed and archived errands, which a search narrows (A12·e). A row opens the errand's thread
 * in the drawer; a long press opens its menu. An agent API with errands off says so.
 */
export default function Errands() {
  const { agent } = useAuth();
  const theme = useTheme();
  const [read, setRead] = useState<Read>({ kind: "loading" });
  const [query, setQuery] = useState("");
  const [reads, setReads] = useState(0);
  const menu = useErrandMenu(agent!, () => setReads((count) => count + 1));

  useEffect(() => {
    if (!agent) return;
    const controller = new AbortController();
    agent.errands(controller.signal).then(
      (list) => {
        if (controller.signal.aborted) return;
        if (list.kind === "list") setRead({ kind: "ready", errands: list.errands.filter((errand) => !errand.archived), history: list.history ?? [] });
        else if (list.kind === "off") setRead({ kind: "off" });
        else setRead({ kind: "problem", text: list.failure.detail || "Your errands could not be read. Try again in a moment." });
      },
      () => !controller.signal.aborted && setRead({ kind: "problem", text: "Nexus could not reach the Assistant. Try again in a moment." }),
    );
    return () => controller.abort();
  }, [agent, reads]);

  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  if (read.kind === "loading") return <Loading alone />;
  if (read.kind === "off") return <Blank title="Errands are not on here" body="Your organization's Assistant does not keep errands yet." />;
  if (read.kind === "problem") return <Blank title="Your errands did not open" body={read.text} tone="danger" />;

  const row = (errand: AgentErrand, line: string) => (
    <RowMenu key={errand.id} title={errandName(errand)} actions={menu.actions(errand)}>
      {(onLongPress) => (
        <Row
          leading={
            <View style={{ width: 18, alignItems: "center" }}>
              <ErrandShape status={errand.status} color={theme.foreground} />
            </View>
          }
          label={errandName(errand)}
          detail={line}
          detailTone="tertiary"
          chevron
          onPress={() => router.dismissTo(errandHref(errand.id))}
          onLongPress={onLongPress}
          accessibilityLabel={`${errandName(errand)}, ${line}`}
        />
      )}
    </RowMenu>
  );
  const groups = errandGroups(read.errands);
  const shown = searchErrands(read.history, query);
  return (
    <Page onRefresh={() => setReads((count) => count + 1)}>
      {groups.length ? null : <Empty>No errand is open. Ask Nexus in the main chat to start one.</Empty>}
      {groups.map((group) => (
        <Section key={group.title} title={group.title}>
          {group.errands.map((errand) => row(errand, [inProject(errand), errandLine(errand)].filter(Boolean).join(" · ")))}
        </Section>
      ))}
      {read.history.length ? (
        <Section title={`History · ${read.history.length}`}>
          <Field value={query} onChangeText={setQuery} placeholder="Search errands" accessibilityLabel="Search errands" returnKeyType="search" clearButtonMode="while-editing" autoCorrect={false} />
          {shown.length ? shown.map((errand) => row(errand, historyLine(errand))) : <Empty>No errand matches.</Empty>}
        </Section>
      ) : null}
      {menu.sheet}
    </Page>
  );
}
