import type { CurrentTenantResponse, SpendResponse } from "@nexus/kit";
import Constants from "expo-constants";
import { useFocusEffect, type Href } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { StyleSheet, Text } from "react-native";
import { useAuth, useCore, useStored } from "@/auth";
import { keptConsent } from "@/consent-sheet";
import { Footnote, LargeTitle, Page, Row, Section } from "@/list";
import { OrgRow } from "@/org-row";
import { OrgSheet } from "@/org-sheet";
import { useOrgs } from "@/org-switcher";
import { orgLabel } from "@/orgs";
import { planLabel, usageLine } from "@/plan";
import { findSettings, settingsFor, type PartKey, type SettingsRow } from "@/settings-map";
import { SettingsProfile } from "@/settings-profile";
import { SettingsSearch } from "@/settings-search";
import { settingsLine } from "@/spend";
import { font, keptAppearance, text, useTheme, type AppearanceChoice } from "@/theme";
import { usePermissions } from "@/use-permissions";
import { usePlan } from "@/use-plan";
import { useSwitchOrg } from "@/use-switch-org";

// Each part's title, in small capitals over its card. "Account" for You: the profile header above already says whose.
const TITLES: Record<PartKey, string> = { you: "Account", org: "Organization", app: "App" };

const APPEARANCES: { value: AppearanceChoice; label: string }[] = [
  { value: "unspecified", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

/** Organizations is the switch row's page, and Profile the header's: the list leaves them out, Search finds them. */
const OPENED_ELSEWHERE = ["/accounts", "/profile"];

/**
 * Settings, a full-screen page as the chat apps lay out theirs, over the desktop's map (`settings-map.ts`): a search
 * field, who you are (`settings-profile.tsx`, which opens Profile), then Account, Organization and App in small
 * capitals, each a card of one-line rows with their state on the right. The organization leads its card
 * (`org-row.tsx`): a tap opens the sheet of every organization, the ones you were in last first. A row that holds
 * several pages (Preferences, General, People, Model access, Usage) opens the list of them (`settings-group/[key].tsx`).
 * Privacy holds the permission to send chats to AI providers and the crash reports switch (`privacy.tsx`); About holds
 * support, the policies and the version (`about.tsx`). Delete account and Sign out stay here, at the foot (App Review
 * 5.1.1(v)).
 */
export default function Settings() {
  const { name, signOut, ui, agent } = useAuth();
  const core = useCore();
  const theme = useTheme();
  const spendCache = useStored<SpendResponse>("cache", "spend");
  const { orgs, current, error } = useOrgs();
  const switcher = useSwitchOrg(current);
  const permissions = usePermissions();
  const [picking, setPicking] = useState(false);
  const [query, setQuery] = useState("");
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [spend, setSpend] = useState<SpendResponse | null>(() => spendCache.load() ?? null);
  // The theme's name and the AI permission on their rows, read again when Settings shows: their pages change them.
  const [look, setLook] = useState(keptAppearance);
  const [aiAllowed, setAiAllowed] = useState(false);
  useFocusEffect(useCallback(() => {
    setLook(keptAppearance());
    setAiAllowed(!!keptConsent.load()?.length);
  }, []));
  // How the organization pays and what is left of its free start; a member of a company sees no Billing row.
  const { plan, usage } = usePlan();

  useEffect(() => {
    core.me().then(setMe, () => undefined);
    // The budget on the Usage row; without it, the row shows no figure.
    core.spend().then((s) => {
      setSpend(s);
      spendCache.save(s);
    }, () => undefined);
  }, [core, spendCache]);

  const personal = current ? current.kind === "personal" : me?.tenant.kind === "personal";
  // On Free the budget is only a cap: "$99.99 of $100.00 left" would read as money beside a used-up free start.
  const budget = plan.kind === "free" ? "Free" : spend ? settingsLine(spend) : undefined;
  // Assistant only in a build with the agent API.
  const hidden = [...(plan.kind === "unknown" ? ["/billing"] : []), ...(ui ? [] : ["/docs"]), ...(agent ? [] : ["/assistant-settings"])];
  // A page you have no access to in this organization is not shown (Billing to an owner, Usage to an owner or admin).
  const parts = settingsFor({ personal, hidden: [...hidden, ...OPENED_ELSEWHERE], permissions });
  const matches = query.trim()
    ? findSettings(query, settingsFor({ personal, hidden, permissions }), current ? orgLabel(current) : "", personal)
    : null;

  // A row's state on its right, where the phone has one.
  const value: Record<string, string | undefined> = {
    "/settings-group/preferences": APPEARANCES.find((a) => a.value === look)?.label,
    "/appearance": APPEARANCES.find((a) => a.value === look)?.label,
    "/settings-group/usage": budget,
    "/spend": budget,
    "/billing": planLabel(plan),
    "/privacy": aiAllowed ? undefined : "AI asks first",
  };
  // Rows are one line, as the chat apps' are: a row's state sits on its right, and the only line under a name is the
  // free start running out on Billing, which the person must see.
  const detail = (row: SettingsRow) => (row.href === "/billing" && plan.kind === "free" && usage ? usageLine(usage) : undefined);

  const results = matches?.length ? (
    <Section>
      {matches.map((match) => (
        <Row key={match.href} icon={match.icon} bareIcon label={match.label} detail={match.where} href={match.href as Href} />
      ))}
    </Section>
  ) : (
    <Footnote>{`No settings match “${query.trim()}”.`}</Footnote>
  );

  const list = (
    <>
      <SettingsProfile name={name} email={me?.user?.email} />
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      {parts.map((part) => {
        const rows = part.rows.map((row) => (
          <Row
            key={row.href}
            icon={row.icon}
            bareIcon
            label={row.label}
            value={value[row.href]}
            detail={detail(row)}
            detailTone={row.href === "/billing" && usage?.usedUp ? "danger" : undefined}
            href={row.href as Href}
          />
        ));
        if (part.key !== "org") {
          return (
            <Section key={part.key} title={TITLES[part.key]} caps>
              {rows}
            </Section>
          );
        }
        // The organization first, its settings under it in the same card.
        return (
          <Section
            key={part.key}
            title={TITLES.org}
            caps
            footer={switcher.problem && !picking ? switcher.problem : undefined}
            footerTone="danger"
          >
            <OrgRow
              key="org"
              current={current}
              logo={me?.tenant.imageUrl}
              count={orgs?.length ?? 0}
              switching={!!switcher.switching}
              onOpen={() => setPicking(true)}
            />
            {rows}
          </Section>
        );
      })}

      {/* Sign out, the common one, alone; Delete account in a card of its own under it, with what it does. */}
      <Section>
        <Row label="Sign out" tone="danger" center onPress={signOut} />
      </Section>
      <Section footer="Deletes your account and its data for good.">
        <Row label="Delete account" tone="danger" href="/delete-account" />
      </Section>
      <Text style={[styles.version, { color: theme.tertiary }]} selectable>{`Nexus ${Constants.expoConfig?.version ?? ""}`}</Text>
    </>
  );

  return (
    // One page for both, so the field keeps the keyboard while its results take the list's place.
    <Page>
      <LargeTitle>Settings</LargeTitle>
      <SettingsSearch query={query} onChange={setQuery} />
      {matches ? results : list}
      {orgs ? (
        <OrgSheet
          visible={picking}
          onClose={() => setPicking(false)}
          orgs={orgs}
          recent={switcher.recent}
          switching={switcher.switching}
          problem={switcher.problem}
          onPick={switcher.pick}
        />
      ) : null}
    </Page>
  );
}

const styles = StyleSheet.create({
  // The app and its version, quiet, at the foot: About has it too, with the policies.
  version: { ...text.caption, fontFamily: font.regular, textAlign: "center", fontVariant: ["tabular-nums"], marginTop: -6 },
});
