import type { ArtifactDetail, PageFrame } from "@nexus/kit";
import { useState, type ReactNode } from "react";
import { Pressable, StyleSheet, Text } from "react-native";
import type { DocCommentAnchor } from "@/doc-comment-anchor";
import type { DocOutline } from "@/doc-contents";
import { DocContentsSheet } from "@/doc-contents-sheet";
import { haptic } from "@/haptics";
import type { Follow } from "@/live-page";
import { DocScreen } from "@/page-bar";
import { PageCommentsSheet, usePageComments } from "@/page-comments";
import { PageView } from "@/page-view";
import { font, text, useTheme } from "@/theme";

export function SharedDocReader({ artifact, external, line, load, follow, onShare, children }: {
  artifact: ArtifactDetail;
  external: boolean;
  line: string;
  load: () => Promise<PageFrame>;
  follow?: Follow;
  onShare: () => void;
  children: ReactNode;
}) {
  const theme = useTheme();
  const pageId = external ? undefined : artifact.pageId;
  const comments = usePageComments(pageId);
  const writable = comments.available && (comments.owner || artifact.pageRole === "edit");
  const [outline, setOutline] = useState<DocOutline>();
  const [heading, setHeading] = useState<{ index: number }>();
  const [showContents, setShowContents] = useState(false);
  const [showComments, setShowComments] = useState(false);
  const [selection, setSelection] = useState<DocCommentAnchor>();
  const [anchor, setAnchor] = useState<DocCommentAnchor>();
  const [focus, setFocus] = useState<DocCommentAnchor>();

  return (
    <DocScreen
      title={artifact.title || "Untitled"}
      line={line}
      onShare={onShare}
      onContents={outline ? () => setShowContents(true) : undefined}
      onComments={comments.available ? () => { setAnchor(undefined); setShowComments(true); } : undefined}
      comments={comments.open}
    >
      {children}
      {writable && selection && (
        <Pressable accessibilityRole="button" style={({ pressed }) => [styles.selection, pressed && { backgroundColor: theme.fill }]}
          onPress={() => { haptic("tap"); setAnchor(selection); setSelection(undefined); setShowComments(true); }}>
          <Text style={[styles.label, { color: theme.foreground }]}>Comment on selection</Text>
        </Pressable>
      )}
      <PageView load={load} follow={follow} onDocOutline={setOutline} docHeading={heading}
        onCommentAnchor={writable ? setSelection : undefined} commentFocus={focus} />
      <DocContentsSheet outline={outline} visible={showContents} onPick={(index) => setHeading({ index })}
        onClose={() => setShowContents(false)} />
      {pageId && comments.available && (
        <PageCommentsSheet pageId={pageId} version={artifact.pageVersion} visible={showComments} comments={comments.comments}
          owner={comments.owner} writable={writable} anchor={anchor} onLocate={(passage) => setFocus({ ...passage })}
          onChanged={comments.reload} onClose={() => setShowComments(false)} />
      )}
    </DocScreen>
  );
}

const styles = StyleSheet.create({
  selection: { minHeight: 44, justifyContent: "center", paddingHorizontal: 16 },
  label: { ...text.body, fontFamily: font.regular },
});
