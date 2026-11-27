import type { PageComment, PagePin } from "@nexus/kit";
import { useCallback, useEffect, useState } from "react";
import { Keyboard, Pressable, ScrollView, StyleSheet, Text, View, useWindowDimensions } from "react-native";
import { ago, spokenAgo } from "@/ago";
import { useArtifacts } from "@/auth";
import { TextInput } from "@/controls";
import type { DocCommentAnchor } from "@/doc-comment-anchor";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { initialsOf, openThreads, threadsOf, type Thread } from "@/library";
import { Sheet } from "@/sheet";
import { font, text, useTheme, type Theme } from "@/theme";
import { ACCESSIBILITY_FONT_SCALE, CHROME_FONT_SCALE, isAccessibilitySize } from "@/text-size";

/** A page's comments, read when the page opens and again after a change: the bar counts the open threads. */
export function usePageComments(pageId: string | undefined) {
  const artifacts = useArtifacts();
  const [comments, setComments] = useState<PageComment[]>([]);
  const [owner, setOwner] = useState(false);
  const [loadedPageId, setLoadedPageId] = useState<string>();
  const reload = useCallback(() => {
    if (!pageId) return Promise.resolve();
    return artifacts.pageComments(pageId).then(
      (r) => {
        setComments(r.comments);
        setOwner(r.owner);
        setLoadedPageId(pageId);
      },
      () => undefined,
    );
  }, [artifacts, pageId]);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { comments, owner, available: Boolean(pageId && loadedPageId === pageId), open: openThreads(comments), reload };
}

/** The keyboard's height while it is up, else 0. */
function useKeyboardHeight() {
  const [height, setHeight] = useState(0);
  useEffect(() => {
    const shown = Keyboard.addListener("keyboardDidShow", (e) => setHeight(e.endCoordinates.height));
    const hidden = Keyboard.addListener("keyboardDidHide", () => setHeight(0));
    return () => {
      shown.remove();
      hidden.remove();
    };
  }, []);
  return height;
}

/** Room for the sheet's title, the field and the status bar above the threads, which take what is left. */
const CHROME = 260;

/**
 * A page's comments in a sheet (brand v5's P20 Comment): each open thread with its passage on top, marked, then who
 * wrote what and when; Reply answers a thread, Resolve closes it (yours, or any when the page is yours). The field at
 * the foot adds a comment, or a reply once Reply is tapped. The phone never underlines in the page itself: the page
 * runs sandboxed, as in Nexus UI, so a passage is shown here, quoted.
 */
