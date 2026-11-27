import { NEVER_TRAINS, PAUSE_BODY, PAUSE_TITLE, pausedLine, revokedText, revokeText, type AgentClient, type AgentPrivacyView } from "@nexus/kit";
import { useEffect, useState } from "react";
import { Alert } from "react-native";
import { useAuth } from "@/auth";
import { Blank, Empty, Footnote, Loading, Page, Row, Section } from "@/list";
import { accountHead, clock, neverLine, pauseText, privacyFailed, shortList } from "@/privacy-words";
import { useToast } from "@/toast";

type Read = { kind: "loading" } | { kind: "view"; view: AgentPrivacyView } | { kind: "off" } | { kind: "problem"; text: string };

/**
 * Account › Assistant › Privacy (A11): Pause and Resume, Revoke all access, and what Nexus can see per
 * account, as Nexus UI's Privacy view (`agent-privacy.tsx` there). Pause asks first, then stops every errand, watch,
 * brief and message. Revoke all names each link and account that ends, and the organization's accounts stay. Each
 * account lists what it reads, what it writes after you approve, and what it never does there. An agent API without
 * Privacy says so.
 */
export default function AssistantPrivacy() {
  const { agent } = useAuth();
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  return <PrivacyPage agent={agent} />;
}

function PrivacyPage({ agent }: { agent: AgentClient }) {
  const toast = useToast();
  const [read, setRead] = useState<Read>({ kind: "loading" });
  const [reads, setReads] = useState(0);
  const [working, setWorking] = useState(false);

  useEffect(() => {
    const controller = new AbortController();
    agent.privacy(controller.signal).then(
      (call) => {
        if (controller.signal.aborted) return;
        if (call.kind === "ok") setRead({ kind: "view", view: call.value });
        else if (call.kind === "off") setRead({ kind: "off" });
        else setRead({ kind: "problem", text: privacyFailed(call) });
      },
      () => !controller.signal.aborted && setRead({ kind: "problem", text: "Nexus could not reach the Assistant. Try again in a moment." }),
    );
    return () => controller.abort();
  }, [agent, reads]);

  if (read.kind === "loading") return <Loading alone />;
  if (read.kind === "off") return <Blank title="Privacy is not on here" body="This agent service has no Privacy settings yet." />;
  if (read.kind === "problem") return <Blank title="Your privacy settings did not open" body={read.text} tone="danger" />;
  const view = read.view;

  const setPaused = (paused: boolean) => {
    setWorking(true);
    agent.setPaused(paused).then(
      (call) => {
        setWorking(false);
        if (call.kind !== "ok") return Alert.alert("That did not run", privacyFailed(call));
        setRead({ kind: "view", view: { ...view, pause: call.value } });
        toast({ text: pauseText(call.value), haptic: "success" });
      },
      () => {
        setWorking(false);
        Alert.alert("That did not run", "Nexus could not reach the Assistant. Try again in a moment.");
      },
    );
  };

  const revoke = () => {
    setWorking(true);
    agent.revokeAll(true).then(
      (call) => {
        setWorking(false);
        if (call.kind !== "ok") return Alert.alert("That did not run", privacyFailed(call));
        toast({ text: revokedText(call.value), haptic: "success" });
        // Read again: what is still connected, and the links that are left.
        setReads((count) => count + 1);
      },
      () => {
        setWorking(false);
        Alert.alert("That did not run", "Nexus could not reach the Assistant. Try again in a moment.");
      },
    );
  };

  const askPause = () =>
    Alert.alert(PAUSE_TITLE, PAUSE_BODY, [
      { text: "Cancel", style: "cancel" },
      { text: "Pause", onPress: () => setPaused(true) },
    ]);
  const askRevoke = () =>
    Alert.alert("Revoke all access?", revokeText(view), [
      { text: "Cancel", style: "cancel" },
      { text: "Revoke all", style: "destructive", onPress: revoke },
    ]);

  return (
    <Page onRefresh={() => setReads((count) => count + 1)}>
      {view.pause.paused ? (
        <Section footer={pausedLine(view.pause, clock)}>
          <Row label="Resume" tone="primary" disabled={working} onPress={() => setPaused(false)} />
        </Section>
      ) : (
        <Section footer="Every errand, watch, brief and message stops at once. Nothing is deleted.">
          <Row label="Pause the Assistant" tone="primary" disabled={working} onPress={askPause} />
        </Section>
      )}
      <Section footer="Ends every agent link and disconnects your own accounts. You can reconnect one at a time.">
        <Row label="Revoke all access" tone="danger" disabled={working} onPress={askRevoke} />
      </Section>
      <Section title="What it can see" footer={view.accountsError ? `Some accounts could not be read: ${view.accountsError}` : "Per account: what it reads, and what it never does there."}>
        {view.accounts.length ? (
          view.accounts.map((account) => {
            const head = accountHead(account);
            const lines = account.error
              ? [account.error]
              : [`Reads: ${account.reads.length ? shortList(account.reads) : "Nothing"}`, ...(account.writes.length ? [`Writes after you approve: ${shortList(account.writes)}`] : []), `Never: ${neverLine(account)}`];
            return <Row key={`${account.connector}:${account.account}`} label={head.name} value={head.whose} detail={lines.join("\n")} detailTone="tertiary" />;
          })
        ) : (
          <Empty>No accounts are connected. Nexus reads nothing outside Nexus until you connect an account.</Empty>
        )}
      </Section>
      {view.links.length ? (
        <Section title="Agent links" footer={view.linksError}>
          {view.links.map((link) => (
            <Row key={link.id} label={link.label} detail={link.agent} detailTone="tertiary" />
          ))}
        </Section>
      ) : null}
      <Footnote>{NEVER_TRAINS}</Footnote>
    </Page>
  );
}
