import { CoreError, type Feedback, type FeedbackCategory } from "@nexus/kit";
import type { Device } from "./diagnostics";

/**
 * Send feedback from the phone (the form of Nexus UI's `feedback.tsx`): what the page offers and sends. The
 * Gateway posts it to Nexus's feedback channel in Slack with the person's name, email and organization.
 */

/** What a person writes from the form: a crash and a report on an answer are sent from elsewhere. */
export type Kind = Exclude<FeedbackCategory, "crash" | "report" | "triage">;

export const KINDS: { id: Kind; label: string }[] = [
  { id: "bug", label: "Bug" },
  { id: "idea", label: "Idea" },
  { id: "other", label: "Other" },
];

export const PLACEHOLDER: Record<Kind, string> = {
  bug: "What happened, and what did you expect?",
  idea: "What would make Nexus better for you?",
  other: "What's on your mind?",
};

/** The Gateway's bound (`MAX_FEEDBACK_CHARS`). */
export const MAX_CHARS = 4_000;
/** The counter shows once the message nears its bound. */
export const COUNT_FROM = 3_500;

/** The body of `POST /v1/diagnostics/feedback`. The phone sends no screen: the page is opened from Account. */
export function feedbackBody(kind: Kind, message: string, device: Device): Feedback {
  return {
    category: kind,
    message: message.trim(),
    screen: null,
    appVersion: device.appVersion,
    appBuild: device.appBuild ?? null,
    platform: device.platform,
    osVersion: device.osVersion,
  };
}

/** The words for a failed send: the Gateway's 503 is a channel no one set up, its 429 a limit; else try again. */
export function failure(error: unknown): string {
  if (error instanceof CoreError && error.status === 503) return "Feedback is not set up on this server yet.";
  if (error instanceof CoreError && error.status === 429) return "You sent a lot of feedback just now. Try again in a few minutes.";
  return "It did not send. Nothing is lost: try again.";
}
