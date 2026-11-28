import { agentProblem, safeUrl, turnAnswer, type AgentFailure, type AgentMessage, type AgentProblem, type AgentTurn } from "@nexus/kit";
import { usd } from "./meta";

const LOCAL_HTTP = /^http:\/\/(localhost|127\.0\.0\.1|\[::1\]|10\.\d+\.\d+\.\d+|192\.168\.\d+\.\d+|172\.(1[6-9]|2\d|3[01])\.\d+\.\d+)(:\d+)?(\/|$)/i;

/**
 * The Assistant on the phone (Agent Interfaces S2.5), without React: where the agent API is, the line under an answer,
 * the words for a failure, and the reply entries. The wire types, the stream and the client are the kit's
 * (`kit/src/agent-chat.ts`), which Nexus UI (S2.4) and desktop (S2.3) read too.
 */

/**
 * The agent API's base URL, from the build's `EXPO_PUBLIC_NEXUS_AGENTS_API_URL` (an EAS environment variable, or `.env`
 * in development): https, or plain http only to this machine or a private network, as for a Gateway. Unset or unsafe,
 * the screen says the agent is not available here.
 */
export function agentsApiUrl(raw: string | undefined): string | undefined {
  const url = raw?.trim().replace(/\/+$/, "");
  return url && (/^https:\/\/[^/]/i.test(url) || LOCAL_HTTP.test(url)) ? url : undefined;
}

/**
 * Where the Assistant's client calls, from the build's config: through the signed-in Gateway's `/v1/agent/*` when
 * `EXPO_PUBLIC_NEXUS_AGENT_VIA_GATEWAY` is `on`, else the agent API at `agentsApi`, as before. Undefined:
 * the agent is not available in this build (or nobody is signed in, through Gateway).
 */
export function agentTarget(viaGateway: string | undefined, agentsApi: string | undefined, gateway: string | undefined): { baseUrl: string; viaGateway?: true } | undefined {
  if (viaGateway?.trim().toLowerCase() === "on") return gateway ? { baseUrl: gateway, viaGateway: true } : undefined;
  return agentsApi ? { baseUrl: agentsApi } : undefined;
}

/** What the screen says when an ask or a read fails. The sign-in is the app's: it signs in again in Settings. */
export function phoneProblem(failure: AgentFailure): AgentProblem {
  if (failure.status === 401) return { title: "Sign in again", detail: "Nexus refused this sign-in. Sign out in Account, sign in again, then ask again.", signIn: true };
  const { title, detail } = agentProblem(failure.status, failure);
  return { title, detail };
}

/** A failure the API did not send: the network, or the agent service that does not answer. */
export const UNREACHABLE: AgentProblem = { title: "Nexus did not answer", detail: "The app could not reach Nexus. Check the connection, then try again." };
/** A stream that ended before its last event. */
export const CUT: AgentProblem = { title: "The answer stopped before it finished", detail: "Ask again to get a new answer." };

/**
 * The line under an answer: the model, why it ran, the cost, the notes it used and the conversation's total, as Nexus
 * UI's `answerFooter`, with this app's model names and money.
 */
export function answerLine(message: Extract<AgentMessage, { kind: "answer" }>, name: (model: string) => string): string[] {
  const choice = message.choice;
  const why = choice?.default ? "your default" : choice?.keyword === "fast" ? "you asked for fast" : choice?.keyword === "best" ? "you asked for best" : choice?.keyword === "model" ? "you picked it" : undefined;
  const total = message.conversation && message.conversation.answers > 1 ? `${usd(message.conversation.costUsd)} in this conversation` : undefined;
  return [
    message.model ? name(message.model) : undefined,
    why,
    Number.isFinite(message.costUsd) ? usd(message.costUsd) : undefined,
    message.notesUsed ? `${message.notesUsed} ${message.notesUsed === 1 ? "note" : "notes"} about you` : undefined,
    total,
  ].filter((part): part is string => Boolean(part));
}

/** An answer's sources that the app may open: web links only, each once, titled by their URL when they have no title. */
export function sourcesOf(message: Extract<AgentMessage, { kind: "answer" }>): { title: string; url: string }[] {
  const seen = new Set<string>();
  return (message.sources ?? []).flatMap((source) => {
    const url = safeUrl(source?.url);
    if (!url || seen.has(url)) return [];
    seen.add(url);
    return [{ title: source.title?.trim() || url, url }];
  });
}

/**
 * One entry of the conversation on screen: your question, or the agent's reply (one or more messages). `readBack`: a
 * reply read back from the agent API, which shows no line under it.
 */
export type Entry = { id: string; role: "user"; text: string } | { id: string; role: "agent"; messages: AgentMessage[]; readBack?: true };

/**
 * Restore complete messages when available. Legacy turns omit answer metadata, so their footer stays hidden.
 */
export const entriesOf = (turns: AgentTurn[]): Entry[] =>
  turns.map((turn, i): Entry => (turn.role === "user" ? { id: `h${i}`, role: "user", text: turn.text } : { id: `h${i}`, role: "agent", messages: turn.messages ?? [turnAnswer(turn)], readBack: turn.messages ? undefined : true }));
