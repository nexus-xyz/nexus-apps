import type { PageDetail, PagePin } from "@nexus/kit";
import { router, useFocusEffect, useLocalSearchParams } from "expo-router";
import { useCallback, useRef, useState } from "react";
import { ActivityIndicator, Alert, Pressable, ScrollView, StyleSheet, Text } from "react-native";
import { useArtifacts } from "@/auth";
import { transientError } from "@/connection-errors";
import { DeckExportSheet } from "@/deck-export-sheet";
import type { DeckInfo } from "@/deck-export";
import { DesignControls } from "@/design-controls";
import type { DesignFocus, DesignOutline } from "@/design-outline";
import type { DocCommentAnchor } from "@/doc-comment-anchor";
import { DocExportSheet } from "@/doc-export-sheet";
import type { DocOutline } from "@/doc-contents";
import { haptic } from "@/haptics";
import { DocContentsSheet } from "@/doc-contents-sheet";
import { kindOf, provenance } from "@/library";
import type { Follow } from "@/live-page";
import { DocScreen, PageScreen } from "@/page-bar";
import { PageCommentsSheet, usePageComments } from "@/page-comments";
import { PageView } from "@/page-view";
import { formatLabel } from "@/pages";
import { ShareSheet, type ShareActions } from "@/share-sheet";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { SheetCellControl, useSheetCellNavigation } from "@/sheet-cell-control";
import { isSiteFormat } from "@/site-version";
import { useSiteVersion } from "@/use-site-version";
import { siteMarkers, type SitePinEvent } from "@/site-pins";
import { font, text, useTheme } from "@/theme";

/**
 * One of your pages (Nexus UI's `/pages/<id>`), as brand v5's P20 draws it. A doc (Markdown) has its bar on top with
 * its provenance under it, and More lists its versions. A site (HTML or React) owns the screen with its bar at the
 * foot, and More opens its overview (`site/[id]`), where its deploys are. Both have Comments and Share. `?version=`
 * opens an earlier version, as a deploy does. Editing and Delete are in Nexus UI. A page shared with you opens as its
 * artifact (`artifact/[id]`).
 */
