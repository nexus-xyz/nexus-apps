import { describe, expect, it } from "vitest";
import { needsManagedSwitch, readManaged } from "./managed-config";

describe("managed app configuration", () => {
  it("reads the pinned organization", () => {
    expect(readManaged({ org: " acme " })).toEqual({ org: "acme" });
  });

  it("is empty without a native module or the key", () => {
    expect(readManaged(null)).toEqual({});
    expect(readManaged(undefined)).toEqual({});
    expect(readManaged({})).toEqual({});
  });

  it("ignores a blank or non-text value, and any other key", () => {
    expect(readManaged({ org: "  " })).toEqual({});
    expect(readManaged({ org: ["acme"] })).toEqual({});
    expect(readManaged({ gatewayURL: "https://nexus.acme.example" })).toEqual({});
  });
});

describe("the pinned organization", () => {
  it("switches only when the tenant is neither the pinned slug nor the pinned id", () => {
    expect(needsManagedSwitch(undefined, { id: "t_1", slug: "acme" })).toBe(false);
    expect(needsManagedSwitch("acme", { id: "t_1", slug: "acme" })).toBe(false);
    expect(needsManagedSwitch("t_1", { id: "t_1", slug: "acme" })).toBe(false);
    expect(needsManagedSwitch("acme", { id: "t_2", slug: "" })).toBe(true);
    expect(needsManagedSwitch("acme", { id: "t_2", slug: null })).toBe(true);
  });
});