export function PageCommentsSheet({
  pageId,
  version,
  anchor,
  pinnedPoint,
  initialThread,
  onOpenVersion,
  onLocate,
  visible,
  comments,
  owner,
  writable = true,
  onChanged,
  onClose,
}: {
  pageId: string;
  /** The version on screen, which a new comment is about. */
  version?: number;
  anchor?: DocCommentAnchor;
  pinnedPoint?: PagePin;
  initialThread?: string;
  onOpenVersion?: (version: number) => void;
  onLocate?: (anchor: DocCommentAnchor) => void;
  visible: boolean;
  comments: PageComment[];
  owner: boolean;
  writable?: boolean;
  onChanged: () => Promise<void>;
  onClose: () => void;
}) {
  const artifacts = useArtifacts();
  const theme = useTheme();
  const { height, fontScale } = useWindowDimensions();
  // Half the screen for the threads; less while the keyboard is up, so the field stays above it.
  const keyboard = useKeyboardHeight();
  // At the accessibility text sizes the threads take more of the screen: half would show a comment or two.
  const share = isAccessibilitySize(fontScale) ? 0.62 : 0.5;
  const threadsHeight = keyboard ? Math.max(48, Math.min(height * share, height - keyboard - CHROME - 88)) : height * share;
  const [draft, setDraft] = useState("");
  const [replyTo, setReplyTo] = useState<Thread | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [filter, setFilter] = useState<"Open" | "Resolved" | "All">("Open");
  const [focused, setFocused] = useState<string>();
  useEffect(() => {
    if (!visible) return;
    setReplyTo(null);
    setFocused(initialThread);
    if (initialThread) setFilter("All");
    setProblem(null);
  }, [visible, initialThread]);
  const threads = threadsOf(comments);
  const shown = threads.filter((thread) => filter === "All" || (filter === "Resolved") === Boolean(thread.root.resolvedAt));
  const position = shown.findIndex((thread) => thread.root.id === focused);
  const pick = (thread: Thread) => {
    haptic("selection");
    setFocused(thread.root.id);
    const { quote, prefix, suffix } = thread.root;
    if (quote) onLocate?.({ quote, prefix, suffix });
  };

  /** One change, then the comments again; what went wrong stays in the sheet. */
  const run = (work: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    setProblem(null);
    work()
      .then(onChanged)
      .then(() => haptic("success"))
      .catch((e: Error) => { haptic("error"); setProblem(e.message); })
      .finally(() => setBusy(false));
  };
  const send = () => {
    const body = draft.trim();
    if (!writable || !body || busy) return;
    haptic("tap");
    run(async () => {
      await artifacts.createPageComment(pageId, { body, ...(replyTo ? { threadId: replyTo.root.id } : { version, ...anchor, pin: pinnedPoint }) });
      setDraft("");
      setReplyTo(null);
    });
  };
  const resolve = (thread: Thread, resolved: boolean) => run(() => artifacts.resolvePageComment(pageId, thread.root.id, resolved));

  return (
    // Solid: it opens over the page, whose white would show through glass.
    <Sheet visible={visible} onClose={onClose} solid>
      <Text style={[styles.title, { color: theme.foreground }]} accessibilityRole="header" maxFontSizeMultiplier={CHROME_FONT_SCALE}>
        Comments
      </Text>
      <View accessibilityRole="tablist" style={styles.filters}>
        {(["Open", "Resolved", "All"] as const).map((choice) => (
          <Pressable key={choice} accessibilityRole="button" accessibilityState={{ selected: filter === choice }}
            style={[styles.filter, filter === choice && { backgroundColor: theme.fill }]}
            onPress={() => { haptic("selection"); setFilter(choice); setFocused(undefined); }}>
            <Text style={[styles.action, { color: filter === choice ? theme.foreground : theme.muted }]}>{choice}</Text>
          </Pressable>
        ))}
      </View>
      {shown.length > 1 && <View style={styles.filters}>
        <Pressable accessibilityRole="button" accessibilityLabel="Previous thread" style={styles.filter}
          onPress={() => pick(shown[position < 0 ? shown.length - 1 : (position + shown.length - 1) % shown.length]!)}>
          <Text style={[styles.action, { color: theme.foreground }]}>Previous</Text>
        </Pressable>
        <Pressable accessibilityRole="button" accessibilityLabel="Show all threads" style={styles.filter} onPress={() => setFocused(undefined)}>
          <Text style={[styles.action, { color: theme.muted }]}>{position < 0 ? shown.length + " threads" : (position + 1) + " of " + shown.length}</Text>
        </Pressable>
        <Pressable accessibilityRole="button" accessibilityLabel="Next thread" style={styles.filter}
          onPress={() => pick(shown[(position + 1) % shown.length]!)}>
          <Text style={[styles.action, { color: theme.foreground }]}>Next</Text>
        </Pressable>
      </View>}
      <ScrollView style={{ maxHeight: threadsHeight }} contentContainerStyle={styles.threads} keyboardShouldPersistTaps="handled">
        {!shown.length && <Text style={[styles.note, { color: theme.muted }]}>No comments in this view.</Text>}
        {(position < 0 ? shown : [shown[position]!]).map((thread) => (
          <ThreadView
            key={thread.root.id}
            thread={thread}
            theme={theme}
            canResolve={writable && (owner || thread.root.mine)}
            replying={replyTo?.root.id === thread.root.id}
            onLocate={onLocate && thread.root.quote ? () => pick(thread) : undefined}
            onOpenVersion={onOpenVersion && thread.root.pin && version !== thread.root.version
              ? () => onOpenVersion(thread.root.version) : undefined}
            onReply={!writable || thread.root.resolvedAt ? undefined : () => { haptic("selection"); setReplyTo(thread); }}
            onResolve={thread.root.resolvedAt ? undefined : () => resolve(thread, true)}
            onReopen={thread.root.resolvedAt ? () => resolve(thread, false) : undefined}
          />
        ))}
      </ScrollView>
      {pinnedPoint && !replyTo && <Text style={[styles.note, { color: theme.muted }]}>Pin on v{version}</Text>}
      {anchor && !replyTo && <Text style={[styles.quote, { color: theme.muted, borderLeftColor: theme.mark }]} numberOfLines={2}>{anchor.quote}</Text>}
      {problem ? <Text style={[styles.note, { color: theme.danger }]}>{problem}</Text> : null}
      {writable && (
        <View style={[styles.field, { backgroundColor: theme.fill }]}>
          <TextInput
            style={[styles.input, { color: theme.foreground }]}
            value={draft}
            onChangeText={setDraft}
            placeholder={replyTo ? `Reply to ${replyTo.root.author ?? "the thread"}` : "Add a comment"}
            placeholderTextColor={theme.muted}
            multiline
            // The field stops at the first accessibility size: past it, it would crowd out the threads it answers.
            maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}
            accessibilityLabel={replyTo ? "Reply" : "Comment"}
          />
          <Pressable
            onPress={send}
            disabled={!draft.trim() || busy}
            style={[styles.send, { backgroundColor: theme.primary }, (!draft.trim() || busy) && styles.idle]}
            hitSlop={6}
            accessibilityRole="button"
            accessibilityLabel="Send"
          >
            <Icon name="send" size={15} color={theme.onPrimary} />
          </Pressable>
        </View>
      )}
    </Sheet>
  );
}

