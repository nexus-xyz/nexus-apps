import type { Artifact, PageDetail } from "@nexus/kit";
import * as Clipboard from "expo-clipboard";
import * as WebBrowser from "expo-web-browser";
import { Stack, router, useFocusEffect, useLocalSearchParams } from "expo-router";
import { useCallback, useState } from "react";
import { Alert, StyleSheet, Text } from "react-native";
import { ago } from "@/ago";
import { exportName, sharePageExport } from "@/artifact-export";
import { ArtifactExportSheet } from "@/artifact-export-sheet";
import { useArtifacts, useAuth } from "@/auth";
import { haptic } from "@/haptics";
import { Blank, Loading, Page, Row, Section } from "@/list";
import { formatLabel } from "@/pages";
import { absoluteLink, audienceLabel } from "@/share";
import { ShareSheet, type ShareActions } from "@/share-sheet";
import { SheetRow } from "@/sheet";
import { font, text, useTheme } from "@/theme";
import { useToast } from "@/toast";

/**
 * A site's overview (brand v5's P20 Site, from the page's More): the version that is up and when it was written, its
 * address once it is shared and who can open it, Open, and its deploys. The design's status marks and visitor counts are
 * left out: the Gateway records neither.
 */
export default function SiteOverview() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const artifacts = useArtifacts();
  const { ui } = useAuth();
  const toast = useToast();
  const theme = useTheme();
  const [page, setPage] = useState<PageDetail | null>(null);
  const [share, setShare] = useState<Artifact | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [sheet, setSheet] = useState<ShareActions | null>(null);

  const load = useCallback(() => {
    artifacts.page(id).then(
      async (detail) => {
        setPage(detail);
        setError(null);
        // The page's share, if it has one: the list is searched by title, so the match is checked by the page's id.
        const shared = await artifacts.artifacts({ scope: "shared_by_me", status: "active", q: detail.title, limit: 20 }).catch(() => null);
        setShare(shared?.artifacts.find((a) => a.pageId === id) ?? null);
      },
      (e: Error) => setError(e.message),
    );
  }, [artifacts, id]);
  useFocusEffect(load);

  const latest = page?.versions[0];
  const link = share ? (share.publicUrl ?? share.url) : undefined;
  const address = link && ui ? absoluteLink(link, ui) : link;
  const copy = async () => {
    if (!address) return;
    await Clipboard.setStringAsync(address);
    toast({ text: "Link copied", icon: "copy" });
  };

  const liveAddress = share && share.pageVersion == null && address && /^https?:\/\//i.test(address) ? address : undefined;
  const canExport = page?.format === "html" || page?.format === "jsx";
  const openLive = () => {
    if (!liveAddress) return;
    haptic("tap");
    setExporting(false);
    // Let the export sheet close before the browser presents its own controller.
    setTimeout(() => {
      void WebBrowser.openBrowserAsync(liveAddress).catch(() => toast({ text: "Couldn't open the site", icon: "globe" }));
    }, 450);
  };

  return (
    <Page onRefresh={load}>
      <Stack.Screen options={{ title: page?.title || "Site" }} />
      {error ? (
        <Blank title="Couldn't open this site" body={error} tone="danger" />
      ) : !page ? (
        <Loading />
      ) : (
        <>
          <Section>
            <Row label="Version" value={`v${page.latestVersion}`} />
            <Row label="Written" value={ago(latest?.createdAt ?? page.updatedAt)} />
            <Row label="Kind" value={formatLabel(page.format)} />
          </Section>
          <Section title="Sharing" footer={address ? "Tap the address to copy it." : "Share the site to give it an address."}>
            {address ? <Row label="Address" detail={address} onPress={copy} accessibilityHint="Copies the address" /> : null}
            <Row
              label="Who can open it"
              detail={share ? audienceLabel(share.audience) : "Only you"}
              chevron
              onPress={() => setSheet({ title: page.title || "This site", shareable: { kind: "page", id }, changed: load })}
            />
          </Section>
          <Section>
            <Row label="Open" tone="primary" onPress={() => router.dismissTo({ pathname: "/page/[id]", params: { id } })} />
          </Section>
          {canExport && (
            <Section>
              <Row label="Export" onPress={() => { haptic("tap"); setExporting(true); }} chevron />
              <ArtifactExportSheet
                key={page.latestVersion}
                title={"Export v" + page.latestVersion}
                visible={exporting}
                onClose={() => setExporting(false)}
                extra={liveAddress ? <SheetRow label="Open live" onPress={openLive} /> : (
                  <Text style={[styles.note, { color: theme.muted }]}>Live site unavailable</Text>
                )}
                options={[{
                  label: "Download source (.zip)",
                  run: (signal) => sharePageExport(artifacts, id, "zip", { version: page.latestVersion }, signal,
                    exportName((page.title || "Site") + "-v" + page.latestVersion, "zip")),
                }]}
              />
            </Section>
          )}
          {canExport && (
            <Section>
              <Row label="Edit" detail="Open on a computer to edit this site." icon="compose" disabled />
            </Section>
          )}
          <Section>
            <Row
              label="Deploys"
              icon="history"
              value={page.versions.length === 1 ? "1 version" : `${page.versions.length} versions`}
              href={{ pathname: "/site/[id]/deploys", params: { id } }}
            />
          </Section>
        </>
      )}
      <ShareSheet actions={sheet} onClose={() => setSheet(null)} onError={(message) => Alert.alert("Couldn't do that", message)} />
    </Page>
  );
}

const styles = StyleSheet.create({
  note: { ...text.secondary, fontFamily: font.regular, paddingHorizontal: 20, paddingVertical: 12 },
});
