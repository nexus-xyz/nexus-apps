import { router } from "expo-router";
import { useState } from "react";
import { Alert, Platform, StyleSheet, View } from "react-native";
import { useAuth } from "@/auth";
import { TextInput } from "@/controls";
import { Icon } from "@/icon";
import { Blank, Busy, Empty, Footnote, Initial, Loading, Page, Row, Section } from "@/list";
import { pickerSections, roleLine, SEARCH_FROM } from "@/org-picker";
import { useOrgs } from "@/org-switcher";
import { orgLabel } from "@/orgs";
import { font, text, useTheme } from "@/theme";
import { useSwitchOrg } from "@/use-switch-org";

const ANDROID = Platform.OS === "android";

/**
 * Organizations, as Search finds it: the same list as Settings' sheet (`org-sheet.tsx`), the ones you were in last and
 * then all of them, with a search from seven on. A tap switches and comes back. Sign out of everything ends the session
 * on this phone.
 */
export default function Organizations() {
  const theme = useTheme();
  const { signOut } = useAuth();
  const { orgs, current, error } = useOrgs();
  const switcher = useSwitchOrg(current);
  const [query, setQuery] = useState("");

  const signOutAll = () =>
    Alert.alert("Sign out of everything?", "Every organization signs out on this phone. Your other devices stay signed in.", [
      { text: "Cancel", style: "cancel" },
      { text: "Sign out", style: "destructive", onPress: () => void signOut() },
    ]);

  if (!orgs) return error ? <Blank title="Couldn't load your organizations" body={error} tone="danger" /> : <Loading alone />;
  const sections = pickerSections(orgs, query, switcher.recent);
  return (
    <Page>
      {orgs.length >= SEARCH_FROM ? (
        <View style={[styles.search, { backgroundColor: theme.fill }]}>
          <Icon name="search" size={15} color={theme.tertiary} />
          <TextInput
            style={[styles.searchText, { color: theme.foreground }]}
            value={query}
            onChangeText={setQuery}
            placeholder={`Search ${orgs.length} organizations`}
            placeholderTextColor={theme.tertiary}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="search"
            clearButtonMode="while-editing"
            accessibilityLabel="Search organizations"
          />
        </View>
      ) : null}
      {switcher.problem ? <Footnote tone="danger">{switcher.problem}</Footnote> : null}
      {sections.map((section) => (
        <Section key={section.key} title={section.title}>
          {section.orgs.length ? (
            section.orgs.map((org) => (
              <Row
                key={org.id}
                leading={<Initial name={orgLabel(org)} size={30} round={org.kind === "personal"} logo={org.kind === "personal" ? undefined : org.imageUrl} />}
                label={orgLabel(org)}
                detail={roleLine(org) || undefined}
                selected={!!org.current}
                accessory={switcher.switching === org.id ? <Busy /> : undefined}
                onPress={() => void switcher.pick(org).then((done) => done && router.back())}
                accessibilityState={{ selected: !!org.current, busy: switcher.switching === org.id }}
              />
            ))
          ) : (
            <Empty>No organization matches “{query.trim()}”.</Empty>
          )}
        </Section>
      ))}
      <Section>
        <Row label="Sign out of everything" tone="danger" center onPress={signOutAll} />
      </Section>
    </Page>
  );
}

const styles = StyleSheet.create({
  search: { flexDirection: "row", alignItems: "center", gap: 6, minHeight: 36, paddingHorizontal: 10, borderRadius: 10, borderCurve: "continuous" },
  searchText: { flex: 1, ...text.secondary, fontFamily: font.regular, paddingVertical: ANDROID ? 4 : 8 },
});
