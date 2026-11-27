import { router } from "expo-router";
import { useEffect } from "react";
import { Text, View, StyleSheet } from "react-native";
import { useCore } from "./auth";
import { NavRow, SessionMark } from "./nav-row";
import { sessionDetail, sessionHref, sessionKey, sessionTitle, useSessionHistory } from "./session-history";
import { font, text, useTheme } from "./theme";
export function HistorySuggestions() {
  const core = useCore();
  const history = useSessionHistory(core, 6);
  const theme = useTheme();
  useEffect(history.refresh, [history.refresh]);
  if (!history.sessions.length) return null;
  return <View style={styles.list}><Text accessibilityRole="header" style={[styles.title, { color: theme.muted }]}>Pick up where you left off</Text>
    {history.sessions.slice(0, 3).map((session) => <NavRow key={sessionKey(session)} leading={<SessionMark />} label={sessionTitle(session, history.titles)} detail={sessionDetail(session, history.titles)} onPress={() => router.replace(sessionHref(session))} />)}
  </View>;
}
const styles = StyleSheet.create({ list: { marginHorizontal: 16, marginBottom: 12 }, title: { ...text.caption, fontFamily: font.medium, marginHorizontal: 12, marginBottom: 4 } });
