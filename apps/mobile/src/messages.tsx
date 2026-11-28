import type { ChatAttachmentRef, ChatSessionMessage } from "@nexus/kit";
import { useState } from "react";
import { Pressable, StyleSheet, Text, View } from "react-native";
import { ActivityChips } from "@/activity-chips";
import { isDataFile } from "@/chat-files";
import { sizeLabel, type ComposerImage } from "@/chat-images";
import { Icon } from "@/icon";
import { QuestionImages, type ImageSource } from "@/images";
import { Markdown } from "@/markdown";
import { MessageMenu, type MessageAction } from "@/message-menu";
import { messageMetaParts } from "@/meta";
import { nameOf, type Model } from "@/models";
import { ProviderMark } from "@/provider-mark";
import { readQuotedPrompt } from "@/quoted-prompt";
import { font, mono, text, type Theme } from "@/theme";

/** A chat's messages as the chat and an artifact's reader both draw them: the question in a bubble, the answer as prose. */

export type MessageLike = Pick<ChatSessionMessage, "id" | "role" | "content" | "status"> &
  Partial<Pick<ChatSessionMessage, "model" | "mode" | "costUsd" | "requestedModel" | "connectors" | "skills" | "sources" | "page">>;

export const failed = (m: Pick<ChatSessionMessage, "status">) => m.status === "failed" || m.status === "refused";

/** The question in its bubble, with its data files and its images above it (a question may be images alone). */
export function Question({ content, theme, attachments, previews, images, actions = [] }: {
  content: string;
  theme: Theme;
  attachments?: ChatAttachmentRef[];
  /** What was attached, shown while the question is sent. */
  previews?: ComposerImage[];
  /** Where its stored images are read. */
  images?: ImageSource;
  /** Its long-press menu (Copy, Restore to here, Retry, Share): with any, the bubble opens it rather than selecting its text. */
  actions?: MessageAction[];
}) {
  const { quotes, prompt } = readQuotedPrompt(content);
  return (
    <View style={styles.questionRow}>
      <QuestionFiles files={attachments?.filter(isDataFile) ?? []} theme={theme} />
      <QuestionImages attachments={attachments?.filter((attachment) => !isDataFile(attachment))} previews={previews} source={images} theme={theme} />
      {quotes.length ? (
        <MessageMenu actions={actions} style={styles.quotes}>
          <View style={styles.quotes}>
            {quotes.map((quote, index) => (
              <Text key={index} selectable={!actions.length} style={[styles.quote, { color: theme.muted, borderColor: theme.chartMuted }]}>{quote}</Text>
            ))}
          </View>
        </MessageMenu>
      ) : null}
      {prompt ? (
        <MessageMenu actions={actions} style={styles.bubbleRoom}>
          <View style={[styles.bubble, actions.length ? null : styles.bubbleRoom, { backgroundColor: theme.userBubble }]}>
            <Folded content={prompt} selectable={!actions.length} theme={theme} />
          </View>
        </MessageMenu>
      ) : null}
    </View>
  );
}

/** A question longer than this folds behind Show more (brand v5's P65·A). */
const FOLD = 7;

/**
 * A question's words, folded to seven lines when longer, with Show more under them. An unseen copy at the bubble's
 * width counts the lines, so the folded text is what shows from the first frame: nothing jumps when it folds.
 */
function Folded({ content, selectable, theme }: { content: string; selectable: boolean; theme: Theme }) {
  const [width, setWidth] = useState(0);
  const [long, setLong] = useState(false);
  const [open, setOpen] = useState(false);
  return (
    <>
      <Text
        selectable={selectable}
        numberOfLines={open ? undefined : FOLD}
        onLayout={(e) => setWidth(e.nativeEvent.layout.width)}
        style={[styles.body, { color: theme.foreground }]}
      >
        {content}
      </Text>
      {width ? (
        <Text
          style={[styles.body, styles.measure, { width }]}
          onTextLayout={(e) => setLong(e.nativeEvent.lines.length > FOLD)}
          accessibilityElementsHidden
          importantForAccessibility="no-hide-descendants"
          pointerEvents="none"
        >
          {content}
        </Text>
      ) : null}
      {long ? (
        <Pressable onPress={() => setOpen((o) => !o)} hitSlop={{ top: 8, bottom: 12, left: 8, right: 8 }} style={styles.more} accessibilityRole="button" accessibilityState={{ expanded: open }}>
          <Text style={[styles.moreText, { color: theme.muted }]}>{open ? "Show less" : "Show more"}</Text>
        </Pressable>
      ) : null}
    </>
  );
}

