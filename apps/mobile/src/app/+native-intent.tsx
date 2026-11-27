import { Platform } from "react-native";
import { isSignInReturn } from "@/app-sign-in";
import { keepConnectionIntent } from "@/connection-intent";

/**
 * Links the system hands the app, before the router sees them. The authentication session handles sign-in returns
 * so the app stays on the current screen.
 */
export function redirectSystemPath({ path }: { path: string; initial: boolean }) {
  if (isSignInReturn(path)) return null;
  if (Platform.OS === "android" && /^[a-z][a-z0-9+.-]*:\/\/expo-sharing\/?(?:[?#]|$)/i.test(path)) {
    return `/incoming-share?share=${Date.now()}`;
  }
  // Keep the secret out of navigation state. The connection screen validates the invitation.
  if (/^(?:(?:nexus|xyz\.nexus\.app):\/\/connect|exps?:\/\/[^/?#]+\/(?:[^?#]*\/)?connect|\/?connect)\/?#invite=/i.test(path)) {
    keepConnectionIntent(/^\/?connect/i.test(path) ? `nexus://connect${path.slice(path.indexOf("#"))}` : path);
    return "/connect";
  }
  return path;
}
