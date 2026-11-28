import { ARTIFACT_SETTING_DEFAULTS, type ArtifactSharingSettings, type ModuleManifest, type OrgSettings } from "@nexus/kit";

// Copies of Nexus UI's overview copy (`catalog.ts`'s one-liners, `external-sharing.tsx`'s labels), beside
// Next-only imports; move them into @nexus/kit if a third client needs them.

/** A product's one line on the overview: the catalog's, else its first capability's description. */
export function productLine(slug: string, capabilities: Pick<ModuleManifest, "capability">[]): string | undefined {
  const lines: Record<string, string> = {
    core: "The control plane of your factory: any model through one gateway, on your own keys, under one set of controls.",
  };
  return lines[slug] ?? capabilities[0]?.capability.description;
}

/** Sharing outside the organization as rows to read, in desktop's words; the defaults for any setting left out. */
export function sharingRows(answer: Partial<OrgSettings> | undefined): { label: string; value: string }[] {
  const s: ArtifactSharingSettings = { ...ARTIFACT_SETTING_DEFAULTS };
  for (const [key, value] of Object.entries(answer ?? {})) if (key.startsWith("artifacts.") && value != null) (s as unknown as Record<string, unknown>)[key] = value;
  const external = { public: "On, with public links", named: "Named people only", off: "Off" }[s["artifacts.external"]];
  if (s["artifacts.external"] === "off") return [{ label: "Sharing outside", value: external }];
  const { allow, deny } = s["artifacts.external.domains"];
  const { maxDays, required } = s["artifacts.external.expiry"];
  return [
    { label: "Sharing outside", value: external },
    { label: "Who may share outside", value: s["artifacts.external.who"] === "admins" ? "Owners and admins only" : "Everyone in the organization" },
    { label: "Continuing a chat", value: s["artifacts.external.continue"] ? "Readers outside may, if the sharer allows" : "Not outside" },
    ...(allow.length ? [{ label: "Only these domains", value: allow.join(", ") }] : []),
    ...(deny.length ? [{ label: "Never these domains", value: deny.join(", ") }] : []),
    ...(s["artifacts.external"] === "public"
      ? [{ label: "Public links expire", value: maxDays ? `Within ${maxDays} days${required ? ", always" : ""}` : required ? "Always, at the sharer's choice" : "When the sharer chooses" }]
      : []),
  ];
}
