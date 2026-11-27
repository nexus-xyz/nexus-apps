import { connectorName, safeUrl, writeConsentOf, type AgentMessage, type CoreClient, type WriteConsent } from "@nexus/kit";

/**
 * The write consent prompt's logic, as Nexus UI's `write-consent-prompt.tsx` has it: Gateway
 * refused a connector write with 403 `write_consent_required`, as the connection was consented for reading only. The
 * thread asks "<Connector> needs your permission to write"; Continue makes the call Gateway named
 * (`POST /v1/connections/{connector}/oauth` with `access: "write"`) and opens the vendor's `authorizeUrl`. The write
 * is not sent again: asking again is the person's to do. Gateway sends this refusal only while
 * `NEXUS_CONNECTOR_WRITE_CONSENT` is on.
 */

export const WRITE_CONSENT_FAILED = "Nexus could not open the permission page. Try again in a moment.";

/** The consent a refusal carries, read strictly; undefined for any other message. */
export function messageConsent(message: AgentMessage): WriteConsent | undefined {
  return message.kind === "refused" ? writeConsentOf(message.consent) : undefined;
}

/** The prompt's title: "<Connector> needs your permission to write". */
export function writeConsentTitle(consent: Pick<WriteConsent, "connector">): string {
  return `${connectorName(consent.connector)} needs your permission to write`;
}

/** What Continue does, in one line. */
export function writeConsentLine(consent: Pick<WriteConsent, "connector" | "account">): string {
  return `${consent.account} was connected to read only. Continue to ${connectorName(consent.connector)} to allow writes, then ask again.`;
}

/** The scopes it asks for, or undefined when Gateway named none. */
export function writeConsentScopes(consent: Pick<WriteConsent, "scopes">): string | undefined {
  return consent.scopes.length ? `It asks for: ${consent.scopes.join(", ")}` : undefined;
}

/** The vendor's consent page, from the call Gateway named; undefined when the call failed or answered no web page. */
export async function consentPage(core: Pick<CoreClient, "startOAuth">, consent: WriteConsent): Promise<string | undefined> {
  try {
    return safeUrl((await core.startOAuth(consent.connector, consent.consent.body.scope, "write")).authorizeUrl);
  } catch {
    return undefined;
  }
}
