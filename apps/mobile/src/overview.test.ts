import { describe, expect, it } from "vitest";
import { productLine, sharingRows } from "./overview";

describe("Gateway overview", () => {
  it("gives each product its one line", () => {
    expect(productLine("core", [])).toMatch(/^The control plane/);
    expect(productLine("review", [{ capability: { slug: "pr", name: "PR review", description: "Reviews pull requests." } } as never])).toBe("Reviews pull requests.");
  });

  it("reads sharing outside in desktop's words, defaults included", () => {
    expect(sharingRows({ "artifacts.external": "off" })).toEqual([{ label: "Sharing outside", value: "Off" }]);
    const rows = sharingRows({ "artifacts.external.domains": { allow: ["acme.example"], deny: [] }, "artifacts.external.expiry": { maxDays: 30, required: true } });
    expect(rows.map((r) => r.value)).toEqual([
      "On, with public links",
      "Everyone in the organization",
      "Readers outside may, if the sharer allows",
      "acme.example",
      "Within 30 days, always",
    ]);
  });
});
