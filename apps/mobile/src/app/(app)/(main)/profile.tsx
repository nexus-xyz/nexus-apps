import type { CurrentTenantResponse } from "@nexus/kit";
import { useEffect, useState } from "react";
import { StyleSheet, Text, View } from "react-native";
import { useAuth, useCore } from "@/auth";
import { initials } from "@/greeting";
import { Page, Row, Section } from "@/list";
import { font, useTheme } from "@/theme";

/**
 * Your profile, pushed from Account (brand v5's P68c on the phone): your photo's place leading, then your name and
 * email, and the time zone and language this phone uses. Read on the phone: your name and photo change in Nexus UI.
 * The design's "Your data and account" (export, leave, delete) waits on the Gateway, which has none of the three.
 */
export default function Profile() {
  const core = useCore();
  const theme = useTheme();
  const { name } = useAuth();
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  useEffect(() => {
    core.me().then(setMe, () => undefined);
  }, [core]);
  const person = me?.user?.name || name || "";
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const locale = Intl.DateTimeFormat().resolvedOptions().locale;
  // Hermes may not have DisplayNames: the locale's own tag then.
  const language = typeof Intl.DisplayNames === "function" ? (new Intl.DisplayNames([locale], { type: "language" }).of(locale) ?? locale) : locale;
  return (
    <Page>
      <View style={styles.head} accessible accessibilityLabel={person || "You"}>
        <View style={[styles.avatar, { backgroundColor: theme.fill }]}>
          <Text style={[styles.letters, { color: theme.muted }]} allowFontScaling={false}>{initials(person) ?? "?"}</Text>
        </View>
      </View>
      <Section footer="Change your name and photo in Nexus UI. Time zone and language follow this phone.">
        <Row label="Name" value={person || "Not set"} />
        {me?.user?.email ? <Row label="Email" value={me.user.email} /> : null}
        <Row label="Time zone" value={zone} />
        {language ? <Row label="Language" value={language} /> : null}
      </Section>
    </Page>
  );
}

const styles = StyleSheet.create({
  // The board's lead (P68c): a 72 pt photo, centred, 16 pt over the rows.
  head: { alignItems: "center", paddingTop: 4, paddingBottom: 8 },
  avatar: { width: 72, height: 72, borderRadius: 36, alignItems: "center", justifyContent: "center" },
  letters: { fontFamily: font.medium, fontSize: 28, lineHeight: 34 },
});
