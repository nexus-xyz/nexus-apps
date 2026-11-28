import {
  autonomyLadder,
  DISCOVER_IDEAS,
  DISCOVER_NEWS,
  safeNextSteps,
  unseenNews,
  type AgentApprovalsView,
  type AgentClient,
  type AgentMemoryView,
} from "@nexus/kit";
import { router, type Href } from "expo-router";
import * as WebBrowser from "expo-web-browser";
import { useEffect, useState } from "react";
import { askNexusHref } from "@/assistant-handoff";
import { useAuth, useCore, useStored } from "@/auth";
import { connectRows, keepsLine, newsTarget, seenIds, TEACH, type ConnectRow } from "@/discover-words";
import { haptic } from "@/haptics";
import { Blank, Empty, Footnote, Loading, Page, Row, Section } from "@/list";
import { NoteSheet } from "@/memory-state";
import { useToast } from "@/toast";
import { useAssistantName } from "@/use-assistant-name";

/**
 * Discover (A15): what Nexus can do, and how to let it do more, as Nexus UI's Discover view
 * (`agent-discover.tsx` there) and board A15 draw it, in the phone's grouped list:
 *
 * - Try these: ideas with the words filled in. A tap sends the line to the Assistant as the person's message.
 * - Connect more: what each account unlocks. A tap opens the connector's page, where the person connects it.
 * - Autonomy: the approval tiers as a ladder, with the person's spend line and the kinds that still ask first.
 * - Teach it about you: how Nexus learns, "Tell Nexus something about you" (a note) and Open memory.
 * - What's new: a dot on each line until this phone has shown it once.
 *
 * It opens from the empty chat and Account › Assistant. Each live part reads on its own: one that
 * cannot read says so in one line, and the rest of the page still works.
 */
export default function Discover() {
  const { agent } = useAuth();
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  return <DiscoverPage agent={agent} />;
}

/** A read of one live part: undefined while it loads, `failed` once it cannot. */
type Part<T> = { data?: T; failed?: true };

