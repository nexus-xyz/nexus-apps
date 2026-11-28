import type { ArtifactDetail, PageFrame, PagePin } from "@nexus/kit";
import { useState, type ComponentProps, type ReactNode } from "react";
import { haptic } from "@/haptics";
import type { Follow } from "@/live-page";
import { PageScreen } from "@/page-bar";
import { PageCommentsSheet, usePageComments } from "@/page-comments";
import { PageView } from "@/page-view";
import type { SitePinEvent } from "@/site-pins";

export function SharedSiteReader({ artifact, external, line, load, follow, onShare, onExport, viewControls, children }: {
  artifact: ArtifactDetail;
  external: boolean;
  line: string;
  load: () => Promise<PageFrame>;
  follow?: Follow;
  onShare: () => void;
  onExport?: () => void;
  viewControls?: Pick<ComponentProps<typeof PageView>, "onDeckInfo" | "onDesignOutline" | "designFocus">;
  children: ReactNode;
}) {
  const pageId = external ? undefined : artifact.pageId;
  const comments = usePageComments(pageId);
  const writable = comments.available && (comments.owner || artifact.pageRole === "edit");
  const [version, setVersion] = useState<number>();
  const [pin, setPin] = useState<PagePin>();
  const [thread, setThread] = useState<string>();
  const [showComments, setShowComments] = useState(false);
  const onSitePinEvent = (event: SitePinEvent, frameVersion: number) => {
    if (event.type === "ready") {
      setVersion(event.supported ? frameVersion : undefined);
      return;
    }
    setPin(event.type === "draft" ? event.pin : undefined);
    setThread(event.type === "open" ? event.id : undefined);
    setShowComments(true);
    haptic("tap");
  };
  return (
    <PageScreen title={artifact.title || "Untitled"} line={line} onShare={onShare} onMore={onExport} moreLabel={onExport ? "Export" : undefined} comments={comments.open}
      onComments={comments.available && version !== undefined ? () => {
        setPin(undefined);
        setThread(undefined);
        setShowComments(true);
      } : undefined}>
      {children}
      <PageView {...viewControls} load={load} follow={follow} sitePins={pageId
        ? { comments: comments.comments, writable, onEvent: onSitePinEvent }
        : undefined} />
      {pageId && comments.available && version !== undefined && (
        <PageCommentsSheet pageId={pageId} version={version} visible={showComments} comments={comments.comments}
          owner={comments.owner} writable={writable} initialThread={thread} pinnedPoint={pin}
          onChanged={comments.reload} onClose={() => setShowComments(false)} />
      )}
    </PageScreen>
  );
}
