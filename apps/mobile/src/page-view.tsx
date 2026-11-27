import { consentKey, createBridge, sharedWriteKey, type BridgeReply, type PageComment, type PageFrame, type SiteFrameRoute } from "@nexus/kit";
import * as WebBrowser from "expo-web-browser";
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { ActivityIndicator, Alert, Pressable, StyleSheet, Text, View } from "react-native";
import { WebView, type WebViewMessageEvent } from "react-native-webview";
import { useArtifacts, useCore, useStored } from "@/auth";
import { deckInfoScript, readDeckInfo, type DeckInfo } from "@/deck-export";
import { designFocusScript, designOutlineScript, readDesignOutline, type DesignFocus, type DesignOutline } from "@/design-outline";
import { docCommentScript, locateDocComment, readDocCommentAnchor, type DocCommentAnchor } from "@/doc-comment-anchor";
import { docOutlineScript, jumpToDocHeading, readDocOutline, type DocOutline } from "@/doc-contents";
import { followLive, isLiveFrame, liveRevision, liveUrl, mintsAgain, type Follow } from "@/live-page";
import { asker, nativeCall, replyScript } from "@/page-bridge";
import { haptic } from "@/haptics";
import { pageNavigation } from "@/pages";
import { siteFrameRoutes, siteRouteForLink, siteRouteForPath } from "@/site-route-navigation";
import { SiteRoutePicker } from "@/site-route-picker";
import { useScreenFocused } from "@/screen-focus";
import { goToCellScript, readSheetSelection, sheetCellScript, type CellRange, type SheetSelection } from "@/sheet-cell";
import { readSitePinEvent, siteMarkers, sitePinScript, updateSitePins, type SitePinEvent } from "@/site-pins";
import { font, text, useTheme } from "@/theme";

/**
 * A Nexus Page in a WebView, as Nexus UI's sandboxed frame shows it. `load` asks Gateway for the signed
 * frame URL, on the sandbox host. It is asked again when `load` changes (another version) and after Try again. The URL
 * lasts five minutes: it only has to be valid when the page loads.
 *
 * The page runs its scripts and nothing more. The host's CSP `sandbox` gives it an opaque origin, so it has no cookies
 * or storage. The WebView keeps it on its own origin (`pageNavigation`), has no file access, and shares no cookies with
 * the app and keeps none (`incognito`).
 *
 * Its one way out is the page bridge (`usePageBridge`): `window.nexus.complete()`, `window.nexus.storage` and
 * `window.fs.readFile` reach the app through `onMessage`, and `createBridge` (`@nexus/kit`) checks and answers them as
 * Nexus UI does. The page never gets the reader's token. The answer goes back through `injectJavaScript`, as data.
 *
 * Live: with `follow`, a frame of the page's live state reloads at each `version` and `preview` event
 * (`./live-page.ts`), so an editor's change shows within about a second. A URL about to expire is minted again first.
 * The stream stops when the share is revoked or expires, and the page then stays as it is.
 */
