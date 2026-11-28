import { Stack, useLocalSearchParams, type Href } from "expo-router";
import { Blank, Page, Row, Section } from "@/list";
import { useAuth } from "@/auth";
import { useOrgs } from "@/org-switcher";
import { hubFor, settingsFor, type SettingsPart, type SettingsRow } from "@/settings-map";
import { usePermissions } from "@/use-permissions";

/**
 * The pages one Settings row holds (`settings-map.ts`): Preferences, General, People, Model access or Usage. Each
 * page is a row with one line on what it is for, as the desktop's nav lists a row's sections under it. `more` is the
 * drawer's Help and feedback: the Inbox, Archived, then the App part's help pages.
 */
export default function SettingsGroup() {
  const { key } = useLocalSearchParams<{ key: string }>();
  const { orgName, ui } = useAuth();
  const { current } = useOrgs();
  const personal = current ? current.kind === "personal" : orgName === "Personal";
  // The same pages Settings showed under this row: none you have no access to.
  const permissions = usePermissions();
  const parts = settingsFor({ personal, permissions, hidden: ui ? [] : ["/docs"] });
  const row = key === "more" ? more(parts) : hubFor(key ?? "", parts);
  if (!row?.pages) {
    return (
      <>
        <Stack.Screen options={{ title: "" }} />
        <Blank title="Not in this account" body="Switch account to see these settings." />
      </>
    );
  }
  const renderPage = (page: (typeof row.pages)[number]) => (
    <Row key={page.href} icon={page.icon} bareIcon label={page.label} detail={page.about} href={page.href as Href} />
  );
  return (
    <Page>
      <Stack.Screen options={{ title: row.label }} />
      {key === "more" ? (
        <>
          <Section>
            <Row icon="inbox" bareIcon label="Inbox" href="/inbox" />
            {row.pages.filter((page) => page.href === "/archived").map(renderPage)}
          </Section>
          <Section title="Help">{row.pages.filter((page) => page.href !== "/archived").map(renderPage)}</Section>
        </>
      ) : <Section>{row.pages.map(renderPage)}</Section>}
    </Page>
  );
}

function more(parts: SettingsPart[]): SettingsRow | null {
  const app = parts.find((part) => part.key === "app");
  return app ? { label: "More", icon: "info", href: "/settings-group/more", pages: app.rows } : null;
}
