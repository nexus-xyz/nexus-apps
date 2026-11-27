import { describe, expect, it } from "vitest";
import { addressOf, domainLine, ownerName } from "./general";

describe("general", () => {
  it("writes the address with the slug, else the id", () => {
    expect(addressOf({ id: "ten_1", slug: "acme" })).toBe("/app/acme");
    expect(addressOf({ id: "ten_1" })).toBe("/app/ten_1");
  });

  it("says whether a domain is verified and what waits on it", () => {
    expect(domainLine({ verified: true, pendingInvitations: 0 })).toBe("Verified");
    expect(domainLine({ verified: false, pendingInvitations: 1 })).toBe("Not verified · 1 invitation waiting");
    expect(domainLine({ verified: true, pendingInvitations: 3 })).toBe("Verified · 3 invitations waiting");
  });

  it("names the owner by name, then email", () => {
    expect(ownerName([{ userId: "u1", role: "admin" }, { userId: "u2", role: "owner", email: "maya@acme.example" }])).toBe("maya@acme.example");
    expect(ownerName([{ userId: "u1", role: "member" }])).toBeUndefined();
  });
});
