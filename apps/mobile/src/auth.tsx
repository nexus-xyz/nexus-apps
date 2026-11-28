import { createAgentClient, createArtifactsClient, createCoreClient, type AgentClient } from "@nexus/kit";
import * as Crypto from "expo-crypto";
import * as SecureStore from "expo-secure-store";
import * as WebBrowser from "expo-web-browser";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Platform } from "react-native";
import { agentsApiUrl, agentTarget } from "./agent-chat";
import {
  accountOf,
  APP_REDIRECT,
  appRedirect,
  authorizeUrl,
  base64Url,
  bytesToBase64Url,
  NO_ANSWER,
  type Provider,
  readCallback,
  SIGN_IN_PROFILE_MS,
  SIGN_IN_STEP_MS,
  withDeadline,
} from "./app-sign-in";
import { BUILD_HOSTS } from "./build-hosts";
import { forgetImages } from "./chat-images";
import { forgetDeviceKey } from "./device-key";
import { DEVELOPMENT_GATEWAY, DEVELOPMENT_NAMESPACE } from "./development";
import { DEFAULT_GATEWAY, movedSignIn, uiOrigin, uiPage } from "./links";
import { useManagedConfig } from "./managed";
import { needsManagedSwitch } from "./managed-config";
import { accountKey, shareInbox } from "./share-inbox-native";
import { forgetAll, forgetCaches, load, remove, save } from "./store";

const uiOf = (saved: Saved) => saved.ui ?? saved.gateway;
const CLIENT = "mobile/0.1.1";
/** What a sign-in calls the app: its token's label, and what Nexus UI's device page shows. */
const LABEL = Platform.OS === "android" ? "Nexus for Android" : Platform.OS === "ios" && Platform.isPad ? "Nexus for iPad" : "Nexus for iPhone";
const KEY = `nexus.auth${DEVELOPMENT_NAMESPACE}`;
// After first unlock, this device only, never in a backup. No `requireAuthentication`: app.json drops the Face ID usage
// string (`faceIDPermission: false`), so a read that asks for Face ID needs that string back, with its reason.
const STORE = { keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY };
const EXPIRED = "Your sign-in expired or was revoked. Sign in again.";
/**
 * The Assistant's API (S2.5), from the build's config: undefined, the agent is not available in this build. Read whole,
 * so Expo writes the value into the bundle.
 */
const AGENTS_API_URL = agentsApiUrl(process.env.EXPO_PUBLIC_NEXUS_AGENTS_API_URL);
/** `on`: the Assistant calls the signed-in Gateway's `/v1/agent/*`, not `AGENTS_API_URL`. Off by default. */
const AGENT_VIA_GATEWAY = process.env.EXPO_PUBLIC_NEXUS_AGENT_VIA_GATEWAY;

type Core = ReturnType<typeof createCoreClient>;
type Artifacts = ReturnType<typeof createArtifactsClient>;
type SignInStatus = { kind: "idle" } | { kind: "browser" | "finishing"; provider: Provider };
/**
 * What a sign-in keeps with its token. All but `gateway` and `token` came later; a sign-in saved before gets `org` and
 * `name` from Gateway. `name` is the person's, for a new chat's greeting: "" when the account has none. `user` is the
 * person's id, which binds what they share to Nexus to them (`share-inbox.ts`): "" when Gateway did not say.
 */
type Saved = { gateway: string; token: string; ui?: string; org?: string; orgName?: string; name?: string; user?: string; expiresAt?: string };

/** Whose the sign-in is, from Gateway's `me()`. */
const personOf = (me: Parameters<typeof accountOf>[0]) => ({ ...accountOf(me), user: me.user?.id ?? "" });

/** A fetch that reports a 401 on the token (expired, revoked), for every client of one sign-in. */
const watching = (onUnauthorized?: () => void): typeof fetch | undefined =>
  onUnauthorized
    ? async (input, init) => {
        const response = await fetch(input, init);
        if (response.status === 401) onUnauthorized();
        return response;
      }
    : undefined;

/**
 * A client for the Gateway; with `onUnauthorized`, any 401 on the token (expired, revoked) reports itself. Every write
 * carries an `Idempotency-Key`, and every edit of an editable resource its `If-Match`.
 */
const connect = (gateway: string, token?: string, onUnauthorized?: () => void) =>
  createCoreClient({ baseUrl: gateway, apiKey: token, client: CLIENT, idempotency: true, ifMatch: true, fetch: watching(onUnauthorized) });