export function PageView({ load, follow, onDocOutline, docHeading, onSheetSelection, cellRange, onDeckInfo, onDesignOutline, designFocus, onCommentAnchor, commentFocus, sitePins }: {
  load: () => Promise<PageFrame>;
  follow?: Follow;
  onDesignOutline?: (outline: DesignOutline) => void;
  designFocus?: DesignFocus;
  onDeckInfo?: (deck: DeckInfo) => void;
  sitePins?: {
    comments: PageComment[];
    writable: boolean;
    onEvent: (event: SitePinEvent, version: number, saved: boolean) => void;
  };
  onCommentAnchor?: (anchor: DocCommentAnchor) => void;
  commentFocus?: DocCommentAnchor;
  onDocOutline?: (outline: DocOutline) => void;
  docHeading?: { index: number };
  onSheetSelection?: (selection: SheetSelection) => void;
  cellRange?: CellRange;
}) {
  const theme = useTheme();
  const [baseFrame, setFrame] = useState<PageFrame | null>(null);
  const [sitePage, setSitePage] = useState<{ id: string; path: string }>();
  const routes = useMemo(() => siteFrameRoutes(baseFrame), [baseFrame]);
  const currentRoute = siteRouteForPath(routes, sitePage?.id === baseFrame?.pageId ? sitePage?.path : undefined);
  const frame = baseFrame && currentRoute && currentRoute.path !== "/" ? { ...baseFrame, url: currentRoute.url } : baseFrame;
  const pickRoute = (route: SiteFrameRoute) => {
    if (!frame || route.path === currentRoute?.path) return;
    haptic("tap");
    setSitePage({ id: frame.pageId, path: route.path });
    if (route.path !== "/") {
      sitePins?.onEvent({ type: "ready", supported: false }, frame.version, !isLiveFrame(frame.url));
    }
  };
  const [revision, setRevision] = useState<string>();
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  const shown = useRef(frame);
  shown.current = frame;
  // `WebView<object>`: the class's default type argument makes its props `never` once a ref names it.
  const web = useRef<WebView<object>>(null);
  // A reloaded live frame may show unsaved edits, as Nexus UI's `preview` frame.
  const onMessage = usePageBridge(frame, revision !== undefined, web);

  useEffect(() => {
    if (designFocus) web.current?.injectJavaScript(designFocusScript(designFocus));
  }, [designFocus]);
  useEffect(() => {
    if (commentFocus) web.current?.injectJavaScript(locateDocComment(commentFocus));
  }, [commentFocus]);

  useEffect(() => {
    if (docHeading) web.current?.injectJavaScript(jumpToDocHeading(docHeading.index));
  }, [docHeading]);

  useEffect(() => {
    if (cellRange) web.current?.injectJavaScript(goToCellScript(cellRange));
  }, [cellRange]);

  useEffect(() => {
    let live = true;
    setFrame(null);
    setRevision(undefined);
    setError(null);
    load().then(
      (next) => live && setFrame(next),
      (e: Error) => live && setError(e.message),
    );
    return () => {
      live = false;
    };
  }, [load, attempt]);

  // Follow while a live frame shows on the screen in front: a new frame URL of the same page keeps the stream. Back in
  // front after a while away, the frame loads again, so edits made meanwhile show.
  const focused = useScreenFocused();
  const away = useRef(false);
  useEffect(() => {
    if (!focused) away.current = true;
    else if (away.current) {
      away.current = false;
      if (shown.current && isLiveFrame(shown.current.url)) retry();
    }
  }, [focused, retry]);
  const showPins = !currentRoute || currentRoute.path === "/";
  const siteComments = showPins ? sitePins?.comments : undefined;
  const siteVersion = frame?.version;
  const savedSite = Boolean(frame && !isLiveFrame(frame.url));
  const canWritePins = Boolean(showPins && sitePins?.writable && savedSite);
  const markerState = useMemo(() => siteComments ? siteMarkers(siteComments, siteVersion) : undefined, [siteComments, siteVersion]);
  useEffect(() => {
    if (markerState) web.current?.injectJavaScript(updateSitePins(markerState, canWritePins));
  }, [markerState, canWritePins]);

  const following = Boolean(follow && frame && isLiveFrame(frame.url) && focused);
  useEffect(() => {
    if (!follow || !following) return;
    const controller = new AbortController();
    void followLive(
      follow,
      (event) => {
        const next = liveRevision(event);
        if (!next) return;
        const current = shown.current;
        if (current && mintsAgain(current.expiresAt)) {
          load().then(
            (fresh) => {
              if (controller.signal.aborted) return;
              setFrame(fresh);
              setRevision(next);
            },
            () => undefined,
          );
        } else {
          setRevision(next);
        }
      },
      controller.signal,
    );
    return () => controller.abort();
  }, [follow, following, load]);

  if (error) {
    return (
      <View style={styles.center}>
        <Text style={[styles.note, { color: theme.danger }]}>{error}</Text>
        <Pressable accessibilityRole="button" hitSlop={12} onPress={retry}>
          <Text style={[styles.note, { color: theme.primary }]}>Try again</Text>
        </Pressable>
      </View>
    );
  }
  if (!frame) return <ActivityIndicator style={styles.loading} color={theme.muted} />;
  const webView = (
    <WebView
      ref={web}
      // A new URL is a new WebView: after a crash, the old one cannot load again.
      key={baseFrame?.url ?? frame.url}
      // A new revision loads the live state again in the same WebView.
      source={{ uri: liveUrl(frame.url, revision) }}
      style={{ backgroundColor: theme.screen }}
      // Every URL goes to the check below. The default list sends the rest to Linking.openURL, which would open a
      // `nexus://` link in the app itself.
      originWhitelist={["*"]}
      onShouldStartLoadWithRequest={(request) => {
        if (routes.length > 1 && request.isTopFrame !== false) {
          const route = siteRouteForLink(frame.url, request.url, routes);
          if (route) {
            pickRoute(route);
            return false;
          }
        }
        const go = pageNavigation(frame.url, request.url);
        if (go === "browser" && request.isTopFrame !== false) void WebBrowser.openBrowserAsync(request.url).catch(() => undefined);
        return go === "load";
      }}
      onLoadEnd={() => {
        if (onDocOutline) web.current?.injectJavaScript(docOutlineScript);
        if (onSheetSelection) web.current?.injectJavaScript(sheetCellScript);
        if (onDeckInfo) web.current?.injectJavaScript(deckInfoScript);
        if (onDesignOutline) web.current?.injectJavaScript(designOutlineScript);
        if (onDocOutline || onCommentAnchor) web.current?.injectJavaScript(docCommentScript);
        if (sitePins && markerState) {
          web.current?.injectJavaScript(sitePinScript);
          web.current?.injectJavaScript(updateSitePins(markerState, canWritePins));
        }
      }}
      onMessage={(event) => {
        if (onDocOutline || onSheetSelection || onDeckInfo || onDesignOutline || onCommentAnchor || sitePins) {
          const data = nativeCall(frame.url, event.nativeEvent.url, event.nativeEvent.data);
          const outline = onDocOutline && readDocOutline(data);
          if (outline && onDocOutline) {
            onDocOutline(outline);
            return;
          }
          const selection = onSheetSelection && readSheetSelection(data);
          if (selection && onSheetSelection) {
            onSheetSelection(selection);
            return;
          }
          const design = onDesignOutline && readDesignOutline(data);
          if (design && onDesignOutline) {
            onDesignOutline(design);
            return;
          }
          const deck = onDeckInfo && readDeckInfo(data);
          if (deck && onDeckInfo) {
            onDeckInfo(deck);
            return;
          }
          const siteEvent = showPins && sitePins && readSitePinEvent(data);
          if (siteEvent && sitePins) {
            if (siteEvent.type === "ready" || (siteEvent.type === "draft" && canWritePins)
              || (siteEvent.type === "open" && markerState?.some((marker) => marker.id === siteEvent.id))) {
              sitePins.onEvent(siteEvent, frame.version, savedSite);
            }
            return;
          }
          const anchor = onCommentAnchor && readDocCommentAnchor(data);
          if (anchor && onCommentAnchor) {
            onCommentAnchor(anchor);
            return;
          }
        }
        onMessage(frame.url, event);
      }}
      javaScriptEnabled
      allowFileAccess={false}
      allowFileAccessFromFileURLs={false}
      allowUniversalAccessFromFileURLs={false}
      incognito
      sharedCookiesEnabled={false}
      thirdPartyCookiesEnabled={false}
      // One window: a `target="_blank"` link or `window.open` loads here, through the check above.
      setSupportMultipleWindows={false}
      allowsLinkPreview={false}
      // The page's process ended (memory pressure): ask for a fresh URL, since this one may have expired.
      onContentProcessDidTerminate={retry}
      onRenderProcessGone={retry}
    />
  );
  if (routes.length < 2 || !currentRoute) return webView;
  return (
    <View style={{ flex: 1 }}>
      <SiteRoutePicker routes={routes} selected={currentRoute} onPick={pickRoute} />
      {webView}
    </View>
  );
}

