import type { CurrentTenantResponse, GithubAppStatus, RepositoriesResponse } from "@nexus/kit";
import { Stack } from "expo-router";
import { useCallback, useEffect, useState } from "react";
import { Platform, Pressable, StyleSheet } from "react-native";
import { ago } from "@/ago";
import { useCore } from "@/auth";
import { Icon } from "@/icon";
import { Blank, Empty, Footnote, LargeTitle, Loading, Page, Row, Section } from "@/list";
import { open } from "@/markdown";
import { useTheme } from "@/theme";
/** How long ago, in the row's words: "2m ago", "just now", or the date past a month. */
const since = (iso: string) => {
  const short = ago(iso);
  return short === "now" ? "just now" : /^\d+[mhdw]$/.test(short) ? `${short} ago` : short;
};

/**
 * Repositories, as brand v5's P71d·R4 draws it on the phone: the title in the list, the Nexus GitHub App (the accounts
 * it is installed on and how many repositories it links), then the linked repositories. A repository opens its own page
 * (`repository/[id]`): its details and Open on GitHub. The bar's + connects more on GitHub's own page, for owners and
 * admins. The board's team per repository and its Removed state are left out: Gateway records neither. Setting the App
 * up stays on a computer.
 */
export default function Repositories() {
  const core = useCore();
  const theme = useTheme();
  const [app, setApp] = useState<GithubAppStatus | null>(null);
  const [linked, setLinked] = useState<RepositoriesResponse | null>(null);
  const [me, setMe] = useState<CurrentTenantResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    setError(null);
    core.githubApp().then(setApp, (e: Error) => setError(e.message));
    core.repositories().then(setLinked, (e: Error) => setError(e.message));
    core.me().then(setMe, () => undefined);
  }, [core]);
  useEffect(load, [load]);

  const manage = !!me?.permissions?.includes("repositories.manage");
  const title = <Stack.Screen options={{ headerTitle: "", headerBackButtonDisplayMode: "default" }} />;

  if (!app || !linked) return error ? <Blank title="Repositories did not load" body={error} tone="danger" /> : <Loading alone />;
  if (!app.configured) {
    return (
      <Blank
        title="The Nexus GitHub App is not set up on this Gateway yet"
        body={manage ? "Set it up once in Nexus for desktop or on the web, then connect repositories." : "Ask your organization's owner or an admin to set it up."}
      />
    );
  }

  const accounts = linked.installations.map((i) => i.account);
  const count = linked.repositories.length;
  // One account: the board's short names, the account named once on the App's row.
  const short = accounts.length === 1;
  const connect = manage && app.installUrl ? () => open(app.installUrl!) : undefined;
  return (
    <Page onRefresh={load}>
      {title}
      {connect ? (
        <Stack.Screen
          options={{
            headerRight: () => (
              <Pressable onPress={connect} accessibilityRole="button" accessibilityLabel="Connect repositories on GitHub" hitSlop={12} style={Platform.OS === "android" ? styles.headerButton : undefined}>
                <Icon name="plus" color={theme.foreground} />
              </Pressable>
            ),
          }}
        />
      ) : null}
      <LargeTitle>Repositories</LargeTitle>
      {error ? <Footnote tone="danger">{error}</Footnote> : null}
      <Section>
        <Row
          label="Nexus GitHub App"
          detail={accounts.length ? `${accounts.join(", ")} · ${count} ${count === 1 ? "repository" : "repositories"}` : "On no GitHub account yet"}
          value={accounts.length ? "Connected" : undefined}
        />
      </Section>
      <Section>
        {count === 0 ? (
          <Empty>
            No repositories linked.{" "}
            {manage ? "Connect repositories on GitHub with +." : "An owner or admin connects repositories."}
          </Empty>
        ) : null}
        {linked.repositories.map((repo) => (
          <Row
            key={repo.id}
            label={short ? repo.fullName.slice(repo.fullName.indexOf("/") + 1) : repo.fullName}
            monoLabel
            detail={[repo.private ? "Private" : "Public", `linked ${since(repo.linkedAt)}`].join(" · ")}
            href={{ pathname: "/repository/[id]", params: { id: repo.id } }}
          />
        ))}
      </Section>
    </Page>
  );
}

const styles = StyleSheet.create({
  // iOS draws the bar button's 44 pt itself; Android's header gives only the icon's 20 dp.
  headerButton: { width: 48, height: 48, alignItems: "center", justifyContent: "center" },
});
