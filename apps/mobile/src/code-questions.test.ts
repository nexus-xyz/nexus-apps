import { describe, expect, it } from "vitest";
import { answerReceipt, answerValues, codeAnswers, codeQuestions } from "./code-questions";

describe("answers sent back to the coding agent", () => {
  it("uses Claude's question text and Codex's stable id in a grouped answer", () => {
    const questions = codeQuestions({ questions: [
      { question: "Which branch?", options: [{ label: "main" }, { label: "dev" }] },
      { id: "checks", question: "Which checks?", multiSelect: true, options: [{ label: "Types" }, { label: "Tests" }] },
    ] });
    const choices = { "Which branch?": { selected: ["main"], own: "my-branch" }, checks: { selected: ["Types", "Tests"], own: "Lint" } };
    expect(codeAnswers(questions, choices)).toEqual({ "Which branch?": "my-branch", checks: ["Types", "Tests", "Lint"] });
    expect(answerValues(questions[0]!, undefined)).toEqual([]);
  });

  it("keeps a private answer out of the event-log receipt", () => {
    const questions = codeQuestions({ questions: [{ id: "password", question: "Enter the password", isSecret: true, options: [] }] });
    const choices = { password: { selected: [], own: "do-not-log-this" } };
    expect(codeAnswers(questions, choices)).toEqual({ password: ["do-not-log-this"] });
    expect(answerReceipt(questions, choices)).toBe("Private answer submitted");
  });

  it("leaves a malformed question unavailable rather than making a bare Allow button", () => {
    expect(codeQuestions(null)).toEqual([]);
    expect(codeQuestions({ questions: [{ question: 3 }, null] })).toEqual([]);
  });
});
