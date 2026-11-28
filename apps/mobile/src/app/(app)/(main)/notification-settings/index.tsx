import { NOTIFICATION_GROUPS } from "@nexus/kit";
import { useAuth } from "@/auth";
import { channelsLine, GROUPS } from "@/inbox";
import { Blank, Footnote, Loading, Page, Row, Section } from "@/list";
import { useNotificationSettings } from "@/use-notification-settings";

/**
 * What notifies you, and where (brand v5's P97 on the phone): the account's groups under its name, each with the
 * channels it reaches you on ("Push · Email · Inbox"), and each opening its channels (`[group].tsx`). Everything always
 * reaches the inbox. The design's "Following your default" line waits on the Gateway, which keeps one set of choices
 * per account and no default across accounts.
 */
export default function NotificationSettings() {
  const { orgName } = useAuth();
  const { settings, error, pushAvailable } = useNotificationSettings();

  if (!settings) return error ? <Blank title="Couldn't load your notifications" body={error} tone="danger" /> : <Loading alone />;
  return (
    <Page>
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      <Section
        title={orgName || "This account"}
        caps
        footer={pushAvailable ? "Push goes to the devices you allowed it on." : "This Gateway can't send push yet. Your choices are kept for when it can."}
      >
        {NOTIFICATION_GROUPS.filter((group) => settings.some((s) => s.group === group)).map((group) => (
          <Row key={group} label={GROUPS[group].label} detail={channelsLine(settings, group)} href={{ pathname: "/notification-settings/[group]", params: { group } }} />
        ))}
      </Section>
    </Page>
  );
}
