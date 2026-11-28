import Constants from "expo-constants";
import { useAuth } from "@/auth";
import { SITE_PAGES } from "@/links";
import { Page, Row, Section } from "@/list";
import { open } from "@/markdown";

/**
 * About, under App: the site's support, privacy and terms pages in the in-app browser (App Review 5.1.1(i)), when the
 * sign-in expires, and the app's version. Moved here from Settings' foot.
 */
export default function About() {
  const { expiresAt } = useAuth();
  return (
    <Page>
      <Section>
        <Row icon="support" bareIcon label="Support" onPress={() => open(SITE_PAGES.support)} accessibilityRole="link" accessibilityHint="Opens nexus.inc" />
        <Row icon="privacy" bareIcon label="Privacy policy" onPress={() => open(SITE_PAGES.privacyPolicy)} accessibilityRole="link" accessibilityHint="Opens nexus.inc" />
        <Row icon="terms" bareIcon label="Terms of service" onPress={() => open(SITE_PAGES.terms)} accessibilityRole="link" accessibilityHint="Opens nexus.inc" />
      </Section>
      <Section>
        {expiresAt ? <Row label="Signed in until" value={new Date(expiresAt).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" })} /> : null}
        <Row label="Version" value={Constants.expoConfig?.version ?? ""} />
      </Section>
    </Page>
  );
}
