import { Redirect } from "expo-router";
import { useAuth, useStored } from "@/auth";
import { connectionIntent } from "@/connection-intent";
import { GUEST_ENABLED } from "@/guest";
import { LAST_SESSION_PLACE, launchSession, type SessionPlace } from "@/session-place";

export default function Index() {
  const { core } = useAuth();
  const previous = useStored<SessionPlace>("place", LAST_SESSION_PLACE);
  const pairing = connectionIntent();
  // A computer pairing link needs an account even when guest mode is enabled.
  if (!core) return <Redirect href={GUEST_ENABLED && !pairing ? "/guest" : "/sign-in"} />;
  return <Redirect href={pairing ? "/connect" : launchSession(previous.load())} />;
}
