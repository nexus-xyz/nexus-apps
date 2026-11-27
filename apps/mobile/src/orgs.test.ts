import { describe, expect, it } from "vitest";
import { orderOrgs, orgLabel, orgSwitchKey } from "./orgs";

describe("organizations", () => {
  const orgs = [
    { id: "t2", name: "Zeta", slug: "zeta", kind: "company" as const },
    { id: "t1", name: "joao", kind: "personal" as const },
    { id: "t3", name: "Acme", kind: "company" as const },
  ];

  it("lists Personal first, then by name, and switches by slug, else id", () => {
    expect(orderOrgs(orgs).map(orgLabel)).toEqual(["Personal", "Acme", "Zeta"]);
    expect(orderOrgs(orgs).map(orgSwitchKey)).toEqual(["personal", "t3", "zeta"]);
  });
});