export default function PageReader() {
  const params = useLocalSearchParams<{ id: string; version?: string }>();
  const id = params.id;
  const artifacts = useArtifacts();
  const theme = useTheme();
  const [loaded, setPage] = useState<PageDetail | null>(null);
  const page = loaded?.id === id ? loaded : null;
  const lastSite = useRef<string | undefined>(undefined);
  const [version, setVersion] = useState<number | "latest">(params.version ? Number(params.version) : "latest");
  const [choosing, setChoosing] = useState(false);
  const [commenting, setCommenting] = useState(false);
  const [selection, setSelection] = useState<DocCommentAnchor>();
  const [commentAnchor, setCommentAnchor] = useState<DocCommentAnchor>();
  const [pinThread, setPinThread] = useState<string>();
  const [pinDraft, setPinDraft] = useState<{ pin: PagePin; version: number }>();
  const [siteFrame, setSiteFrame] = useState<{ version: number; saved: boolean }>();
  const [commentFocus, setCommentFocus] = useState<DocCommentAnchor>();
  const [showContents, setShowContents] = useState(false);
  const [docMenu, setDocMenu] = useState(false);
  const [deck, setDeck] = useState<DeckInfo>();
  const [design, setDesign] = useState<DesignOutline>();
  const [designFocus, setDesignFocus] = useState<DesignFocus>();
  const [exporting, setExporting] = useState(false);
  const cells = useSheetCellNavigation();
  const [outline, setOutline] = useState<DocOutline>();
  const [docHeading, setDocHeading] = useState<{ index: number }>();
  const [sheet, setSheet] = useState<ShareActions | null>(null);
  const [error, setError] = useState<string | null>(id ? null : "That link has no page in it.");
  const comments = usePageComments(id);

  // Read again on coming back: a rollback from the site's deploys writes a new version.
  useFocusEffect(
    useCallback(() => {
      if (!id) return;
      let active = true;
      artifacts.page(id).then(
        (detail) => {
          if (!active) return;
          lastSite.current = isSiteFormat(detail.format) ? detail.id : undefined;
          setPage(detail);
          setError(null);
        },
        (e: Error) => {
          if (active && !(lastSite.current === id && transientError(e))) setError(e.message);
        },
      );
      return () => { active = false; };
    }, [artifacts, id]),
  );

  const kind = kindOf(page?.format);
  const isSite = isSiteFormat(page?.format);
  const follow = useCallback<Follow>((onEvent, signal) => artifacts.pageEvents(id, onEvent, signal), [artifacts, id]);
  const site = useSiteVersion(id, isSite && !error && version === "latest" ? follow : undefined);
  const saved = page && isSite ? Math.max(page.latestVersion, site?.version ?? 0) : page?.latestVersion;
  const selectedFrame = version !== "latest" ? version : isSite ? (saved ?? "latest") : "live";
  const load = useCallback(() => {
    const siteVersion = isSite && typeof selectedFrame === "number" ? selectedFrame : undefined;
    return artifacts.framePageVersion(id, selectedFrame, siteVersion);
  }, [artifacts, id, isSite, selectedFrame]);
  const pick = (next: number | "latest") => {
    setOutline(undefined);
    setDeck(undefined);
    setDesign(undefined);
    setDesignFocus(undefined);
    setDocHeading(undefined);
    cells.reset();
    setSelection(undefined);
    setCommentAnchor(undefined);
    setCommentFocus(undefined);
    setSiteFrame(undefined);
    setPinThread(undefined);
    setPinDraft(undefined);
    setVersion(next);
    setChoosing(false);
  };

  const shown = version === "latest" ? saved : version;
  const entry = page?.versions.find((v) => v.version === shown);
  const siteDraft = isSite && !error && version === "latest" && site?.draft === true && site.version === shown;
  const older = version === "latest" || !page ? "" : ` of ${page.latestVersion}`;
  const share = page && id ? () => setSheet({ title: page.title || "This page", shareable: { kind: "page", id } }) : undefined;
  const openComments = () => {
    setCommentAnchor(undefined);
    setPinThread(undefined);
    setPinDraft(undefined);
    setCommenting(true);
  };
  const onSitePinEvent = (event: SitePinEvent, frameVersion: number, saved: boolean) => {
    if (event.type === "ready") {
      setSiteFrame(event.supported ? { version: frameVersion, saved } : undefined);
      return;
    }
    openComments();
    haptic("tap");
    if (event.type === "open") setPinThread(event.id);
    if (event.type === "draft") setPinDraft({ pin: event.pin, version: frameVersion });
  };
  const bar = {
    title: page?.title || (kind === "doc" ? "Doc" : "Page"),
    onShare: share,
    onComments: page ? openComments : undefined,
    comments: comments.open,
  };
  const body = error ? <Text style={[styles.note, { color: theme.danger }]}>{error}</Text> : id && page ? (
    <PageView load={load} follow={isSite ? undefined : follow} onDocOutline={kind === "doc" ? setOutline : undefined} docHeading={docHeading}
      onSheetSelection={kind === "sheet" ? cells.receive : undefined} cellRange={cells.range}
      onDeckInfo={kind === "site" ? setDeck : undefined}
      onDesignOutline={kind === "site" ? setDesign : undefined} designFocus={designFocus}
      onCommentAnchor={kind === "doc" ? setSelection : undefined} commentFocus={commentFocus}
      sitePins={kind === "site" ? { comments: comments.comments, writable: comments.owner, onEvent: onSitePinEvent } : undefined} />
  ) : id ? <ActivityIndicator style={styles.note} color={theme.muted} /> : null;
  const commentsSheet = id ? (
    <PageCommentsSheet
      pageId={id}
      version={pinDraft?.version ?? siteFrame?.version ?? shown}
      anchor={commentAnchor}
      initialThread={pinThread}
      pinnedPoint={pinDraft?.pin}
      onOpenVersion={siteFrame ? (target) => { setCommenting(false); pick(target); } : undefined}
      onLocate={kind === "doc" ? (anchor) => setCommentFocus({ ...anchor }) : undefined}
      visible={commenting}
      comments={comments.comments}
      owner={comments.owner}
      onChanged={comments.reload}
      onClose={() => setCommenting(false)}
    />
  ) : null;
  // Solid sheets over the page: its white would show through glass (`sheet.tsx`).
  const shareSheet = <ShareSheet actions={sheet} onClose={() => setSheet(null)} onError={(message) => Alert.alert("Couldn't do that", message)} solid />;

  // A board shows full-bleed as a site does: the host draws it, with pan and zoom.
  if (page && (kind === "site" || kind === "board")) {
    return (
      <PageScreen
        {...bar}
        line={[deck ? "Slides" : design ? "Design" : formatLabel(page.format), `v${shown}${older}`].join(" · ")}
        onMore={deck
          ? () => { haptic("tap"); setExporting(true); }
          : () => router.push({ pathname: "/site/[id]", params: { id } })}
        moreLabel={deck ? "Export" : kind === "board" ? "Board" : "Site"}
      >
        {siteDraft && (
          <Text style={[styles.siteDraft, { backgroundColor: theme.fill, color: theme.foreground }]} accessibilityLiveRegion="polite">
            {`A draft is available. You’re viewing saved v${shown}.`}
          </Text>
        )}
        {design && <DesignControls outline={design} setFocus={setDesignFocus} pageId={id} version={shown} />}
        {siteFrame && comments.owner && (siteFrame.saved ? (
          siteMarkers(comments.comments, siteFrame.version).length > 0 ? null :
          <Text style={[styles.siteHint, { color: theme.muted }]}>Press and hold a spot to comment.</Text>
        ) : (
          <Pressable accessibilityRole="button" style={styles.heading} onPress={() => { haptic("tap"); pick(siteFrame.version); }}>
            <Text style={[styles.headingText, { color: theme.foreground }]}>Open saved version to pin a comment</Text>
          </Pressable>
        ))}
        {body}
        {deck && <DeckExportSheet pageId={id} version={shown} deck={deck} visible={exporting} onClose={() => setExporting(false)} />}
        {commentsSheet}
        {shareSheet}
      </PageScreen>
    );
  }

  return (
    <DocScreen
      {...bar}
      line={page && shown ? provenance(kind, shown, entry?.createdVia, entry?.restoredFrom) + older : undefined}
      onMore={page && (kind === "doc" || page.versions.length > 1) ? () => { haptic("tap"); setDocMenu(true); } : undefined}
      onContents={outline ? () => setShowContents(true) : undefined}
    >
      {kind === "sheet" && cells.selection && <SheetCellControl selection={cells.selection} result={cells.result} onRange={cells.setRange} />}
      {kind === "doc" && selection && (
        <Pressable accessibilityRole="button" style={({ pressed }) => [styles.heading, pressed && { backgroundColor: theme.fill }]}
          onPress={() => { haptic("tap"); setCommentAnchor(selection); setSelection(undefined); setCommenting(true); }}>
          <Text style={[styles.headingText, { color: theme.foreground }]}>Comment on selection</Text>
        </Pressable>
      )}
      {body}
      <Sheet visible={docMenu} onClose={() => setDocMenu(false)} solid>
        <SheetTitle>{page?.title || "Doc"}</SheetTitle>
        {page && page.versions.length > 1 && <SheetRow label="Versions" onPress={() => { setDocMenu(false); setChoosing(true); }} />}
        {kind === "doc" && shown !== undefined && <SheetRow label="Export" onPress={() => { setDocMenu(false); setExporting(true); }} />}
      </Sheet>
      {kind === "doc" && shown !== undefined && page && (
        <DocExportSheet key={`${id}:${shown}`} pageId={id} title={page.title || "Doc"} version={shown} visible={exporting} onClose={() => setExporting(false)} />
      )}
      <Sheet visible={choosing} onClose={() => setChoosing(false)} solid>
        <SheetTitle>Version</SheetTitle>
        <ScrollView style={styles.versions}>
          <SheetRow label={`Latest (v${page?.latestVersion ?? ""})`} tone={version === "latest" ? "primary" : "normal"} onPress={() => pick("latest")} />
          {page?.versions.map((v) => (
            <SheetRow
              key={v.version}
              label={`v${v.version}${v.restoredFrom ? ` (restored v${v.restoredFrom})` : ""} · ${new Date(v.createdAt).toLocaleDateString()}`}
              tone={version === v.version ? "primary" : "normal"}
              onPress={() => pick(v.version)}
            />
          ))}
        </ScrollView>
      </Sheet>
      <DocContentsSheet outline={outline} visible={showContents} onPick={(index) => setDocHeading({ index })}
        onClose={() => setShowContents(false)} />
      {commentsSheet}
      {shareSheet}
    </DocScreen>
  );
}

const styles = StyleSheet.create({
  siteDraft: { ...text.sm, fontFamily: font.medium, marginHorizontal: 16, marginVertical: 8, paddingHorizontal: 12, paddingVertical: 10, borderRadius: 12, borderCurve: "continuous" },
  versions: { maxHeight: 360 },
  siteHint: { ...text.sm, fontFamily: font.regular, paddingHorizontal: 16, paddingVertical: 8 },
  heading: { minHeight: 44, paddingLeft: 16, paddingRight: 16, paddingVertical: 8, flexDirection: "row", alignItems: "center", gap: 8 },
  headingText: { ...text.body, fontFamily: font.regular, flex: 1 },
  note: { ...text.sm, fontFamily: font.regular, padding: 20 },
});
