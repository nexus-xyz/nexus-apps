import { NOTIFICATION_GROUPS, type NotificationGroup } from "@nexus/kit";
import { Stack, useLocalSearchParams } from "expo-router";
import { GROUPS } from "@/inbox";
import { Blank, Footnote, Loading, Page, Row, Section } from "@/list";
import { Switch } from "@/switch";
import { useNotificationSettings, type Channel } from "@/use-notification-settings";

/**
 * One group's channels (P97: "Each row opens its channels"): the inbox, which always gets everything, then Push and
 * Email, each a switch. A group that holds a required kind still delivers that kind with its switch off, and says so.
 * Push this Gateway cannot send is shown off and greyed, with why.
 */
export default function NotificationGroupSettings() {
  const params = useLocalSearchParams<{ group: string }>();
  const group = (NOTIFICATION_GROUPS as readonly string[]).includes(params.group) ? (params.group as NotificationGroup) : undefined;
  const { settings, saving, error, set, of } = useNotificationSettings();

  if (!group) return <Blank title="No such notifications" body="That link names no group of notifications." tone="danger" />;
  const title = GROUPS[group].label;
  if (!settings) return error ? <Blank title="Couldn't load your notifications" body={error} tone="danger" /> : <Loading alone />;

  const push = of(group, "push");
  const required = settings.some((s) => s.group === group && s.required);
  const channel = (c: Channel, label: string) => {
    const setting = of(group, c);
    if (!setting) return null;
    const usable = setting.available && !setting.locked;
    return (
      <Row
        key={c}
        label={label}
        accessory={
          // The switch stays while it saves, already showing the change: a spinner in its place would move the row.
          <Switch value={setting.available && setting.setting === "on"} onValueChange={(on) => set(group, c, on)} disabled={!usable || saving !== null} accessibilityLabel={`${label}: ${title}`} />
        }
      />
    );
  };

  return (
    <Page>
      <Stack.Screen options={{ title }} />
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      <Section
        footer={[
          GROUPS[group].detail ? `${GROUPS[group].detail}.` : undefined,
          required ? "Some of these are required: they still reach you with a switch off." : undefined,
          push && !push.available ? "This Gateway can't send push yet. Your choice is kept for when it can." : undefined,
        ]
          .filter(Boolean)
          .join(" ")}
      >
        <Row label="Inbox" value="Always" />
        {channel("push", "Push")}
        {channel("email", "Email")}
      </Section>
    </Page>
  );
}
