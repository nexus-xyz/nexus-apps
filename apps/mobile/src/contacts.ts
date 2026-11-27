import { Contact, ContactField, getPermissionsAsync, requestPermissionsAsync } from "expo-contacts";
import { contactMatches, type ContactMatch } from "@/contact-text";

/** Whether Nexus may read the contacts: asked yet or not, and the answer. iOS 18's limited access counts as granted. */
export type ContactsAccess = "granted" | "denied" | "undetermined";

const FIELDS = [ContactField.FULL_NAME, ContactField.GIVEN_NAME, ContactField.FAMILY_NAME, ContactField.PHONES] as const;

/** The access Nexus has now. With `ask`, the system's prompt shows first when it was never asked. */
export async function contactsAccess(ask: boolean): Promise<ContactsAccess> {
  try {
    const now = ask ? await requestPermissionsAsync() : await getPermissionsAsync();
    return now.granted ? "granted" : now.canAskAgain ? "undetermined" : "denied";
  } catch {
    return "denied";
  }
}

/**
 * The contacts that match `to`, read on the device: nothing leaves the phone. A full name the
 * device's search misses is searched again by its first word.
 */
export async function findContacts(to: string): Promise<ContactMatch[]> {
  const search = async (name: string) => contactMatches(await Contact.getAllDetails(FIELDS, { name, limit: 25 }), to);
  const found = await search(to);
  const first = to.trim().split(/\s+/)[0];
  if (found.length || !first || first === to.trim()) return found;
  return await search(first);
}
