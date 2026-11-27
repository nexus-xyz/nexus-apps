import type { PolicyTestResponse } from "@nexus/kit";

/** Where a policy decision came from, as desktop's `ruleRef` says it. */
export function ruleRef(rule: PolicyTestResponse["rule"]): string {
  if (rule.level === "none") return "no rule restricts it";
  if (rule.level === "catalog") return "the model catalog";
  const where = rule.level === "organization" ? "organization" : rule.level === "deployment" ? "this Gateway deployment" : `${rule.level}${rule.subject ? ` ${rule.subject}` : ""}`;
  return `${where}${rule.list ? ` · ${rule.list} list` : ""}${rule.pattern ? ` · ${rule.pattern}` : ""}`;
}
