import { PROACTIVITY_DIAL, type AgentClient, type AgentProactivityRead, type AgentProactivityView, type ProactivityChange } from "@nexus/kit";
import { useEffect, useState } from "react";
import { Alert } from "react-native";
import { useAuth } from "@/auth";
import { Blank, Field, Footnote, Loading, Page, Row, Section } from "@/list";
import { batchLine, DEFAULT_QUIET_HOURS, dialText, failedText, mutedLine, quietHoursOf, quietText, stepLine } from "@/proactivity-words";
import { Switch } from "@/switch";
import { useToast } from "@/toast";

const zone = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
};

/**
 * Account › Assistant › Proactivity (A10): how often Nexus writes first. One dial, Quiet, Balanced or
 * Hands-on, each with what it sends and about how often (A10·a, g). Quiet hours hold everything but the urgent, from
 * one time to another in the phone's time zone. The kinds turned off at every step come back with one row. Each change
 * saves at once (`PUT /agent/proactivity`). An agent API with proactivity off says so.
 */
export default function Proactivity() {
  const { agent } = useAuth();
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  return <ProactivityPage agent={agent} />;
}

function ProactivityPage({ agent }: { agent: AgentClient }) {
  const toast = useToast();
  const [read, setRead] = useState<AgentProactivityRead | "unreachable">();
  const [reads, setReads] = useState(0);
  const [working, setWorking] = useState(false);
  const [start, setStart] = useState<string>(DEFAULT_QUIET_HOURS.start);
  const [end, setEnd] = useState<string>(DEFAULT_QUIET_HOURS.end);

  useEffect(() => {
    const controller = new AbortController();
    agent.proactivity(controller.signal).then(
      (result) => {
        if (controller.signal.aborted) return;
        setRead(result);
        if (result.kind === "view" && result.view.quietHours) {
          setStart(result.view.quietHours.start);
          setEnd(result.view.quietHours.end);
        }
      },
      () => !controller.signal.aborted && setRead("unreachable"),
    );
    return () => controller.abort();
  }, [agent, reads]);

  const change = (next: ProactivityChange, said: (view: AgentProactivityView) => string) => {
    if (working) return;
    setWorking(true);
    agent.setProactivity(next).then(
      (result) => {
        setWorking(false);
        if (result.kind !== "view") return Alert.alert("That was not saved", failedText(result));
        setRead(result);
        toast({ text: said(result.view), haptic: "success" });
      },
      () => {
        setWorking(false);
        Alert.alert("That was not saved", "That was not saved. Try again in a moment.");
      },
    );
  };

  if (read === undefined) return <Loading alone />;
  if (read !== "unreachable" && read.kind === "off") return <Blank title="Proactivity is not on here" body="Proactivity is not on for Nexus here yet." />;
  if (read === "unreachable" || read.kind === "failed") return <Blank title="Your proactivity did not open" body="Try again in a moment." tone="danger" />;
  const view = read.view;
  const timezone = view.quietHours?.timezone ?? zone();
  const muted = mutedLine(view);

  const saveHours = () => {
    const hours = quietHoursOf(start, end, timezone);
    if (typeof hours === "string") return Alert.alert("Quiet hours", hours);
    change({ quietHours: hours }, quietText);
  };

  return (
    <Page onRefresh={() => setReads((count) => count + 1)}>
      <Footnote>How often Nexus writes first. It writes first only with a reason it can show.</Footnote>
      <Section title="Proactivity">
        {PROACTIVITY_DIAL.map((step) => (
          <Row
            key={step.level}
            label={step.label}
            detail={stepLine(step)}
            detailTone="tertiary"
            selected={view.level === step.level}
            disabled={working}
            onPress={() => view.level !== step.level && change({ level: step.level }, dialText)}
            accessibilityRole="radio"
            accessibilityState={{ checked: view.level === step.level, disabled: working }}
          />
        ))}
      </Section>
      <Section title="Quiet hours" footer={`Nothing proactive comes then; it waits. ${timezone}`}>
        <Row
          label="Hold everything but the urgent"
          accessory={
            <Switch
              value={view.quietHours !== null}
              disabled={working}
              accessibilityLabel="Quiet hours"
              onValueChange={(on) => (on ? saveHours() : change({ quietHours: null }, quietText))}
            />
          }
        />
        <Field value={start} onChangeText={setStart} placeholder="22:00" keyboardType="numbers-and-punctuation" maxLength={5} accessibilityLabel="Quiet from" />
        <Field value={end} onChangeText={setEnd} placeholder="07:00" keyboardType="numbers-and-punctuation" maxLength={5} accessibilityLabel="Quiet until" />
        <Row label={view.quietHours ? "Save" : "Turn on"} tone="primary" disabled={working} onPress={saveHours} />
      </Section>
      {muted ? (
        <Section footer={muted}>
          <Row label="Turn them back on" tone="primary" disabled={working} onPress={() => change({ muted: [] }, () => "Every kind is back on.")} />
        </Section>
      ) : null}
      <Footnote>{batchLine(view)}</Footnote>
    </Page>
  );
}