/** A thread: its passage, marked; each comment with who and when; Reply and Resolve, or Reopen when resolved. */
function ThreadView({
  thread,
  theme,
  canResolve,
  replying,
  onReply,
  onResolve,
  onReopen,
  onLocate,
  onOpenVersion,
}: {
  thread: Thread;
  theme: Theme;
  canResolve: boolean;
  replying?: boolean;
  onReply?: () => void;
  onResolve?: () => void;
  onReopen?: () => void;
  onOpenVersion?: () => void;
  onLocate?: () => void;
}) {
  return (
    <View style={[styles.thread, thread.root.resolvedAt ? styles.faded : null]}>
      {thread.root.quote ? (
        <Text style={[styles.quote, { color: theme.muted, borderLeftColor: theme.mark }]} numberOfLines={3}>
          {thread.root.quote}
        </Text>
      ) : null}
      {[thread.root, ...thread.replies].map((c) => (
        <View key={c.id} style={styles.comment} accessible accessibilityLabel={`${c.mine ? "You" : (c.author ?? "Someone")}, ${spokenAgo(c.createdAt)}: ${c.body}`}>
          <View style={styles.byline}>
            <View style={[styles.avatar, { backgroundColor: theme.fill }]}>
              <Text style={[styles.avatarText, { color: theme.foreground }]} allowFontScaling={false}>
                {initialsOf(c.author)}
              </Text>
            </View>
            <Text style={[styles.author, { color: theme.foreground }]} numberOfLines={1}>
              {c.mine ? "You" : (c.author ?? "Someone")}
            </Text>
            <Text style={[styles.when, { color: theme.muted }]}>{ago(c.createdAt)}</Text>
          </View>
          <Text style={[styles.body, { color: theme.foreground }]}>{c.body}</Text>
        </View>
      ))}
      <View style={styles.actions}>
        {onOpenVersion && (
          <Pressable onPress={onOpenVersion} style={styles.threadAction} accessibilityRole="button">
            <Text style={[styles.action, { color: theme.muted }]}>Made on v{thread.root.version}</Text>
          </Pressable>
        )}
        {onLocate && (
          <Pressable onPress={onLocate} style={styles.threadAction} accessibilityRole="button">
            <Text style={[styles.action, { color: theme.muted }]}>Show passage</Text>
          </Pressable>
        )}
        {onReply ? (
          <Pressable onPress={onReply} style={styles.threadAction} accessibilityRole="button" accessibilityState={{ selected: !!replying }}>
            <Text style={[styles.action, { color: replying ? theme.foreground : theme.muted }]}>Reply</Text>
          </Pressable>
        ) : null}
        {canResolve && onResolve ? (
          <Pressable onPress={onResolve} style={styles.threadAction} accessibilityRole="button">
            <Text style={[styles.action, { color: theme.muted }]}>Resolve</Text>
          </Pressable>
        ) : null}
        {canResolve && onReopen ? (
          <Pressable onPress={onReopen} style={styles.threadAction} accessibilityRole="button">
            <Text style={[styles.action, { color: theme.muted }]}>Reopen</Text>
          </Pressable>
        ) : null}
      </View>
    </View>
  );
}