/** A question's data files, by name and size, as Nexus UI's `MessageFiles` lists them. */
function QuestionFiles({ files, theme }: { files: ChatAttachmentRef[]; theme: Theme }) {
  if (!files.length) return null;
  return (
    <View style={styles.files}>
      {files.map((file) => (
        <View
          key={file.id}
          accessible
          accessibilityLabel={`File: ${file.name ?? "File"}, ${sizeLabel(file.bytes)}`}
          style={[styles.file, { backgroundColor: theme.userBubble, borderColor: theme.border }]}
        >
          <Icon name="file" size={13} color={theme.muted} />
          <Text numberOfLines={1} ellipsizeMode="middle" style={[styles.fileName, { color: theme.foreground }]}>
            {file.name ?? "File"}
          </Text>
          <Text style={[styles.fileSize, { color: theme.muted }]}>{sizeLabel(file.bytes)}</Text>
        </View>
      ))}
    </View>
  );
}

/** A finished answer: its prose and the line under it. The chat adds Thinking and Retry around it. */
export function Answer({ message, models, theme }: { message: MessageLike; models: Model[]; theme: Theme }) {
  return (
    <View style={styles.answer}>
      {message.content ? <AnswerProse message={message} theme={theme} /> : null}
      <AnswerFoot message={message} models={models} theme={theme} />
    </View>
  );
}

export function AnswerProse({ message, theme }: { message: MessageLike; theme: Theme }) {
  return (
    <Markdown
      source={message.content}
      color={failed(message) ? theme.danger : undefined}
      linkHosts={!!message.connectors?.length}
    />
  );
}

/**
 * Under the prose: the calls the answer made (P38), then the model that ran and its cost, and
 * with `onMore` the answer's More button. The answer keeps its text selectable, so its menu is a button
 * rather than a long press.
 */
export function AnswerFoot({ message, models, theme, onMore }: {
  message: MessageLike;
  models: Model[];
  theme: Theme;
  onMore?: () => void;
}) {
  const meta = !!message.model || message.costUsd !== undefined;
  return (
    <>
      <ActivityChips message={message} theme={theme} />
      {meta || onMore ? (
        <View style={styles.metaRow}>
          {message.model ? <ProviderMark model={message.model} size={12} /> : null}
          {meta ? <Text style={[styles.meta, { color: theme.muted }]}>{messageMetaParts(message, (m) => nameOf(models, m)).join(" · ")}</Text> : null}
          {onMore ? (
            <Pressable onPress={onMore} style={styles.moreButton} accessibilityRole="button" accessibilityLabel="More for this answer" accessibilityHint="Copy, select text, share, read aloud or report this answer">
              <Icon name="more" size={16} color={theme.muted} />
            </Pressable>
          ) : null}
        </View>
      ) : null}
    </>
  );
}

export const messageStyles = StyleSheet.create({
  meta: { ...text.xs, fontFamily: mono },
  metaRow: { flexDirection: "row", alignItems: "center", gap: 6, marginTop: 6 },
  answer: { marginBottom: 20, paddingHorizontal: 4 },
});

const styles = StyleSheet.create({
  quotes: { alignSelf: "flex-end", maxWidth: "85%" },
  quote: { ...text.secondary, fontFamily: font.regular, paddingLeft: 10, marginVertical: 2, borderLeftWidth: 3 },
  ...messageStyles,
  meta: { ...messageStyles.meta, flexShrink: 1 },
  questionRow: { alignItems: "flex-end", marginBottom: 20 },
  bubble: { borderRadius: 20, paddingHorizontal: 14, paddingVertical: 10 },
  bubbleRoom: { maxWidth: "85%" },
  files: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: 6, marginBottom: 6, maxWidth: "85%" },
  file: { flexDirection: "row", alignItems: "center", gap: 6, maxWidth: 260, paddingHorizontal: 10, paddingVertical: 4, borderRadius: 999, borderWidth: StyleSheet.hairlineWidth },
  fileName: { ...text.xs, fontFamily: font.regular, flexShrink: 1 },
  fileSize: { ...text.xs, fontFamily: font.regular, fontVariant: ["tabular-nums"] },
  body: { ...text.reading, fontFamily: font.regular },
  measure: { position: "absolute", top: 0, left: 0, opacity: 0 },
  more: { alignSelf: "flex-start", marginTop: 6 },
  moreButton: { marginLeft: "auto", minWidth: 44, minHeight: 44, alignItems: "center", justifyContent: "center" },
  moreText: { ...text.sm, fontFamily: font.medium },
});
