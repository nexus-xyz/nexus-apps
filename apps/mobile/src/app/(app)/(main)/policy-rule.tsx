import { useLocalSearchParams } from "expo-router";
import { Footnote, Page, Section } from "@/list";
import { AdminHead, Fact } from "@/people-controls";
import { shortDay } from "@/people-controls-words";

/**
 * One rule, full screen (P71a·H's A rule, P105·D's An item): what it says, whom it applies to, where it comes from and
 * when it last changed, as Policy hands them over. Left out: the design's Set by, since the Gateway keeps when a policy
 * changed but not who changed it, Why it applies and History, which it does not answer per rule, and Change: rules
 * change on a computer.
 */
export default function PolicyRule() {
  const { title, text, scope, from, at } = useLocalSearchParams<{ title: string; text: string; scope: string; from: "here" | "gateway"; at?: string }>();
  return (
    <Page>
      <AdminHead title={title || "Rule"} />
      <Section>
        <Fact label="Rule" value={text ?? ""} />
        {scope ? <Fact label="Applies to" value={scope} /> : null}
        <Fact label="Comes from" value={from === "gateway" ? "Your Gateway's own rules, above every organization" : "Set here"} />
        {at ? <Fact label="Changed" value={shortDay(at)} /> : null}
      </Section>
      <Footnote>Change rules on a computer.</Footnote>
    </Page>
  );
}
