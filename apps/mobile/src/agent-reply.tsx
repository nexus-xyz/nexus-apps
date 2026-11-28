import { answerTexts, safeUrl, type AgentErrand, type AgentMessage } from "@nexus/kit";
import type { ReactNode } from "react";
import { Linking, Pressable, StyleSheet, Text, View } from "react-native";
import { answerLine, sourcesOf } from "@/agent-chat";
import { ApprovalCard, type DecideWrite, type EditWrite } from "@/approval-card";
import { connectCards, errandCard, OPENED_ERRAND, sessionCards } from "@/assistant-cards";
import { ConnectCard } from "@/connect-card";
import { Markdown, open } from "@/markdown";
import { messageStyles } from "@/messages";
import { ProviderMark } from "@/provider-mark";
import { TextCard } from "@/text-card";
import { font, mono, text, type Theme } from "@/theme";
import { ErrandCard, SessionCard } from "@/thread-cards";
import { useAssistantName } from "@/use-assistant-name";
import { useWebBilling } from "@/use-storefront";
import { messageConsent } from "@/write-consent";
import { WriteConsentPrompt } from "@/write-consent-prompt";

/**
 * One message of Nexus's reply (S2.5), as Nexus UI's `AgentReplyView` draws it (S2.4): an answer with its sources
 * and the line under it, a refusal, a notice, the notes it keeps, and a write that waits for you. An answer that needed
 * an account the person has not connected ends with a Connect card for each (A6·e, `connect-card.tsx`). A write that
 * waits is the approval card (A9, `approval-card.tsx`): with `decide`, the person approves or declines it in the
 * thread; without, its link opens Nexus UI's approval page in the browser. `line: false` leaves out the line under an
 * answer, for an answer read back (`entriesOf`). The Assistant's thread draws an answer as a session's prose, full
 * width with no bubble (`assistant-bubbles.tsx`). An answer's sessions end it with their link cards, and an "I opened
 * an errand" notice is the errand's link card (A1·5, `thread-cards.tsx`). A text Nexus prepared is its text card
 * (`text-card.tsx`). `errandNow` gives the errand as the list has it now. A refusal of a write that needs the person's
 * consent at the service ends with the write consent prompt.
 */
export function AgentReply({
  message,
  name,
  theme,
  line = true,
  decide,
  edit,
  errandNow,
}: {
  message: AgentMessage;
  name: (model: string) => string;
  theme: Theme;
  line?: boolean;
  decide?: DecideWrite;
  edit?: EditWrite;
  errandNow?: (id: string) => AgentErrand | undefined;
}) {
  const assistant = useAssistantName();
  switch (message.kind) {
    case "answer": {
      const sources = sourcesOf(message);
      const connects = [
        ...connectCards(message, assistant).map((view) => <ConnectCard key={view.connector} view={view} theme={theme} />),
        ...sessionCards(message).map((card) => <SessionCard key={`${card.title}:${card.url ?? card.line}`} card={card} theme={theme} />),
        // A text Nexus prepared: the person checks it and sends it from Messages.
        ...answerTexts(message).map((card) => <TextCard key={`${card.to}:${card.text}`} card={card} theme={theme} />),
      ];
      return (
        <View style={messageStyles.answer}>
          <Markdown source={message.text} linkHosts />
          {sources.length ? <Sources sources={sources} theme={theme} /> : null}
          {message.aiNotice ? <Text style={[styles.quiet, { color: theme.muted }]}>AI can make mistakes. Check what matters.</Text> : null}
          {line ? (
            <View style={messageStyles.metaRow}>
              {message.model ? <ProviderMark model={message.model} size={12} /> : null}
              <Text style={[messageStyles.meta, { color: theme.muted }]}>{answerLine(message, name).join(" · ")}</Text>
            </View>
          ) : null}
          {connects.length ? <View style={styles.cards}>{connects}</View> : null}
        </View>
      );
    }
    case "refused": {
      // A write that needs the person's consent at the service first: the prompt under the refusal.
      const consent = messageConsent(message);
      const refused = <Refused detail={message.detail} billingUrl={message.billingUrl} theme={theme} />;
      if (!consent) return refused;
      return (
        <View>
          {refused}
          <WriteConsentPrompt consent={consent} theme={theme} />
        </View>
      );
    }
    case "pending_action":
      return <ApprovalCard message={message} theme={theme} decide={decide} edit={edit} />;
    case "notice": {
      // The agent opened an errand for a bigger job (A1·5): the board's line, then the errand's link card.
      const card = errandCard(message, errandNow);
      if (card) {
        return (
          <View style={[messageStyles.answer, styles.cards]}>
            <Text style={[styles.body, { color: theme.foreground }]}>{OPENED_ERRAND}</Text>
            <ErrandCard card={card} theme={theme} />
          </View>
        );
      }
      return (
        <Note theme={theme}>
          <Text style={[styles.body, { color: theme.foreground }]}>{message.text}</Text>
        </Note>
      );
    }
    case "insight": {
      const url = safeUrl(message.url);
      return (
        <Note theme={theme}>
          <Text style={[styles.body, { color: theme.foreground }]}>{message.text}</Text>
          {url ? <LinkText label="Open it" url={url} theme={theme} /> : null}
        </Note>
      );
    }
    case "memories":
      return (
        <Note theme={theme}>
          {!message.enabled ? (
            <Text style={[styles.body, { color: theme.foreground }]}>Notes are off: {assistant} keeps nothing about you.</Text>
          ) : message.notes.length ? (
            <>
              <Text style={[styles.body, { color: theme.foreground }]}>What {assistant} keeps about you:</Text>
              {message.notes.map((note) => (
                <Text key={`${note.createdAt}:${note.text}`} style={[styles.body, { color: theme.foreground }]}>
                  {"• "}
                  {note.text}
                </Text>
              ))}
            </>
          ) : (
            <Text style={[styles.body, { color: theme.foreground }]}>{assistant} keeps no notes about you yet.</Text>
          )}
        </Note>
      );
    case "forgotten":
      return (
        <Note theme={theme}>
          <Text style={[styles.body, { color: theme.foreground }]}>
            {assistant} forgot {message.count} {message.count === 1 ? "note" : "notes"}.
          </Text>
        </Note>
      );
    case "error":
      return (
        <Text style={[styles.body, styles.error, { color: theme.danger }]} accessibilityRole="alert">
          {message.cut ? "The answer was cut off before it finished." : `${assistant} could not finish this answer.`}
          {message.requestId ? ` Request ${message.requestId}.` : ""}
        </Text>
      );
    default:
      return null;
  }
}

