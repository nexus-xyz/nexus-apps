import { isArtifactId, type ArtifactDetail } from "@nexus/kit";
import { Stack, router, useLocalSearchParams } from "expo-router";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ActivityIndicator, FlatList, Platform, Pressable, StyleSheet, Text, View } from "react-native";
import { useArtifacts, useAuth, useCore } from "@/auth";
import { DesignControls } from "@/design-controls";
import type { DesignFocus, DesignOutline } from "@/design-outline";
import { DeckExportSheet } from "@/deck-export-sheet";
import type { DeckInfo } from "@/deck-export";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import type { ImageSource } from "@/images";
import { Answer, Question } from "@/messages";
import { useModels } from "@/models";
import { ago } from "@/ago";
import { DocScreen, PageScreen } from "@/page-bar";
import { PageView } from "@/page-view";
import { UNVERIFIED_LABEL } from "@/pages";
import { absoluteLink, audienceLabel } from "@/share";
import { ShareSheet, type ShareActions } from "@/share-sheet";
import { SharedDocReader } from "@/shared-doc-reader";
import { SharedSiteReader } from "@/shared-site-reader";
import { followShared } from "@/live-page";
import { attachmentShared, frameShared, linkProblem, readShared, type LinkProblem } from "@/shared-link";
import { SheetCellControl, useSheetCellNavigation } from "@/sheet-cell-control";
import { font, mono, text, useTheme } from "@/theme";
import { ACCESSIBILITY_FONT_SCALE } from "@/text-size";

/**
 * An artifact, read first as brand v5's P20 draws one on the phone. A shared chat reads as a Doc: a small line with what
 * it is, how long and how old, its title, who shared it with whom, then its messages; Share in the bar. A shared page
 * (`kind: "page"`) owns the screen in its sandboxed WebView (`page-view.tsx`), at the version its share pins or live,
 * with its title, audience and version in the bar at its foot (`page-bar.tsx`).
 * `id` is an artifact's id, or a public link's token when a shared link opened the app (`app/a/[key].tsx`).
 */
