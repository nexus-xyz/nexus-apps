/**
 * Which models the picker shows first: the newest of each line, the rest under "Other models". A line is the model's
 * name without its version numbers ("Claude Opus 5.5" → "Claude Opus", "GPT-6.1 Sol" → "GPT Sol"), so Opus 4.8 sits
 * under Opus 5.5 while GPT-6 Luna, Sol and Astra (tiers of one family) each stay. The catalog has no "current" flag.
 * `keep` names what stays in front however old (Auto and the organization's default, in the picker).
 */
type Named = { id: string; name: string };

const VERSION = /\d+(?:\.\d+)*/g;
const line = (name: string) => name.replace(VERSION, "").replace(/[-\s]+/g, " ").trim().toLowerCase();
const version = (name: string) => (name.match(VERSION) ?? []).join(".").split(".").map(Number);

/** a > b by version, segment by segment (6.1 > 6, 5.5 > 5 > 4.8). */
function newer(a: number[], b: number[]) {
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const d = (a[i] ?? 0) - (b[i] ?? 0);
    if (d) return d > 0;
  }
  return false;
}

export function splitCurrent<T extends Named>(models: T[], keep: (m: T) => boolean): { current: T[]; other: T[] } {
  const newest = new Map<string, T>();
  for (const m of models) {
    const at = newest.get(line(m.name));
    if (!at || newer(version(m.name), version(at.name))) newest.set(line(m.name), m);
  }
  const current: T[] = [];
  const other: T[] = [];
  for (const m of models) (newest.get(line(m.name)) === m || keep(m) ? current : other).push(m);
  return { current, other };
}

/** A context window as the model sheet's rows say it (P32·F): "200k", "1M". */
export function contextLabel(tokens?: number): string | undefined {
  if (!tokens) return undefined;
  if (tokens >= 1_000_000) return `${+(tokens / 1_000_000).toFixed(1)}M`;
  return `${Math.round(tokens / 1000)}k`;
}

/** The models whose name or id has every word of the query, in their order (the sheet's search, P32·F). */
export function findModels<T extends Named>(models: T[], query: string): T[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return words.length ? models.filter((m) => words.every((w) => `${m.name} ${m.id}`.toLowerCase().includes(w))) : models;
}

/**
 * The few models the sheet's first view lists (P32·F: Auto, then three, then All models): the one in use, the
 * organization's default, then the latest of each line in their order.
 */
export function shortList<T extends Named>(latest: T[], selected: string, isDefault: (m: T) => boolean, size = 3): T[] {
  const first = [latest.find((m) => m.id === selected), latest.find(isDefault)].filter((m): m is T => !!m);
  return [...new Set([...first, ...latest])].slice(0, size);
}
