/** A local Gateway preview uses its own sign-in and device keys. No credential is bundled with Metro. */
export const DEVELOPMENT_GATEWAY = typeof __DEV__ !== "undefined" && __DEV__ ? process.env.EXPO_PUBLIC_NEXUS_DEV_GATEWAY?.replace(/\/$/, "") : undefined;
export const DEVELOPMENT_NAMESPACE = DEVELOPMENT_GATEWAY ? ".local-code" : "";
