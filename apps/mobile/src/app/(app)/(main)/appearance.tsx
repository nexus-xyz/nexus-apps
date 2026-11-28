import { useEffect, useState } from "react";
import { AccessibilityInfo, Platform, useWindowDimensions } from "react-native";
import { Inset, Page, Row, Section, Segmented } from "@/list";
import { keptAppearance, setAppearance, type AppearanceChoice } from "@/theme";
import { hapticsEnabled, setHapticsEnabled } from "@/haptics";
import { Switch } from "@/switch";

// The board's order (P68·F): System, then Dark, then Light.
const THEMES: { id: AppearanceChoice; label: string }[] = [
  { id: "unspecified", label: "System" },
  { id: "dark", label: "Dark" },
  { id: "light", label: "Light" },
];

/** The text size the phone is set to, in the words Settings uses for its slider's ends. */
const textSize = (fontScale: number) => (fontScale > 1.05 ? "Larger" : fontScale < 0.95 ? "Smaller" : "Default");

/**
 * Preferences, in Global (brand v5's P68·F on the phone): the theme, kept on this phone, then what this phone's own
 * settings set and the app follows, text size and reduce motion. The design's "Everywhere you sign in" (focus when a
 * session starts, autocomplete, Return) waits on the Gateway, which keeps no preferences across devices.
 */
export default function Preferences() {
  const [look, setLook] = useState(keptAppearance);
  const [feedback, setFeedback] = useState(hapticsEnabled);
  const { fontScale } = useWindowDimensions();
  const [reduceMotion, setReduceMotion] = useState<boolean | null>(null);
  useEffect(() => {
    AccessibilityInfo.isReduceMotionEnabled().then(setReduceMotion, () => undefined);
    const changed = AccessibilityInfo.addEventListener("reduceMotionChanged", setReduceMotion);
    return () => changed.remove();
  }, []);
  const phone = Platform.OS === "ios" ? "iPhone" : "phone";
  return (
    <Page>
      <Section title="Appearance" footer="Nexus UI and the desktop app keep their own.">
        <Inset>
          <Segmented
            label="Theme"
            options={THEMES}
            value={look}
            onChange={(value) => {
              setAppearance(value);
              setLook(value);
            }}
          />
        </Inset>
      </Section>
      <Section title="Feedback" footer="Touch feedback for actions in Nexus, kept on this phone. System controls follow your device settings.">
        <Row
          label="Haptic feedback"
          accessory={
            <Switch
              value={feedback}
              accessibilityLabel="Haptic feedback"
              onValueChange={(value) => {
                setHapticsEnabled(value);
                setFeedback(value);
              }}
            />
          }
        />
      </Section>
      <Section title={`This ${phone}`} footer={`Set in the ${phone}'s own Settings, under Accessibility. Nexus follows them.`}>
        <Row label="Text size" value={textSize(fontScale)} />
        <Row label="Reduce motion" value={reduceMotion === null ? "System" : reduceMotion ? "On" : "Off"} />
      </Section>
    </Page>
  );
}
