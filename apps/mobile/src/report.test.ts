import { CoreError, MAX_REPORTED_ANSWER_CHARS } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { reportable, reportBody, reportedText, reportFailure } from "./report";

const phone = { appVersion: "1.4.0", appBuild: "212", platform: "ios" as const, osVersion: "26.1" };
const answer = { id: "m_1", role: "assistant" as const, content: "  The plan, in three steps.\n", status: "complete" as const, model: "anthropic/claude-sonnet-5-5" };

describe("report an answer", () => {
  it("sends the answer, its model and the reason as the category report", () => {
    expect(reportBody(answer, "offensive", phone)).toEqual({
      category: "report",
      message: "",
      screen: null,
      appVersion: "1.4.0",
      appBuild: "212",
      platform: "ios",
      osVersion: "26.1",
      answer: { reason: "offensive", text: "The plan, in three steps.", model: "anthropic/claude-sonnet-5-5" },
    });
    expect(reportBody({ content: "x" }, "other", { ...phone, appBuild: undefined })).toMatchObject({ appBuild: null, answer: { model: null } });
  });

  it("cuts a long answer to the Gateway's bound in characters, not UTF-16 units", () => {
    const long = "😀".repeat(MAX_REPORTED_ANSWER_CHARS + 5);
    expect(Array.from(reportedText(long))).toHaveLength(MAX_REPORTED_ANSWER_CHARS);
  });

  it("reports only a model's finished answer with words", () => {
    expect(reportable(answer)).toBe(true);
    expect(reportable({ ...answer, role: "user" })).toBe(false);
    expect(reportable({ ...answer, id: "answer" })).toBe(false);
    expect(reportable({ ...answer, status: "failed" })).toBe(false);
    expect(reportable({ ...answer, content: " \n" })).toBe(false);
  });

  it("names why a report failed", () => {
    expect(reportFailure(new CoreError(503, "not configured", "not_configured"))).toMatch(/not set up/);
    expect(reportFailure(new CoreError(429, "limit", "feedback_limit"))).toMatch(/few minutes/);
    expect(reportFailure(new TypeError("Network request failed"))).toMatch(/Try again/);
  });
});
