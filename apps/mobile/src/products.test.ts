import { describe, expect, it } from "vitest";
import { duration, githubRepo, percent, productName, repoName } from "./products";

describe("products", () => {
  it("names products as desktop's sidebar does", () => {
    expect(productName({ slug: "intelligence", name: "Nexus Intelligence" })).toBe("Intelligence");
    expect(productName({ slug: "core", name: "Nexus Core" })).toBe("Gateway");
  });

  it("reads a GitHub repository", () => {
    expect(repoName("https://github.com/acme/api.git")).toBe("acme/api");
    expect(repoName("anything else")).toBe("anything else");
    expect(githubRepo("https://github.com/acme/api/")).toBe("https://github.com/acme/api");
    expect(githubRepo("https://gitlab.com/acme/api")).toBeUndefined();
  });

  it("formats coverage and duration", () => {
    expect(percent(0.0123)).toBe("1.2%");
    expect(percent(0.456)).toBe("46%");
    expect(duration("2026-10-22T10:00:00Z", "2026-10-22T10:00:42Z")).toBe("42s");
    expect(duration("2026-10-22T10:00:00Z", "2026-10-22T10:04:12Z")).toBe("4m 12s");
    expect(duration("2026-10-22T10:00:00Z", "2026-10-22T11:03:00Z")).toBe("1h 3m");
  });
});
