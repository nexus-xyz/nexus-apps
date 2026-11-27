import Constants from "expo-constants";
import type { BuildHosts } from "./links";

/** The host names this build was given (`extra.hosts`, set by `app.config.ts`); none when the build named none. */
export const BUILD_HOSTS: BuildHosts = (Constants.expoConfig?.extra?.hosts as BuildHosts | undefined) ?? {};
