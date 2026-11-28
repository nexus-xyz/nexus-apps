/** What a Gateway has on, read from `GET /v1/tenants/me` or `GET /v1/auth/whoami`. */
import type { GatewayFeatures } from "./api";

/**
 * Whether the Gateway that answered has Nexus Wiki on, and with it Ask your code, which reads it. An older Gateway, with
 * no `features`, reads as off.
 */
export function wikiOn(me: { features?: GatewayFeatures } | null | undefined): boolean {
  return me?.features?.wiki === true;
}

/** Whether the Gateway that answered has Nexus Apps on. An older Gateway, with no `apps`, reads as off. */
export function appsOn(me: { features?: GatewayFeatures } | null | undefined): boolean {
  return me?.features?.apps === true;
}

/**
 * Whether the Gateway that answered writes audit events for coding agent sessions, so the Audit log offers
 * their filter. An older Gateway, with no `agentSessionAudit`, reads as off.
 */
export function agentSessionAuditOn(me: { features?: GatewayFeatures } | null | undefined): boolean {
  return me?.features?.agentSessionAudit === true;
}
