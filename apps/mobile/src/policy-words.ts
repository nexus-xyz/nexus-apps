import type { ModelRule } from "@nexus/kit";
import { provider } from "./providers";

/**
 * A model rule in the plain sentences brand v5's P71a reads on the phone ("Only Anthropic and OpenAI models", "Never
 * DeepSeek V4", "Up to $15 per M output"), grouped as the board groups them. Read-only: rules change on a computer.
 */

/** A pattern as a person reads it: `anthropic/*` is "Anthropic models", a model id its name. */
export function patternName(pattern: string, nameOf: (id: string) => string): string {
  const every = /^([a-z0-9-]+)\/\*$/i.exec(pattern);
  if (every) return `${provider(every[1].toLowerCase()).name} models`;
  return pattern.includes("*") ? pattern : nameOf(pattern);
}

/** "A", "A and B", "A, B and C". */
export function listOf(names: string[]): string {
  return names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

const price = (usd: number) => `$${Number.isInteger(usd) ? usd : usd.toFixed(2)}`;

/** One rule as a phone row and its page (P71a·H, P105·D's "Price cap"): what the rule is called, and its sentence. */
export type PolicyItem = { title: string; text: string };

/** A rule's items, grouped as the board groups them: the models it allows, then where calls may go. */
export function policyItems(rule: ModelRule, nameOf: (id: string) => string): { models: PolicyItem[]; where: PolicyItem[] } {
  // Every pattern a provider's: "Anthropic, OpenAI and Google models", not "models" after each name.
  const names = (patterns: string[]) => {
    const providers = patterns.map((p) => /^([a-z0-9-]+)\/\*$/i.exec(p)?.[1]);
    if (providers.every(Boolean)) return `${listOf(providers.map((id) => provider(id!.toLowerCase()).name))} models`;
    return listOf(patterns.map((p) => patternName(p, nameOf)));
  };
  const models: PolicyItem[] = [];
  if (rule.allow?.length) models.push({ title: "Allowed models", text: `Only ${names(rule.allow)}` });
  if (rule.deny?.length) models.push({ title: "Blocked models", text: `Never ${names(rule.deny)}` });
  const caps = [rule.maxPrice?.input !== undefined ? `${price(rule.maxPrice.input)} per M input` : undefined, rule.maxPrice?.output !== undefined ? `${price(rule.maxPrice.output)} per M output` : undefined].filter(Boolean);
  if (caps.length) models.push({ title: "Price cap", text: `Up to ${caps.join(" and ")}` });
  if (rule.defaults?.chat) models.push({ title: "Chat default", text: `Chats start on ${nameOf(rule.defaults.chat)}` });
  if (!models.length) models.push({ title: "Every model", text: "Every model the catalog lists" });
  const where: PolicyItem[] = [];
  if (rule.providers?.allow?.length) where.push({ title: "Allowed providers", text: `Only through ${listOf(rule.providers.allow.map((p) => provider(p).name))}` });
  if (rule.providers?.deny?.length) where.push({ title: "Blocked providers", text: `Never through ${listOf(rule.providers.deny.map((p) => provider(p).name))}` });
  if (rule.denyCountries?.maker?.length) where.push({ title: "Model makers", text: `No models made in ${listOf(rule.denyCountries.maker)}` });
  if (rule.denyCountries?.provider?.length) where.push({ title: "Provider countries", text: `No providers in ${listOf(rule.denyCountries.provider)}` });
  return { models, where };
}

/** Whether a rule says anything of its own; an empty one leaves the catalog and the rules above it in effect. */
export const hasRules = (rule: ModelRule) =>
  !!(rule.allow?.length || rule.deny?.length || rule.maxPrice || rule.defaults?.chat || rule.providers?.allow?.length || rule.providers?.deny?.length || rule.denyCountries?.maker?.length || rule.denyCountries?.provider?.length);

export function policySentences(rule: ModelRule, nameOf: (id: string) => string): { models: string[]; where: string[] } {
  const { models, where } = policyItems(rule, nameOf);
  return { models: models.map((i) => i.text), where: where.map((i) => i.text) };
}
