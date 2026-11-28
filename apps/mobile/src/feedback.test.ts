import { CoreError } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { failure, feedbackBody } from "./feedback";

const phone = { appVersion: "1.4.0", appBuild: "212", platform: "ios" as const, osVersion: "26.1" };

describe("feedback from the phone", () => {
  it("sends the trimmed words and the phone's versions, with no screen", () => {
    expect(feedbackBody("idea", "  dark mode for pages \n", phone)).toEqual({
      category: "idea",
      message: "dark mode for pages",
      screen: null,
      appVersion: "1.4.0",
      appBuild: "212",
      platform: "ios",
      osVersion: "26.1",
    });
    expect(feedbackBody("bug", "x", { ...phone, appBuild: undefined }).appBuild).toBeNull();
  });

  it("names why a send failed", () => {
    expect(failure(new CoreError(503, "not configured", "not_configured"))).toMatch(/not set up/);
    expect(failure(new CoreError(429, "limit", "feedback_limit"))).toMatch(/few minutes/);
    expect(failure(new CoreError(502, "upstream", "upstream_error"))).toMatch(/try again/);
    expect(failure(new TypeError("Network request failed"))).toMatch(/try again/);
  });
});
