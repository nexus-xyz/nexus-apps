/**
 * What device management pins for the app, read by `modules/managed-config`: on iOS, MDM AppConfig under
 * the `UserDefaults` key `com.apple.configuration.managed`; on Android, the managed configuration
 * (`RestrictionsManager`). One key, `org`: the organization to work in, a slug or an id. The app switches to it after
 * sign-in. Device management cannot pin the Gateway; a person may still choose another server on the sign-in screen.
 */
export type ManagedConfig = { org?: string };

/**
 * The pinned organization from what the native module read: `{}` without one, as in Expo Go. A blank or non-text value
 * is ignored.
 */
export function readManaged(raw: unknown): ManagedConfig {
  const org = raw && typeof raw === "object" ? (raw as Record<string, unknown>).org : undefined;
  return typeof org === "string" && org.trim() ? { org: org.trim() } : {};
}

/** Whether a sign-in must move to the pinned organization: one is pinned, and the tenant is not it by slug or id. */
export const needsManagedSwitch = (pinned: string | undefined, tenant: { id: string; slug?: string | null }) =>
  !!pinned && pinned !== tenant.id && pinned !== tenant.slug;
