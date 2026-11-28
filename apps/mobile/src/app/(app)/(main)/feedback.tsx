import { router } from "expo-router";
import { useEffect, useState } from "react";
import { StyleSheet } from "react-native";
import { useCore, useStored } from "@/auth";
import { COUNT_FROM, KINDS, MAX_CHARS, PLACEHOLDER, failure, feedbackBody, type Kind } from "@/feedback";
import { Busy, Field, Inset, Page, Row, Section, Segmented } from "@/list";
import { device } from "@/reporting";

/**
 * Send feedback, pushed from Account: a bug, an idea or anything else, to Nexus's feedback channel in Slack, as Nexus UI's
 * dialog and the desktop's menu send it (`POST /v1/diagnostics/feedback`). What the person typed is kept until it is
 * sent, so leaving the page or a failed send loses nothing.
 */
export default function SendFeedback() {
  const core = useCore();
  const kept = useStored<string>("draft", "feedback");
  const [kind, setKind] = useState<Kind>("bug");
  const [message, setMessage] = useState(() => kept.load() ?? "");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  const [problem, setProblem] = useState<string>();
  const [email, setEmail] = useState<string>();
  useEffect(() => {
    core.me().then((me) => setEmail(me.user?.email ?? undefined), () => undefined);
  }, [core]);

  const write = (next: string) => {
    setMessage(next);
    if (next.trim()) kept.save(next);
    else kept.remove();
  };
  const empty = !message.trim();
  const send = () => {
    if (busy || empty) return;
    setBusy(true);
    setProblem(undefined);
    core
      .sendFeedback(feedbackBody(kind, message, device))
      .then(() => {
        kept.remove();
        setMessage("");
        setSent(true);
      })
      .catch((error: unknown) => setProblem(failure(error)))
      .finally(() => setBusy(false));
  };

  if (sent) {
    return (
      <Page>
        <Section
          title="Thanks, it is sent"
          footer={`The Nexus team reads every message.${email ? ` If we need more, we will write to ${email}.` : ""}`}
        >
          <Row label="Done" tone="primary" center onPress={() => router.back()} />
        </Section>
      </Page>
    );
  }

  const count = message.length >= COUNT_FROM ? ` ${message.length.toLocaleString("en-US")} of ${MAX_CHARS.toLocaleString("en-US")}.` : "";
  return (
    <Page>
      <Section title="What is it?">
        <Inset>
          <Segmented label="What is it?" options={KINDS} value={kind} onChange={setKind} />
        </Inset>
      </Section>
      <Section
        footer={problem ?? `The team sees your name, email and organization, and the app's version.${count}`}
        footerTone={problem ? "danger" : undefined}
      >
        <Field
          value={message}
          onChangeText={write}
          placeholder={PLACEHOLDER[kind]}
          multiline
          maxLength={MAX_CHARS}
          editable={!busy}
          autoFocus
          textAlignVertical="top"
          style={styles.message}
          accessibilityLabel="Your feedback"
        />
      </Section>
      <Section>
        <Row label={busy ? "Sending…" : "Send"} tone="primary" center disabled={busy || empty} accessory={busy ? <Busy /> : undefined} onPress={send} />
      </Section>
    </Page>
  );
}

const styles = StyleSheet.create({
  message: { minHeight: 160, paddingTop: 12, paddingBottom: 12 },
});