function DiscoverPage({ agent }: { agent: AgentClient }) {
  const core = useCore();
  const { uiLink } = useAuth();
  const assistant = useAssistantName();
  const toast = useToast();
  const seen = useStored<string[]>("cache", "discover-seen");
  // The lines unseen when the page opened keep their dot for this visit; then every line counts as seen.
  const [fresh] = useState(() => new Set(unseenNews(seenIds(seen.load()))));
  const [memory, setMemory] = useState<Part<AgentMemoryView>>({});
  const [approvals, setApprovals] = useState<Part<AgentApprovalsView>>({});
  const [rows, setRows] = useState<Part<ConnectRow[]>>({});
  const [adding, setAdding] = useState<string | undefined>();
  const [working, setWorking] = useState(false);

  useEffect(() => {
    seen.save([...new Set([...seenIds(seen.load()), ...DISCOVER_NEWS.map((item) => item.id)])]);
  }, [seen]);

  useEffect(() => {
    const controller = new AbortController();
    agent.memory(controller.signal).then(
      (read) => !controller.signal.aborted && setMemory(read.kind === "view" ? { data: read.view } : { failed: true }),
      () => !controller.signal.aborted && setMemory({ failed: true }),
    );
    agent.approvals(controller.signal).then(
      (read) => !controller.signal.aborted && setApprovals(read.kind === "view" ? { data: read.view } : { failed: true }),
      () => !controller.signal.aborted && setApprovals({ failed: true }),
    );
    Promise.all([core.connectors(), core.connections().catch(() => ({ connections: [] }))]).then(
      ([catalog, mine]) => !controller.signal.aborted && setRows({ data: connectRows(catalog.connectors, mine.connections) }),
      () => !controller.signal.aborted && setRows({ failed: true }),
    );
    return () => controller.abort();
  }, [agent, core]);

  const go = (href: Href) => {
    haptic("tap");
    router.push(href);
  };
  const add = (text: string) => {
    setWorking(true);
    agent.changeMemory({ add: text }).then(
      (done) => {
        setWorking(false);
        if (done.kind !== "note") return toast({ text: `${assistant} could not keep the note. Try again in Memory.`, haptic: "error" });
        setAdding(undefined);
        setMemory((now) => (now.data ? { data: { ...now.data, notes: [done.note, ...now.data.notes] } } : now));
        toast({ text: `${assistant} keeps the note, from its next message.`, haptic: "success" });
      },
      () => {
        setWorking(false);
        toast({ text: `${assistant} could not keep the note. Try again in Memory.`, haptic: "error" });
      },
    );
  };
  const steps = approvals.data?.tiers === "on" ? safeNextSteps(approvals.data) : [];

  return (
    <Page>
      <Footnote>{`What ${assistant} can do, and how to let it do more.`}</Footnote>
      <Section title="Try these" footer={`One tap sends the words to ${assistant}. Change them in the chat if you like.`}>
        {DISCOVER_IDEAS.map((idea) => (
          <Row key={idea.id} label={idea.title} detail={`“${idea.line}”`} detailTone="tertiary" chevron onPress={() => go(askNexusHref(idea.line) as Href)} accessibilityHint={`Sends it to ${assistant}`} />
        ))}
      </Section>
      <Section title="Connect more" footer="Each account opens new things it can do.">
        {rows.failed ? (
          <Empty>Your accounts did not load. Open Integrations to see them.</Empty>
        ) : !rows.data ? (
          <Loading bare rows={2} />
        ) : rows.data.length === 0 ? (
          <Empty>Your organization offers no accounts to connect yet.</Empty>
        ) : (
          rows.data.map((row) => (
            <Row
              key={row.connector}
              label={row.name}
              detail={row.line}
              detailTone="tertiary"
              value={row.connected ? undefined : "Connect"}
              valueTone="foreground"
              href={{ pathname: "/integration/[name]", params: { name: row.connector } }}
            />
          ))
        )}
      </Section>
      <Section
        title="Autonomy"
        footer={
          approvals.failed
            ? "Your approval settings did not load, so the line shows no amount."
            : approvals.data?.tiers === "off"
              ? "Here, every write asks first for now: the tiers are not on yet."
              : "A ladder: move a thing down one rung when you trust it. You can always move it back."
        }
      >
        {autonomyLadder(approvals.data).map((rung) => (
          <Row key={rung.id} label={rung.title} value={rung.tag} detail={rung.line} detailTone="tertiary" />
        ))}
      </Section>
      {steps.length ? (
        <Section title="Safe next steps" footer="Choose Always allow on its next card.">
          {steps.map((step) => (
            <Row key={step.kind} label={`${step.what}, without asking`} />
          ))}
        </Section>
      ) : null}
      {uiLink ? (
        <Section>
          <Row label="Open Approvals" tone="primary" onPress={() => void WebBrowser.openBrowserAsync(uiLink("/agent?view=approvals")).catch(() => undefined)} accessibilityRole="link" accessibilityHint="Opens it in Nexus on the web" />
        </Section>
      ) : null}
      <Section
        title="Teach it about you"
        footer={memory.failed ? "Memory did not load here." : memory.data ? `${keepsLine(assistant, memory.data.notes.length)} It learns as you go. Correct anything in Memory; it forgets at once.` : undefined}
      >
        {TEACH.map((kind) => (
          <Row key={kind.title} label={kind.title} detail={`“${kind.line}”`} detailTone="tertiary" />
        ))}
        {memory.data?.controls ? <Row label={`Tell ${assistant} something about you`} tone="primary" disabled={working} onPress={() => setAdding("")} /> : null}
        <Row label="Open memory" href="/assistant-memory" />
      </Section>
      <Section title="What’s new" footer="A dot until you’ve seen it.">
        {DISCOVER_NEWS.map((item) => {
          const target = newsTarget(item.action, uiLink);
          const press = !target ? undefined : "href" in target ? () => go(target.href as Href) : () => void WebBrowser.openBrowserAsync(target.url).catch(() => undefined);
          return (
            <Row
              key={item.id}
              label={fresh.has(item.id) ? `● ${item.title}` : item.title}
              accessibilityLabel={`${item.title}${fresh.has(item.id) ? ", new" : ""}, ${item.line}`}
              detail={item.line}
              detailTone="tertiary"
              chevron={Boolean(press)}
              onPress={press}
            />
          );
        })}
      </Section>
      <NoteSheet
        title={`Tell ${assistant} something about you`}
        text={adding}
        submit="Add"
        hint={`${assistant} uses it in your direct messages, from its next message.`}
        placeholder="Lily is in 1st grade at Rooftop."
        working={working}
        onText={setAdding}
        onClose={() => setAdding(undefined)}
        onSubmit={add}
      />
    </Page>
  );
}