/** An answer's sources, numbered, each opening in the browser. */
function Sources({ sources, theme }: { sources: { title: string; url: string }[]; theme: Theme }) {
  return (
    <View style={[styles.sources, { borderColor: theme.border }]} accessibilityLabel="Sources">
      <Text style={[styles.label, { color: theme.muted }]}>Sources</Text>
      {sources.map((source, i) => (
        <Pressable key={source.url} style={styles.source} onPress={() => open(source.url)} accessibilityRole="link" accessibilityHint="Opens it in the browser">
          <Text style={[styles.sourceN, { color: theme.muted }]}>{i + 1}</Text>
          <Text style={[styles.sourceTitle, { color: theme.primary }]} numberOfLines={2}>
            {source.title}
          </Text>
        </Pressable>
      ))}
    </View>
  );
}

/** A reply that is not an answer: on a card, with its label above it. */
function Note({ theme, label, children }: { theme: Theme; label?: string; children: ReactNode }) {
  return (
    <View style={[styles.note, { backgroundColor: theme.card, borderColor: theme.border }]} accessibilityRole="summary">
      {label ? <Text style={[styles.label, { color: theme.muted }]}>{label}</Text> : null}
      {children}
    </View>
  );
}

/**
 * A refusal, in the agent's words. Its billing link shows only where the store allows a link to a purchase outside the
 * app (`storefront.ts`), and opens in Safari, outside the app. Anywhere else the words stay, with no link.
 */
function Refused({ detail, billingUrl, theme }: { detail: string; billingUrl?: string | null; theme: Theme }) {
  const web = useWebBilling();
  const billing = web ? safeUrl(billingUrl) : undefined;
  return (
    <Note theme={theme} label="Refused">
      <Text style={[styles.body, { color: theme.foreground }]}>{detail}</Text>
      {billing ? (
        <Pressable onPress={() => void Linking.openURL(billing).catch(() => undefined)} accessibilityRole="link" accessibilityHint="Opens Nexus on the web in Safari" hitSlop={8}>
          <Text style={[styles.body, styles.underline, { color: theme.primary }]}>Open billing on the web</Text>
        </Pressable>
      ) : null}
    </Note>
  );
}

function LinkText({ label, url, theme }: { label: string; url: string; theme: Theme }) {
  return (
    <Pressable onPress={() => open(url)} accessibilityRole="link" hitSlop={8}>
      <Text style={[styles.body, styles.underline, { color: theme.primary }]}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  body: { ...text.base, fontFamily: font.regular },
  quiet: { ...text.sm, fontFamily: font.regular },
  code: { ...text.xs, fontFamily: mono },
  label: { ...text.xs, fontFamily: font.medium },
  underline: { textDecorationLine: "underline" },
  error: { marginBottom: 20, paddingHorizontal: 4 },
  note: { marginBottom: 20, padding: 14, gap: 6, borderRadius: 16, borderCurve: "continuous", borderWidth: StyleSheet.hairlineWidth },
  sources: { marginTop: 12, borderTopWidth: StyleSheet.hairlineWidth, paddingTop: 10, gap: 6 },
  source: { flexDirection: "row", gap: 10, minHeight: 36, alignItems: "center" },
  sourceN: { ...text.xs, fontFamily: font.medium, minWidth: 14, fontVariant: ["tabular-nums"] },
  sourceTitle: { ...text.sm, fontFamily: font.medium, flex: 1, textDecorationLine: "underline" },
  cards: { marginTop: 6, gap: 6 },
});