interface Auth {
  /** Redeems an approved, single-use device grant against the configured local Gateway, in development only. */
  connectDevelopment: (deviceCode: string) => Promise<void>;
  /** `undefined` while the Keychain is read, `null` when signed out. */
  core: Core | null | undefined;
  /** Sharing, artifacts and exports (the kit's artifacts client), on the same sign-in. */
  artifacts: Artifacts | null | undefined;
  /**
   * The Assistant's API (S2.5) on the same sign-in; `null` while signed out or when this build has no agent API. Its 401
   * does not sign you out: the agent API checks the token with its own Gateway, which may not be the one you use.
   */
  agent: AgentClient | null;
  /** The signed-in Gateway's URL. */
  gateway?: string;
  /** The organization's name ("Personal" for yours) and when the token expires, when known. */
  /** The organization's slug (else id), as Nexus UI's paths name it. */
  org?: string;
  orgName?: string;
  /** The person's name, once Gateway has said it ("" when the account has none). */
  name?: string;
  expiresAt?: string;
  /** Nexus UI's address (where the device sign-in's approval page was, else the Gateway's). */
  ui?: string;
  /** A Nexus UI page of the signed-in organization (`/app/<org><path>`), once the organization is known. */
  uiLink?: (path: string) => string;
  /** Why the app signed out on its own, shown on the sign-in screen. */
  notice: string | null;
  /** The key the Share extension records with an import (`accountKey`), once the person's id is known. */
  shareAccount?: string;
  signInStatus: SignInStatus;
  /**
   * Continue with Apple, Google, or email in the system's sign-in sheet.
   * Closing the sheet leaves the person signed out and free to try again.
   */
  signIn(provider: Provider): Promise<void>;
  signOut(): Promise<void>;
  /** Another of your organizations (`personal`, a slug or an id): a token for it, and the old one revoked. */
  switchOrg(key: string): Promise<void>;
}

