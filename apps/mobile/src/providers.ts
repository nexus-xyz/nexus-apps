import { FAMILIES, PROVIDERS } from "./generated/providers";

export type Provider = { id: string; name: string; hex?: string; path?: string };

const FAMILY = FAMILIES.map(([pattern, provider]) => [new RegExp(pattern), provider] as const);

/** The provider of a `provider/model` id, of a bare model name from a known family, or the id itself (Nexus UI's `providerOf`). */
export function providerOf(model: string): string {
  const id = model.toLowerCase();
  const slash = id.indexOf("/");
  if (slash > 0) return id.slice(0, slash);
  return FAMILY.find(([family]) => family.test(id))?.[1] ?? id;
}

/** How a provider is drawn: its mark and colour (no `hex`: the foreground's), or (no `path`) a neutral initial chip; `nexus` for Auto. */
export function provider(id: string): Provider {
  if (id === "nexus") return { id, name: "Nexus" };
  return PROVIDERS.find((p) => p.id === id) ?? { id, name: id.charAt(0).toUpperCase() + id.slice(1) };
}

/** Providers in Nexus UI's order (its PROVIDERS table), then any others by name. */
export const providerRank = (id: string) => {
  const at = PROVIDERS.findIndex((p) => p.id === id);
  return at < 0 ? PROVIDERS.length : at;
};
