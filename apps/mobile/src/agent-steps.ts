import { routeOf, type AgentAccessEntry, type AgentRoute, type SetupStep } from "@nexus/kit";

export const DESKTOP_AGENT_SETUP_LINK = "nexus://settings/agents";

export function agentSetupAvailable(entry: AgentAccessEntry, route: AgentRoute, subscriptions: boolean) {
  return routeOf(entry, route)?.available === true && (route !== "subscription" || subscriptions);
}

/**
 * The setup steps the phone shows for an agent: only what an owner or admin still has to do, and a route that cannot
 * run, as text. Installing an agent and signing it in happen on the computer, in Nexus desktop or the `nexus` CLI, so
 * the phone lists no terminal commands.
 */
export function phoneSteps(steps: SetupStep[]): SetupStep[] {
  return steps.filter((step) => step.done !== true && (step.who === "admin" || !step.command));
}
