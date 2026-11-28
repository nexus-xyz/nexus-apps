import { CoreError, MAX_REPORTED_ANSWER_CHARS, type AnswerReportReason, type ChatSessionMessage, type Feedback } from "@nexus/kit";
import type { Device } from "./diagnostics";

/**
 * Report an answer (App Review 1.2): a person reports a model's answer to Nexus's team. The Gateway posts it to
 * the feedback channel in Slack (`POST /v1/diagnostics/feedback`, category `report`), with the reason, the model, the
 * answer as the app showed it, and the person's name, email and organization.
 */

export const REASONS: { id: AnswerReportReason; label: string }[] = [
  { id: "offensive", label: "Offensive or harmful" },
  { id: "wrong", label: "Wrong or misleading" },
  { id: "other", label: "Something else" },
];

/** What the sheet says goes with a report. */
export const WHAT_IS_SENT =
  "Nexus's team gets this answer, the model that wrote it, and your name, email and organization. The report stays in Nexus's feedback channel until the team deletes it, also after you delete the chat.";

/** A model's finished answer with words: not a question, not one still streaming, not a failure's message. */
export function reportable(m: Pick<ChatSessionMessage, "id" | "role" | "content" | "status">): boolean {
  return m.role === "assistant" && m.id !== "answer" && m.status === "complete" && !!m.content.trim();
}

/** An answer's text as a report carries it: trimmed, and cut to the Gateway's bound in characters (code points). */
export function reportedText(text: string): string {
  return Array.from(text.trim()).slice(0, MAX_REPORTED_ANSWER_CHARS).join("").trim();
}

/** The body of `POST /v1/diagnostics/feedback` for a report. The phone sends no note and no screen. */
export function reportBody(answer: Pick<ChatSessionMessage, "content" | "model">, reason: AnswerReportReason, device: Device): Feedback {
  return {
    category: "report",
    message: "",
    screen: null,
    appVersion: device.appVersion,
    appBuild: device.appBuild ?? null,
    platform: device.platform,
    osVersion: device.osVersion,
    answer: { reason, text: reportedText(answer.content), model: answer.model || null },
  };
}

/** The words for a failed report: the Gateway's 503 is a channel no one set up, its 429 a limit; else try again. */
export function reportFailure(error: unknown): string {
  if (error instanceof CoreError && error.status === 503) return "Reports are not set up on this server yet.";
  if (error instanceof CoreError && error.status === 429) return "You sent a lot of reports just now. Try again in a few minutes.";
  return "The report did not send. Try again.";
}
