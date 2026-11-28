import { Redirect, useLocalSearchParams } from "expo-router";
import { useAuth } from "@/auth";
import SignIn from "@/app/sign-in";

/**
 * A shared link: `https://<Nexus UI>/a/<key>` tapped in Slack, Mail or Messages, or `nexus://a/<key>` from
 * the viewer's "Open in the app". Outside the sign-in guard, so a link opened signed out signs in here first and then
 * opens. The artifact screen reads the key: an artifact's id, or a public link's token.
 */
export default function SharedLink() {
  const { key } = useLocalSearchParams<{ key: string }>();
  const { core } = useAuth();
  if (!core) return <SignIn />;
  return <Redirect href={{ pathname: "/artifact/[id]", params: { id: key } }} />;
}