// The board's sheet (P20 Comment): 20 pt in; the passage 15/22 beside a 2 pt mark (text-2, not the board's text-3,
// which the glass over a white page washes out); a comment's byline 15 pt
// with a 22 pt avatar, its words 17/24 under it, 30 pt in; the field a 44 pt pill with a 32 pt send.
const styles = StyleSheet.create({
  filters: { flexDirection: "row", gap: 4, paddingHorizontal: 12 },
  filter: { flex: 1, minHeight: 44, justifyContent: "center", alignItems: "center", borderRadius: 12, paddingHorizontal: 6 },
  threadAction: { minHeight: 44, justifyContent: "center", paddingHorizontal: 6 },
  title: { fontFamily: font.medium, fontSize: 20, lineHeight: 25, paddingHorizontal: 20, paddingTop: 8, paddingBottom: 4 },
  threads: { paddingHorizontal: 20, paddingVertical: 8, gap: 20 },
  thread: { gap: 12 },
  faded: { opacity: 0.6 },
  quote: { ...text.secondary, fontFamily: font.regular, lineHeight: 22, paddingLeft: 12, borderLeftWidth: 2 },
  comment: { gap: 6 },
  byline: { flexDirection: "row", alignItems: "center", gap: 8 },
  avatar: { width: 22, height: 22, borderRadius: 11, alignItems: "center", justifyContent: "center" },
  avatarText: { fontFamily: font.medium, fontSize: 9 },
  author: { ...text.secondary, fontFamily: font.medium, flexShrink: 1 },
  when: { ...text.secondary, fontFamily: font.regular },
  body: { fontFamily: font.regular, fontSize: 17, lineHeight: 24, paddingLeft: 30 },
  actions: { flexDirection: "row", flexWrap: "wrap", columnGap: 20, rowGap: 6, paddingLeft: 30 },
  action: { ...text.sm, fontFamily: font.medium },
  resolvedToggle: { minHeight: 44, justifyContent: "center" },
  note: { ...text.sm, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 6 },
  field: { flexDirection: "row", alignItems: "flex-end", gap: 8, marginHorizontal: 16, marginTop: 8, marginBottom: 8, minHeight: 44, paddingLeft: 16, paddingRight: 6, paddingVertical: 6, borderRadius: 22 },
  input: { ...text.body, fontFamily: font.regular, flex: 1, maxHeight: 110, paddingVertical: 4 },
  send: { width: 32, height: 32, borderRadius: 16, alignItems: "center", justifyContent: "center" },
  idle: { opacity: 0.3 },
});