/** What a page asks the reader, once per page: to use Nexus AI, and to write data that every reader sees. */
type Prompt = "ai" | "shared";

/**
 * The page's bridge, as Nexus UI's frame answers it: a model call on the reader's session once the reader
 * allows the page, the page's own storage, with a question before its first shared write, and the data files of the
 * version the frame shows (`fs.readFile`). The choice is kept for the page in the signed-in organization
 * until sign-out (`choice:` in `store.ts`). A frame that may show unsaved edits (`preview`) refuses `complete()` and
 * storage writes. The answer goes into the document the WebView shows now; each id carries its document's prefix
 * (`BRIDGE_SCRIPT`), so an answer that outlives a reload settles nothing.
 *
 * As in Nexus UI's frame, Deny has no undo but signing out. Add a per-page setting on both when readers ask
 * to change their mind.
 */
function usePageBridge(frame: PageFrame | null, preview: boolean, web: RefObject<WebView<object> | null>) {
  const core = useCore();
  const artifacts = useArtifacts();
  const pageId = frame?.pageId ?? "";
  const choices = { ai: useStored<boolean>("choice", consentKey(pageId)), shared: useStored<boolean>("choice", sharedWriteKey(pageId)) };
  const latest = useRef({ core, artifacts, preview, choices, title: frame?.title, version: frame?.version });
  latest.current = { core, artifacts, preview, choices, title: frame?.title, version: frame?.version };
  const bridge = useRef<{ answer: ReturnType<typeof createBridge>; source: { postMessage: (reply: BridgeReply) => void } } | null>(null);

  useEffect(() => {
    if (!pageId) return;
    const ask = (prompt: Prompt) =>
      asker(
        () => latest.current.choices[prompt],
        (decide) => {
          const [title, text, no] =
            prompt === "ai"
              ? ["Allow Nexus AI?", `“${latest.current.title || "This page"}” wants to use Nexus AI. Calls are billed to your account.`, "Deny"]
              : ["Allow shared data?", "This page saves data that everyone who can open it will see.", "Cancel"];
          Alert.alert(title, text, [{ text: no, style: "cancel", onPress: () => decide(false) }, { text: "Allow", onPress: () => decide(true) }], { cancelable: false });
        },
      );
    const source = { postMessage: (reply: BridgeReply) => web.current?.injectJavaScript(replyScript(reply)) };
    const answer = createBridge({
      frame: () => source,
      core: () => latest.current.core,
      consent: ask("ai"),
      pageId,
      storage: () => latest.current.artifacts,
      confirmShared: ask("shared"),
      preview: () => latest.current.preview,
      version: () => latest.current.version,
    });
    bridge.current = { answer, source };
    return () => {
      answer.dispose();
      bridge.current = null;
    };
  }, [pageId, web]);

  return useCallback((frameUrl: string, event: WebViewMessageEvent) => {
    const data = nativeCall(frameUrl, event.nativeEvent.url, event.nativeEvent.data);
    const current = bridge.current;
    if (data !== undefined && current) current.answer.onMessage({ source: current.source, data });
  }, []);
}

const styles = StyleSheet.create({
  center: { flex: 1, alignItems: "center", justifyContent: "center", padding: 20, gap: 8 },
  loading: { paddingVertical: 40 },
  note: { ...text.sm, fontFamily: font.regular, textAlign: "center" },
});
