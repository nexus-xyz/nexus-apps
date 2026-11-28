import { Redirect, useLocalSearchParams } from "expo-router";
export default function LegacyCodeSession() {
  const { threadId } = useLocalSearchParams<{ threadId: string }>();
  return <Redirect href={{ pathname: "/session/[id]", params: { id: threadId, source: "synced" } }} />;
}
