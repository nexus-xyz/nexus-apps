import * as Clipboard from "expo-clipboard";
import { Stack, useLocalSearchParams } from "expo-router";
import { StyleSheet, Text, View } from "react-native";
import { actionText, actorName, changeRows, KIND, keptEvent, show, SURFACE, targetName } from "@/audit";
import { Fact } from "@/fact";
import { Blank, LargeTitle, Page, Row, Section } from "@/list";
import { mono, useTheme } from "@/theme";
import { useToast } from "@/toast";

const when = (iso: string) =>
  new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit", timeZone: "UTC" }) + " UTC";

/**
 * One change of the audit log, full screen (brand v5's P29·E "An event", by P105·D's item): what changed and on what,
 * who made it and with which credential, when (to the second, UTC) and from where; the record before and after, field
 * by field; then Copy as JSON. It opens from the log, which holds the event: Gateway has no call for one event, so a
 * page opened after the app restarted says so. The board's Why and Open the rule are left out: an event names no rule.
 */
export default function AuditEvent() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const theme = useTheme();
  const toast = useToast();
  const event = keptEvent(id);
  if (!event) return <Blank title="This change is no longer loaded" body="Go back to Audit and open it again." />;
  const { rows, hasBefore, hasAfter } = changeRows(event);
  const metadata = event.metadata && Object.keys(event.metadata).length ? event.metadata : undefined;
  const copy = async () => {
    await Clipboard.setStringAsync(JSON.stringify(event, null, 2));
    toast({ text: "Copied as JSON", icon: "copy" });
  };
  return (
    <Page>
      <Stack.Screen options={{ title: "Change", headerTitle: "", headerBackButtonDisplayMode: "default" }} />
      <LargeTitle>{actionText(event.action)}</LargeTitle>
      <Section>
        <Fact label="Event" value={event.action} monoValue />
        <Fact label="On" value={`${targetName(event)} · ${event.target.type.replace(/_/g, " ")}`} />
        <Fact label="By" value={`${actorName(event)} · ${KIND[event.actor.kind]}`} />
        <Fact label="When" value={when(event.at)} />
        <Fact label="Where" value={SURFACE[event.surface]} />
        {event.requestId ? <Fact label="Request" value={event.requestId} monoValue /> : null}
      </Section>
      {rows.length ? (
        <Section title={hasBefore && hasAfter ? "Before → after" : hasAfter ? "Set to" : "Was"}>
          {rows.map((row) => (
            <Row
              key={row.field}
              label={
                <View style={styles.change}>
                  <Text style={[styles.mono, { color: theme.muted }]}>{row.field}</Text>
                  <Text style={[styles.mono, { color: theme.foreground }]} selectable>
                    {hasBefore && hasAfter ? `${show(row.from)} → ${show(row.to)}` : hasAfter ? show(row.to) : show(row.from)}
                  </Text>
                </View>
              }
              accessibilityLabel={`${row.field}: ${hasBefore && hasAfter ? `${show(row.from)} to ${show(row.to)}` : show(hasAfter ? row.to : row.from)}`}
            />
          ))}
        </Section>
      ) : null}
      {metadata ? (
        <Section title="Details">
          <Row
            label={
              <Text style={[styles.mono, { color: theme.foreground }]} selectable>
                {JSON.stringify(metadata, null, 2)}
              </Text>
            }
            accessibilityLabel={JSON.stringify(metadata)}
          />
        </Section>
      ) : null}
      <Section>
        <Row label="Copy as JSON" tone="primary" center onPress={copy} />
      </Section>
    </Page>
  );
}

const styles = StyleSheet.create({
  change: { gap: 2 },
  mono: { fontFamily: mono, fontSize: 13, lineHeight: 18 },
});
