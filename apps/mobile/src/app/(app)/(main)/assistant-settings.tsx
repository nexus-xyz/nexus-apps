import type { Href } from "expo-router";
import * as WebBrowser from "expo-web-browser";
import { useState } from "react";
import { assistantFooter, assistantSections } from "@/assistant-settings";
import { assistantSoundsOn, setAssistantSounds } from "@/assistant-sounds";
import { useAuth } from "@/auth";
import { Blank, Footnote, Page, Row, Section } from "@/list";
import { Switch } from "@/switch";

/**
 * Account › Assistant (A4·B): the Assistant's settings as a list under Account, under the scope's name, one row per
 * section with no icon, each opening its page (`assistant-settings.ts`). A section with no page yet stays in its
 * place, dimmed. Approvals opens Nexus UI's Assistant page in the in-app browser, as the app opens other
 * pages that live only on the web. Under them, the chat's send and receive sounds, kept on this phone
 * (`assistant-sounds.ts`).
 */
export default function AssistantSettings() {
  const { agent, uiLink, orgName } = useAuth();
  const [problem, setProblem] = useState<string>();
  const [sounds, setSounds] = useState(assistantSoundsOn);
  if (!agent) return <Blank title="Nexus is not available here" body="This build of the app is not connected to the agent service." />;
  const sections = assistantSections(uiLink);
  const openWeb = async (url: string, label: string) => {
    setProblem(undefined);
    try {
      await WebBrowser.openBrowserAsync(url);
    } catch {
      setProblem(`${label} could not be opened. Try again.`);
    }
  };
  return (
    <Page>
      {/* Discover (A15): what it can do, and how to let it do more, above the settings. */}
      <Section>
        <Row label="Discover" detail="What it can do, and how to let it do more" detailTone="tertiary" href="/discover" />
      </Section>
      <Section title={orgName} footer={assistantFooter(sections)}>
        {sections.map((section) =>
          section.href ? (
            <Row key={section.label} label={section.label} detail={section.detail} detailTone="tertiary" href={section.href as Href} />
          ) : section.url ? (
            <Row
              key={section.label}
              label={section.label}
              detail={section.detail}
              detailTone="tertiary"
              chevron
              onPress={() => void openWeb(section.url, section.label)}
              accessibilityRole="link"
              accessibilityHint="Opens it in Nexus on the web"
            />
          ) : (
            <Row key={section.label} label={section.label} detail={section.detail} detailTone="tertiary" disabled />
          ),
        )}
      </Section>
      {problem ? <Footnote tone="danger">{problem}</Footnote> : null}
      <Section title="Chat" footer="A quiet sound when you send and when Nexus answers, kept on this phone. The ring switch silences them too.">
        <Row
          label="Sounds"
          accessory={
            <Switch
              value={sounds}
              accessibilityLabel="Sounds"
              onValueChange={(value) => {
                setAssistantSounds(value);
                setSounds(value);
              }}
            />
          }
        />
      </Section>
    </Page>
  );
}