export default function ArtifactReader() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const artifacts = useArtifacts();
  const core = useCore();
  const { ui, uiLink } = useAuth();
  const theme = useTheme();
  const cells = useSheetCellNavigation();
  const models = useModels(core);
  const [artifact, setArtifact] = useState<ArtifactDetail | null>(null);
  const [deck, setDeck] = useState<DeckInfo>();
  const [design, setDesign] = useState<DesignOutline>();
  const [designFocus, setDesignFocus] = useState<DesignFocus>();
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [problem, setProblem] = useState<LinkProblem | null>(null);
  const [sheet, setSheet] = useState<ShareActions | null>(null);

  useEffect(() => {
    // Expo Router opens this screen only with an id; a malformed link without one says so instead of loading forever.
    if (!id) {
      setError("That link has no artifact in it.");
      return;
    }
    readShared(artifacts, id).then(setArtifact, (e: unknown) => setProblem(linkProblem(e)));
  }, [artifacts, id]);

  const siteVersion = artifact?.pageFormat === "html" || artifact?.pageFormat === "jsx" ? artifact.pageVersion ?? undefined : undefined;
  const frame = useCallback(() => frameShared(artifacts, id, siteVersion), [artifacts, id, siteVersion]);
  // An unpinned page share follows its live edits, by the key it was opened with.
  const follow = useMemo(() => followShared(artifacts, id), [artifacts, id]);
  /** Its questions' images: the artifact's own copies, for whoever may read it; a public link's by its token. */
  const images = useMemo<ImageSource | undefined>(
    () => (artifact ? { key: `artifact:${artifact.id}`, load: (attachment) => attachmentShared(artifacts, id, attachment) } : undefined),
    [artifacts, artifact?.id, id],
  );
  const byline = (a: ArtifactDetail) => [a.author.name || a.author.email, audienceLabel(a.audience)].filter(Boolean).join(" · ");
  // As Nexus UI's: a reader outside the organization sees no author, and is told the page is unverified.
  const external = (a: ArtifactDetail) => a.variant === "external" && !a.mine;

  const actions = (a: ArtifactDetail): ShareActions => ({
    title: a.title || "Untitled",
    audience: audienceLabel(a.audience),
    link: ui ? absoluteLink(a.publicUrl ?? a.url, ui) : undefined,
    // Its author allowed it: a chat of your own that starts from this one.
    continueChat: a.allowContinue
      ? async () => {
          // By the key it was opened with: a reader outside the organization may continue a public link only by its token.
          const chat = await artifacts.continueArtifact(id);
          router.dismissTo({ pathname: "/session/[id]", params: { id: chat.id } });
        }
      : undefined,
    // Yours: shared again with another audience (a chat as it is now, or the page), replacing this share.
    shareable: a.mine ? (a.kind === "page" && a.pageId ? { kind: "page", id: a.pageId } : { kind: "chat", id: a.sessionId }) : undefined,
    // A new share is a new artifact: open it in place of this one, which it replaced.
    changed: (shared) =>
      shared ? router.replace({ pathname: "/artifact/[id]", params: { id: shared.id } }) : void artifacts.artifact(a.id).then(setArtifact, (e: Error) => setError(e.message)),
    markdown: a.kind === "page" || !isArtifactId(id) ? undefined : () => artifacts.exportArtifact(a.id),
    openUrl: ui ? absoluteLink(a.path, ui) : undefined,
    revoke: a.mine
      ? async () => {
          await artifacts.revokeArtifact(a.id);
          router.back();
        }
      : undefined,
  });

  const ask = () =>
    artifacts.requestArtifactAccess(id).then(
      () => setProblem((p) => p && { ...p, requested: true }),
      (e: Error) => setError(e.message),
    );

  if (artifact?.kind === "page") {
    const isExternal = external(artifact);
    const line = isExternal
      ? [UNVERIFIED_LABEL, artifact.organization].filter(Boolean).join(" · ")
      : [audienceLabel(artifact.audience), artifact.pageVersion ? "v" + artifact.pageVersion : "Live"].join(" · ");
    const onShare = () => setSheet(actions(artifact));
    const chrome = <>
      <Stack.Screen options={{ headerShown: false }} />
      {error ? <Text style={[styles.note, { color: theme.danger }]}>{error}</Text> : null}
      <ShareSheet actions={sheet} onClose={() => setSheet(null)} onError={setError} solid />
    </>;
    if (artifact.pageFormat === "md") {
      return <SharedDocReader key={id} artifact={artifact} external={isExternal} line={line} load={frame} follow={follow} onShare={onShare}>
        {chrome}
      </SharedDocReader>;
    }
    if (artifact.pageFormat === "html" || artifact.pageFormat === "jsx") {
      const canExport = deck && artifact.pageId && !isExternal;
      return <SharedSiteReader key={id} artifact={artifact} external={isExternal} line={line} load={frame} follow={follow} onShare={onShare}
        onExport={canExport ? () => { haptic("tap"); setExporting(true); } : undefined}
        viewControls={{ onDeckInfo: setDeck, onDesignOutline: setDesign, designFocus }}>
        {chrome}
        {design && <DesignControls outline={design} setFocus={setDesignFocus}
          pageId={isExternal ? undefined : artifact.pageId} version={artifact.pageVersion ?? undefined} />}
        {deck && artifact.pageId && canExport && (
          <DeckExportSheet pageId={artifact.pageId} version={artifact.pageVersion ?? undefined} deck={deck}
            visible={exporting} onClose={() => setExporting(false)} />
        )}
      </SharedSiteReader>;
    }
    const isSheet = artifact.pageFormat === "sheet";
    const Screen = isSheet ? DocScreen : PageScreen;
    return (
      <Screen
        title={artifact.title || "Untitled"}
        line={line}
        onShare={onShare}
      >
        {chrome}
        {isSheet && cells.selection && <SheetCellControl selection={cells.selection} result={cells.result} onRange={cells.setRange} />}
        <PageView load={frame} follow={follow} onSheetSelection={isSheet ? cells.receive : undefined} cellRange={cells.range} />
      </Screen>
    );
  }

  return (
    <>
      <Stack.Screen
        options={{
          // The title is the Doc's, in its first lines; the bar keeps Back and Share.
          title: "",
          headerRight: artifact
            ? () => (
                <Pressable
                  accessibilityRole="button"
                  hitSlop={12}
                  onPress={() => setSheet(actions(artifact))}
                  accessibilityLabel="Share and more"
                  // iOS draws the bar button's 44 pt itself; Android's header gives only the icon's 20 dp.
                  style={Platform.OS === "android" ? styles.headerButton : undefined}
                >
                  <Icon name="share" color={theme.foreground} />
                </Pressable>
              )
            : undefined,
        }}
      />
      {problem ? (
        <View style={styles.problem}>
          <Text style={[styles.problemTitle, { color: theme.foreground }]}>{problem.title}</Text>
          <Text style={[styles.problemText, { color: theme.muted }]}>{problem.text}</Text>
          {problem.kind === "access" && problem.requested ? (
            <Text style={[styles.problemText, { color: theme.foreground }]}>Access requested. You'll be able to open it here once the person who shared it approves.</Text>
          ) : problem.kind === "access" ? (
            <Pressable accessibilityRole="button" style={[styles.pill, { backgroundColor: theme.primary }]} onPress={ask}>
              <Text style={[styles.pillText, { color: theme.onPrimary }]}>Ask for access</Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
      {error ? <Text style={[styles.note, { color: theme.danger }]}>{error}</Text> : null}
      {!artifact && !error && !problem ? <ActivityIndicator style={styles.loading} color={theme.muted} /> : null}
      {artifact ? (
        <FlatList
          data={artifact.messages}
          keyExtractor={(m) => m.id}
          contentContainerStyle={styles.feed}
          ListHeaderComponent={
            <View style={styles.doc} accessible accessibilityRole="header" accessibilityLabel={artifact.title || "Untitled"}>
              <Text style={[styles.kind, { color: theme.tertiary }]}>
                {["Chat", `${artifact.messages.length} ${artifact.messages.length === 1 ? "message" : "messages"}`, ago(artifact.createdAt)].join(" · ")}
              </Text>
              {/* Already a display title: it grows to the first accessibility size, where its words still fit the line. */}
              <Text style={[styles.docTitle, { color: theme.foreground }]} maxFontSizeMultiplier={ACCESSIBILITY_FONT_SCALE}>
                {artifact.title || "Untitled"}
              </Text>
              <Text style={[styles.docBy, { color: theme.muted }]}>{external(artifact) ? [UNVERIFIED_LABEL, artifact.organization].filter(Boolean).join(" · ") : byline(artifact)}</Text>
            </View>
          }
          ListFooterComponent={
            artifact.hiddenSources > 0 ? (
              <Text style={[styles.note, { color: theme.muted }]}>
                {artifact.hiddenSources} {artifact.hiddenSources === 1 ? "source is" : "sources are"} left out for readers outside the organization.
              </Text>
            ) : undefined
          }
          renderItem={({ item }) =>
            item.role === "user" ? (
              <Question content={item.content} theme={theme} attachments={item.attachments} images={images} />
            ) : (
              <Answer message={item} models={models} theme={theme} />
            )
          }
        />
      ) : null}
      <ShareSheet actions={sheet} onClose={() => setSheet(null)} onError={setError} />
    </>
  );
}

const styles = StyleSheet.create({
  headerButton: { width: 48, height: 48, alignItems: "center", justifyContent: "center" },
  feed: { paddingHorizontal: 16, paddingTop: 4, paddingBottom: 24 },
  loading: { paddingVertical: 40 },
  // The Doc's first lines (P20): kind, length and age in mono, the title at 26, who shared it with whom.
  doc: { paddingHorizontal: 4, paddingTop: 4, paddingBottom: 20, gap: 6 },
  kind: { fontFamily: mono, fontSize: 12, lineHeight: 16, fontVariant: ["tabular-nums"] },
  docTitle: { fontFamily: font.display, fontSize: 26, lineHeight: 32, letterSpacing: -0.4 },
  docBy: { ...text.secondary, fontFamily: font.regular },
  note: { ...text.xs, fontFamily: font.regular, paddingVertical: 8, paddingHorizontal: 4 },
  problem: { gap: 12, padding: 24 },
  problemTitle: { ...text.lg, fontFamily: font.medium },
  problemText: { ...text.base, fontFamily: font.regular },
  pill: { alignSelf: "flex-start", minHeight: 44, paddingHorizontal: 20, borderRadius: 999, alignItems: "center", justifyContent: "center" },
  pillText: { ...text.base, fontFamily: font.medium },
});