const AuthContext = createContext<Auth | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [saved, setSaved] = useState<Saved | null | undefined>(undefined);
  const [notice, setNotice] = useState<string | null>(null);
  const managed = useManagedConfig();

  // Stable (they touch only the store and state setters), so the effects and the memo below may use them freely.
  const keep = useCallback(async (next: Saved) => {
    await SecureStore.setItemAsync(KEY, JSON.stringify(next), STORE);
    setSaved(next);
  }, []);

  // A sign-in saved on a retired Gateway host is kept on production's. A Keychain read that fails, or a
  // saved value that does not parse, shows the sign-in screen: the app never stays on a blank screen.
  useEffect(() => {
    SecureStore.getItemAsync(KEY, STORE)
      .then((raw) => {
        const stored = raw ? (JSON.parse(raw) as Saved) : null;
        const moved = stored && movedSignIn(stored, BUILD_HOSTS);
        if (moved) keep(moved).catch(() => setSaved(moved));
        else setSaved(stored);
      })
      .catch(() => setSaved(null));
  }, [keep]);
  const forget = useCallback(async (why: string | null) => {
    await SecureStore.deleteItemAsync(KEY, STORE);
    // What the app kept (caches, drafts, reading places, images read, what was shared to Nexus) is the person's: it
    // goes with the sign-in.
    forgetAll();
    forgetImages();
    await shareInbox.setAccount(null);
    await shareInbox.clear();
    setSaved(null);
    setNotice(why);
  }, []);
  // A ref, so the client (memoised on `saved`) always reaches the current `forget`.
  const unauthorized = useRef(() => {});
  unauthorized.current = () => void forget(EXPIRED);

  /**
   * Keeps a new sign-in, first moved to the organization device management pins, when it is another. When
   * the switch fails (the person is not a member, Gateway refuses), the sign-in stays in its own organization.
   *
   * It asks Gateway whose the sign-in is before it keeps it. Kept without the names, the first screen showed
   * without them, and the names came one `me()` later: a second save, which loaded that screen again.
   */
  const keepSignIn = useCallback(
    async (next: Saved, tenant: { id: string; slug?: string | null }) => {
      const pinned = managed.org;
      const answer =
        pinned && needsManagedSwitch(pinned, tenant)
          ? await withDeadline(connect(next.gateway, next.token).switchOrg(pinned), SIGN_IN_STEP_MS, NO_ANSWER).catch(() => null)
          : null;
      let signedIn = next;
      if (answer) {
        const { org, orgName } = accountOf(answer);
        signedIn = { ...next, token: answer.token, org, orgName, expiresAt: answer.expiresAt };
      }
      const me = await withDeadline(connect(signedIn.gateway, signedIn.token).me(), SIGN_IN_PROFILE_MS, NO_ANSWER).catch(() => null);
      // Without `me()` (Gateway did not answer), the effect below asks again once the sign-in is kept.
      await keep(me ? { ...signedIn, ...personOf(me) } : signedIn);
      // The first token would stay valid until it expires; nothing uses it any more.
      if (answer) void connect(next.gateway, next.token).logout().catch(() => undefined);
    },
    [keep, managed.org],
  );

  // A sign-in from before `org`, `name` or `user` was saved, or one without the organization's name (the device grant's
  // tenant doesn't say which is Personal): ask Gateway once which organization the token is for, and whose it is.
  useEffect(() => {
    if (!saved || (saved.org && saved.orgName !== undefined && saved.name !== undefined && saved.user !== undefined)) return;
    let current = true;
    connect(saved.gateway, saved.token, () => {
      if (current) unauthorized.current();
    })
      .me()
      .then((me) => {
        if (current) return keep({ ...saved, ...personOf(me) });
      })
      .catch(() => undefined);
    return () => { current = false; };
  }, [saved, keep]);

  // The Share extension records who was signed in with each import; signed out, it records no one.
  const [shareAccount, setShareAccount] = useState<string>();
  const keychainRead = saved !== undefined;
  const sharingGateway = saved?.gateway;
  const sharingUser = saved?.user;
  useEffect(() => {
    if (!keychainRead) return;
    if (!sharingGateway || !sharingUser) {
      setShareAccount(undefined);
      if (!sharingGateway) void shareInbox.setAccount(null);
      return;
    }
    let current = true;
    accountKey(sharingGateway, sharingUser).then((key) => {
      if (!current) return;
      setShareAccount(key);
      void shareInbox.setAccount(key);
    }, () => undefined);
    return () => { current = false; };
  }, [keychainRead, sharingGateway, sharingUser]);

  // The clients follow the Gateway and the token only, so a save of the names alone does not make new clients, and
  // the screens that load on `core` do not load again.
  const loaded = saved !== undefined;
  const signedGateway = saved?.gateway;
  const signedToken = saved?.token;
  const core = useMemo(
    () => (!loaded ? undefined : signedGateway && signedToken ? connect(signedGateway, signedToken, () => unauthorized.current()) : null),
    [loaded, signedGateway, signedToken],
  );
  const artifacts = useMemo(
    () =>
      !loaded
        ? undefined
        : signedGateway && signedToken
          ? createArtifactsClient({ baseUrl: signedGateway, apiKey: signedToken, ifMatch: true, fetch: watching(() => unauthorized.current()) })
          : null,
    [loaded, signedGateway, signedToken],
  );

  const agent = useMemo(() => {
    const target = agentTarget(AGENT_VIA_GATEWAY, AGENTS_API_URL, signedGateway);
    return signedToken && target ? createAgentClient({ ...target, apiKey: signedToken, client: CLIENT }) : null;
  }, [signedToken, signedGateway]);

  const connectDevelopment = useCallback(async (deviceCode: string) => {
    if (!DEVELOPMENT_GATEWAY) throw new Error("A local development Gateway is not configured.");
    const granted = await connect(DEVELOPMENT_GATEWAY).pollDeviceLogin(deviceCode);
    const me = await connect(DEVELOPMENT_GATEWAY, granted.access_token).me();
    await keep({ gateway: DEVELOPMENT_GATEWAY, token: granted.access_token, ...personOf(me), expiresAt: granted.expires_at });
  }, [keep]);

  const [signInStatus, setSignInStatus] = useState<SignInStatus>({ kind: "idle" });
  const signingIn = useRef(false);
  const signIn = useCallback(async (provider: Provider) => {
    if (signingIn.current) return;
    signingIn.current = true;
    setNotice(null);
    setSignInStatus({ kind: "browser", provider });
    try {
      const gateway = DEFAULT_GATEWAY;
      const verifier = bytesToBase64Url(Crypto.getRandomBytes(32));
      const challenge = base64Url(
        await Crypto.digestStringAsync(Crypto.CryptoDigestAlgorithm.SHA256, verifier, { encoding: Crypto.CryptoEncoding.BASE64 }),
      );
      const state = bytesToBase64Url(Crypto.getRandomBytes(16));
      const redirect = appRedirect(Platform.OS, Platform.Version);
      const result = await WebBrowser.openAuthSessionAsync(
        authorizeUrl(gateway, { client: LABEL, redirect, challenge, state, provider }),
        redirect,
        { preferEphemeralSession: true, preferUniversalLinks: redirect !== APP_REDIRECT },
      ).catch(() => {
        throw new Error("Couldn't open sign-in. Try again.");
      });
      if (result.type === "cancel" || result.type === "dismiss") return;
      if (result.type !== "success") throw new Error("Couldn't open sign-in. Try again.");
      const code = readCallback(result.url, state, redirect);
      setSignInStatus({ kind: "finishing", provider });
      const granted = await withDeadline(
        connect(gateway).exchangeAppCode(code, verifier, redirect).catch(() => {
          throw new Error("Couldn't complete sign-in. Check your connection and try again.");
        }),
        SIGN_IN_STEP_MS,
        NO_ANSWER,
      );
      await keepSignIn(
        {
          gateway,
          token: granted.access_token,
          ui: uiOrigin(gateway, granted.ui_url),
          org: granted.tenant.slug || granted.tenant.id,
          // Optional at runtime: `me()` fills it in when the exchange leaves it out.
          user: granted.user?.id,
          expiresAt: granted.expires_at,
        },
        granted.tenant,
      );
    } finally {
      signingIn.current = false;
      setSignInStatus({ kind: "idle" });
    }
  }, [keepSignIn]);

  const value = useMemo<Auth>(
    () => ({
      connectDevelopment,
      core,
      artifacts,
      agent,
      gateway: saved?.gateway,
      org: saved?.org,
      orgName: saved?.orgName,
      name: saved?.name,
      expiresAt: saved?.expiresAt,
      ui: saved ? uiOf(saved) : undefined,
      uiLink: saved?.org ? (path) => uiPage(uiOf(saved), saved.org!, path) : undefined,
      notice,
      shareAccount,
      signInStatus,
      signIn,
      async signOut() {
        // Revoke the token on the Gateway; sign out locally even when it cannot be reached.
        await (saved && connect(saved.gateway, saved.token).logout().catch(() => undefined));
        // The device key for synced sessions goes too: the next person to sign in here pairs the phone afresh.
        await forgetDeviceKey().catch(() => undefined);
        await forget(null);
      },
      async switchOrg(key) {
        if (!saved || !core) return;
        const answer = await core.switchOrg(key);
        forgetCaches();
        forgetImages();
        await keep({
          ...saved,
          token: answer.token,
          // The new organization's, so every `useStored` key moves to its namespace with the switch.
          org: answer.tenant.slug || answer.tenant.id,
          orgName: answer.tenant.kind === "personal" ? "Personal" : answer.tenant.name,
          expiresAt: answer.expiresAt,
        });
        // The old token would stay valid until it expires; nothing uses it any more.
        await connect(saved.gateway, saved.token).logout().catch(() => undefined);
      },
    }),
    [core, artifacts, agent, saved, notice, shareAccount, signInStatus, signIn, keep, forget, connectDevelopment],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): Auth {
  const auth = useContext(AuthContext);
  if (!auth) throw new Error("useAuth outside AuthProvider");
  return auth;
}

