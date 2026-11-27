/** The first word of the account's name ("Daniel Marin" is "Daniel"); null when there is no name, or it is an email. */
export function firstName(name: string | null | undefined): string | null {
  const first = name?.trim().split(/\s+/)[0] ?? "";
  return first && !first.includes("@") ? first : null;
}

/** The sidebar's avatar: the first letters of the name's first and last words ("Daniel Marin" is "DM"); null as above. */
export function initials(name: string | null | undefined): string | null {
  if (!firstName(name)) return null;
  const words = name!.trim().split(/\s+/);
  return (words[0].charAt(0) + (words.length > 1 ? words[words.length - 1].charAt(0) : "")).toUpperCase();
}

/** The empty New session board uses the phone's local time and the person's first name. */
export function greeting(name: string | null | undefined, now = new Date()): string {
  const first = firstName(name);
  const hour = now.getHours();
  const salutation = hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";
  return first ? `${salutation}, ${first}` : salutation;
}
