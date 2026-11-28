import type { PageDetail, PageVersion } from "@nexus/kit";
import { Stack, router, useFocusEffect, useLocalSearchParams } from "expo-router";
import { useCallback, useState } from "react";
import { ago } from "@/ago";
import { useArtifacts } from "@/auth";
import { deployDetail } from "@/library";
import { Blank, Loading, Page, Row, Section } from "@/list";
import { Sheet, SheetRow, SheetTitle } from "@/sheet";
import { useToast } from "@/toast";

/** How long Undo stays on a rollback's toast: long enough to read the site and change your mind. */
const UNDO_FOR = 10_000;
/** A deploy's time in its sheet: "Nov 5, 9:52 AM", since several deploys land on one day. */
const WHEN = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });

/**
 * A site's deploys (brand v5's P20 Site · Deploys): each version, newest first, with how it was written and when. A
 * version opens a sheet to read it or roll the site back to it. A rollback writes a new version with the old one's
 * content, as Nexus UI's Restore does (nothing is lost), and its toast holds Undo for ten seconds, which rolls forward
 * to the version that was up.
 */
export default function SiteDeploys() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const artifacts = useArtifacts();
  const toast = useToast();
  const [page, setPage] = useState<PageDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [chosen, setChosen] = useState<PageVersion | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => artifacts.page(id).then(setPage, (e: Error) => setError(e.message)), [artifacts, id]);
  useFocusEffect(
    useCallback(() => {
      void load();
    }, [load]),
  );

  /** `version`'s content as a new version on top of `base`, which must still be the latest. */
  const restore = (version: number, base: number) => artifacts.restorePage(id, { version, baseVersion: base });

  const rollBack = async (to: PageVersion) => {
    if (!page || busy) return;
    const was = page.latestVersion;
    setChosen(null);
    setBusy(true);
    try {
      const written = await restore(to.version, was);
      await load();
      toast({
        text: `Rolled back to v${to.version}`,
        icon: "history",
        stays: UNDO_FOR,
        action: {
          label: "Undo",
          onPress: () =>
            restore(was, written.version.version)
              .then(load)
              .then(() => toast({ text: `Back on v${was}` }))
              .catch((e: Error) => toast({ text: `Couldn't undo: ${e.message}` })),
        },
      });
    } catch (e) {
      toast({ text: `Couldn't roll back: ${(e as Error).message}` });
    } finally {
      setBusy(false);
    }
  };

  const latest = page?.latestVersion;
  return (
    <Page onRefresh={() => void load()}>
      <Stack.Screen options={{ title: "Deploys" }} />
      {error ? (
        <Blank title="Couldn't read the deploys" body={error} tone="danger" />
      ) : !page ? (
        <Loading rows={4} />
      ) : (
        <Section footer="Rolling back writes a new version with the old one's content, so every version stays here.">
          {page.versions.map((v) => (
            <Row
              key={v.version}
              label={`v${v.version}`}
              detail={[v.version === latest ? "Up now" : undefined, deployDetail(v.createdVia, v.restoredFrom)].filter(Boolean).join(" · ") || undefined}
              value={ago(v.createdAt)}
              onPress={() => setChosen(v)}
              disabled={busy}
            />
          ))}
        </Section>
      )}
      <Sheet visible={!!chosen} onClose={() => setChosen(null)}>
        {chosen ? (
          <>
            <SheetTitle>{`v${chosen.version} · ${WHEN.format(new Date(chosen.createdAt))}`}</SheetTitle>
            <SheetRow
              label={`Open v${chosen.version}`}
              onPress={() => {
                setChosen(null);
                router.push({ pathname: "/page/[id]", params: { id, ...(chosen.version === latest ? {} : { version: String(chosen.version) }) } });
              }}
            />
            {chosen.version === latest ? null : <SheetRow label={`Roll back to v${chosen.version}`} tone="primary" onPress={() => void rollBack(chosen)} />}
            <SheetRow label="Cancel" tone="muted" onPress={() => setChosen(null)} />
          </>
        ) : null}
      </Sheet>
    </Page>
  );
}