/** The signed-in client, for screens behind the sign-in guard. */
export function useCore(): Core {
  const { core } = useAuth();
  if (!core) throw new Error("useCore while signed out");
  return core;
}

/** The signed-in artifacts client, for screens behind the sign-in guard. */
export function useArtifacts(): Artifacts {
  const { artifacts } = useAuth();
  if (!artifacts) throw new Error("useArtifacts while signed out");
  return artifacts;
}

/** One stored value of the signed-in organization: `kind` is cache, draft, place or choice. */
export function useStored<T>(kind: "cache" | "draft" | "place" | "choice", name: string) {
  const { org } = useAuth();
  // `org` is kept by AuthProvider at sign-in (`org: granted.tenant.slug || granted.tenant.id`), on a switch
  // (`org: answer.tenant.slug || answer.tenant.id`), and from Gateway's `me()` on start for a sign-in saved before it
  // was. Until it is known nothing is read or written, so no value lands in a namespace shared across organizations.
  const key = org ? `${kind}:${org}:${name}` : undefined;
  return useMemo(
    () => ({
      load: () => (key ? load<T>(key) : undefined),
      save: (value: T) => {
        if (key) save(key, value);
      },
      remove: () => {
        if (key) remove(key);
      },
    }),
    [key],
  );
}
