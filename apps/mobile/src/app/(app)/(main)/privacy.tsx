import { useFocusEffect } from "expo-router";
import { useCallback, useState } from "react";
import { Alert } from "react-native";
import { keptConsent } from "@/consent-sheet";
import { Switch } from "@/controls";
import { Page, Row, Section } from "@/list";
import { errorReportsOn, setErrorReports } from "@/reporting";

/**
 * Privacy, under You (the desktop's Privacy row): the permission to send chats to AI providers, to withdraw (App
 * Review 5.1.1(ii)), and the switch for error and crash reports. Moved here from Settings' foot.
 */
export default function Privacy() {
  // Whether chats may go to AI providers, read again each time the page shows: the chat's question sets it.
  const [aiAllowed, setAiAllowed] = useState(false);
  useFocusEffect(useCallback(() => setAiAllowed(!!keptConsent.load()?.length), []));
  const withdrawAi = () =>
    Alert.alert("Withdraw permission?", "Nexus sends nothing to an AI provider until you allow it again. It asks before your next message.", [
      { text: "Cancel", style: "cancel" },
      {
        text: "Withdraw",
        style: "destructive",
        onPress: () => {
          const saved = keptConsent.withdraw();
          setAiAllowed(false);
          if (!saved) Alert.alert("Couldn't save the change", "Nexus asks before your next message. If you restart the app, withdraw again here.");
        },
      },
    ]);

  // Error and crash reports, on until the person turns them off. Off deletes the reports the app kept.
  const [reports, setReports] = useState(errorReportsOn);
  const switchReports = (on: boolean) => {
    const saved = setErrorReports(on);
    setReports(errorReportsOn());
    if (!saved)
      Alert.alert(
        "Couldn't save the change",
        on ? "Reports stay off. Try again." : "Nexus sends no reports until the app restarts. After a restart, turn reports off again here.",
      );
  };

  return (
    <Page>
      <Section
        footer={aiAllowed ? "Tap to withdraw. Nexus then asks before your next message." : "Nexus asks before it sends a chat to an AI provider."}
      >
        <Row
          icon="brain"
          bareIcon
          label="AI providers"
          value={aiAllowed ? "Allowed" : "Asks first"}
          onPress={aiAllowed ? withdrawAi : undefined}
          accessibilityHint={aiAllowed ? "Withdraws your permission to send chats to AI providers" : undefined}
        />
      </Section>
      <Section footer="What failed and where in the app's code, never your chats. Off deletes the reports not sent yet.">
        <Row
          icon="pulse"
          bareIcon
          label="Send crash and error reports"
          accessory={<Switch value={reports} onValueChange={switchReports} accessibilityLabel="Send crash and error reports" />}
        />
      </Section>
    </Page>
  );
}
