import { router, type Href } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { ActivityIndicator, Alert, Image, ScrollView, StyleSheet, Text, View } from "react-native";
import { ago, spokenAgo } from "@/ago";
import { useAuth } from "@/auth";
import { Blank, Footnote, Page, Row, Section } from "@/list";
import { handOff } from "@/share-handoff";
import { contentsOf, destinationsFor, draftOf, skippedLine, type Destination } from "@/share-inbox";
import { shareInbox } from "@/share-inbox-native";
import { font, text, useTheme } from "@/theme";
import { clearRemovedForOthers, readInbox, removedForOthersCount, type Waiting } from "@/use-share-inbox";

const WHERE: Record<Destination, Href> = {
  chat: { pathname: "/session/[id]", params: { id: "new" } },
  assistant: "/agent",
};

/**
 * What another app shared to Nexus, before it reaches a composer: what it holds, what the extension left
 * out, the organization it goes to, and where: a new chat or the Assistant, asked each time. It becomes an unsent draft
 * there; nothing is uploaded or sent until Send, which keeps the composer's consent, policy and model checks. Discard
 * removes it from the phone.
 */
export default function Shared() {
  const theme = useTheme();
  const { shareAccount, org, orgName, agent } = useAuth();
  const [waiting, setWaiting] = useState<Waiting[] | null>(null);
  const [removed] = useState(removedForOthersCount);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  useEffect(clearRemovedForOthers, []);

  const load = useCallback(async () => {
    if (!shareAccount) return;
    setWaiting(await readInbox(shareAccount));
  }, [shareAccount]);
  useEffect(() => {
    void load();
  }, [load]);

  const shared = waiting?.[0];

  async function continueIn(to: Destination) {
    if (!shared || !org || busy) return;
    setBusy(true);
    setProblem(undefined);
    try {
      const copies = await shareInbox.copyImages(shared.id, shared.images.map((image) => image.file));
      // Out of the inbox before the composer has it, so a second review can't add it twice.
      await shareInbox.remove(shared.id);
      handOff({
        to,
        org,
        text: draftOf(shared),
        images: shared.images.map((image, n) => ({ uri: copies[n], name: image.name, mimeType: image.type })),
        skipped: skippedLine(shared.skipped),
      });
      router.dismissTo(WHERE[to]);
    } catch {
      setProblem("Nexus couldn't open the photos. Try again, or discard it.");
    } finally {
      setBusy(false);
    }
  }

  function discard() {
    if (!shared) return;
    Alert.alert("Discard what you shared?", "It is removed from this iPhone.", [
      { text: "Cancel", style: "cancel" },
      {
        text: "Discard",
        style: "destructive",
        onPress: async () => {
          await shareInbox.remove(shared.id);
          const rest = waiting?.slice(1) ?? [];
          setWaiting(rest);
          if (!rest.length && !removed) router.back();
        },
      },
    ]);
  }

  if (!waiting) {
    return (
      <Page>
        <ActivityIndicator style={styles.loading} color={theme.muted} accessibilityLabel="Reading what you shared" />
      </Page>
    );
  }

  const removedLine = removed
    ? `${removed === 1 ? "Something" : `${removed} items`} shared while another account was signed in ${removed === 1 ? "was" : "were"} removed.`
    : undefined;

  if (!shared) {
    return (
      <Page>
        <Blank title="Nothing waiting" body={removedLine ?? "What you share to Nexus from another app appears here."} />
      </Page>
    );
  }

  const destinations = destinationsFor(shared, !!agent);
  const skipped = skippedLine(shared.skipped);
  const iso = new Date(shared.createdAt).toISOString();
  const folder = shared.folder.endsWith("/") ? shared.folder : `${shared.folder}/`;
  const draft = draftOf(shared);

  return (
    <Page>
      {removedLine ? <Footnote>{removedLine}</Footnote> : null}
      <Section
        title={waiting.length > 1 ? `1 of ${waiting.length} shared` : undefined}
        footer={[skipped, shared.account === null ? "Shared while no one was signed in to Nexus. Check it before you continue." : undefined].filter(Boolean).join(" ") || undefined}
      >
        <Row label={contentsOf(shared)} value={ago(iso)} accessibilityLabel={`${contentsOf(shared)}, saved ${spokenAgo(iso)} ago`}>
          {draft ? (
            <Text style={[styles.words, { color: theme.foreground }]} numberOfLines={8} selectable>
              {draft}
            </Text>
          ) : null}
          {shared.images.length ? (
            <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.photos} accessibilityLabel={`${shared.images.length} photos`}>
              {shared.images.map((image) => (
                <Image key={image.file} source={{ uri: `${folder}${image.file}` }} style={[styles.photo, { backgroundColor: theme.userBubble }]} accessibilityLabel={image.name} />
              ))}
            </ScrollView>
          ) : null}
        </Row>
      </Section>
      <Section
        title="Continue in"
        footer={`It opens as a draft in ${orgName ?? "this organization"}. Nothing is sent until you tap Send. To use another organization, switch to it first.`}
      >
        {destinations.map((destination) => (
          <Row
            key={destination.to}
            label={destination.label}
            detail={destination.why}
            tone={destination.why ? undefined : "primary"}
            disabled={!!destination.why || busy}
            chevron={!destination.why}
            onPress={() => void continueIn(destination.to)}
            accessibilityHint={destination.why ? undefined : "Opens it as an unsent draft"}
          />
        ))}
      </Section>
      {problem ? <Footnote tone="danger">{problem}</Footnote> : null}
      <Section>
        <Row label="Discard" tone="danger" center disabled={busy} onPress={discard} />
      </Section>
    </Page>
  );
}

const styles = StyleSheet.create({
  loading: { marginTop: 48 },
  words: { ...text.sm, fontFamily: font.regular, paddingTop: 4 },
  photos: { gap: 8, paddingTop: 10 },
  photo: { width: 72, height: 72, borderRadius: 12, borderCurve: "continuous" },
});
