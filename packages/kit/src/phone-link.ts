import type { E2eDevice } from "./api";
import type { ComputerScope, PhoneInvitation, PhonePairingDecision, PhonePairingEnvelope } from "./computer-types";
import { b64u, fromB64u, type DeviceKey, sign, verify } from "./e2e";
import { approvalOf, holderKind, thisDevice } from "./pairing";
import { trustedDevices } from "./device-trust";
import { CLOCK_SKEW_MS, trustedRunner } from "./remote-control";
import { binary, computerName, deviceId, frame, integer, MAX_PHONE_PAIRING_BYTES, object, phonePairingMessage, protocolError, randomUuid, readScope, sameScope, scopeParts, sealObject, sha256, text, uuid } from "./computer-protocol";

export { phonePairingMessage } from "./computer-protocol";
export type { PhoneInvitation, PhonePairingEnvelope } from "./computer-types";

export async function phoneInvitationMessage(invitation: Omit<PhoneInvitation, "signature">): Promise<string> {
  return frame(`nexus-phone-invitation-v${invitation.version}`, [...scopeParts(invitation.scope), invitation.inviteId, invitation.computerId,
    invitation.computerName, invitation.inviterDeviceId, invitation.encryptionKey, invitation.signingKey,
    String(invitation.expiresAt), await sha256(binary(invitation.secret, 32))]);
}

function readInvitation(value: unknown): PhoneInvitation {
  const fields = object(value);
  if (fields.version !== 1 && fields.version !== 2) throw protocolError("This connection link needs a newer version of Nexus.");
  binary(fields.encryptionKey, 32);
  binary(fields.signingKey, 32);
  binary(fields.secret, 32);
  binary(fields.signature, 64);
  return {
    version: fields.version, scope: readScope(fields.scope), inviteId: uuid(fields.inviteId), computerId: uuid(fields.computerId),
    computerName: computerName(fields.computerName), inviterDeviceId: deviceId(fields.inviterDeviceId),
    encryptionKey: text(fields.encryptionKey, 64), signingKey: text(fields.signingKey, 64), expiresAt: integer(fields.expiresAt),
    secret: text(fields.secret, 64), signature: text(fields.signature, 128),
  };
}

export function phoneInvitationLink(invitation: PhoneInvitation, { appUrl = "xyz.nexus.app://connect" }: { appUrl?: string } = {}): string {
  const checked = readInvitation(invitation);
  const url = new URL(appUrl);
  if (!["xyz.nexus.app:", "nexus:", "exp:", "exps:"].includes(url.protocol) || url.username || url.password) throw protocolError("The phone connection address is invalid.");
  url.hash = `invite=${b64u(new TextEncoder().encode(JSON.stringify(checked)))}`;
  return url.toString();
}

/** Reads only an invitation fragment. Never returns a URL to which an existing credential should be forwarded. */
export function parsePhoneInvitation(value: string): PhoneInvitation {
  try {
    if (value.length > 16 * 1024) throw protocolError();
    let encoded: string;
    if (value.startsWith("nexus-connect:v1:")) encoded = value.slice("nexus-connect:v1:".length);
    else {
      const url = new URL(value);
      if (!["xyz.nexus.app:", "nexus:", "exp:", "exps:"].includes(url.protocol)
        || (url.hostname !== "connect" && !url.pathname.endsWith("/connect")) || url.username || url.password) throw protocolError();
      encoded = new URLSearchParams(url.hash.slice(1)).get("invite") ?? "";
    }
    const bytes = binary(encoded);
    if (bytes.length > MAX_PHONE_PAIRING_BYTES) throw protocolError();
    return readInvitation(JSON.parse(new TextDecoder().decode(bytes)));
  } catch {
    throw protocolError("That is not a Nexus computer connection link. Scan the code shown on your computer.");
  }
}

/** The scan anchors exact keys; a directory row alone cannot authorize a new root. */
export async function verifyPhoneInvitation({ invitation: raw, scope, devices, reader, now = Date.now() }: {
  invitation: PhoneInvitation; scope: ComputerScope; devices: E2eDevice[];
  reader?: { deviceId: string; holder: DeviceKey }; now?: number;
}): Promise<E2eDevice> {
  const invitation = readInvitation(raw);
  if (!sameScope(invitation.scope, readScope(scope))) throw protocolError("Sign in to the same Nexus account and organization as this computer.");
  if (invitation.expiresAt <= now || invitation.expiresAt > now + 5 * 60_000 + CLOCK_SKEW_MS) throw protocolError("This connection code expired. Show a new one on your computer.");
  const inviter = devices.find((device) => device.id === invitation.inviterDeviceId);
  if (!inviter || holderKind(inviter) !== "device" || (inviter.state !== "trusted" && inviter.state !== "pending")
    || inviter.encryptionKey !== invitation.encryptionKey || inviter.signingKey !== invitation.signingKey
    || !await verify(binary(invitation.signingKey, 32), await phoneInvitationMessage(invitation), binary(invitation.signature, 64))) throw protocolError("This computer's keys do not match the scanned code. Show a new code on the computer.");
  if (reader) {
    const self = thisDevice(devices, reader.holder);
    if (self && self.id !== reader.deviceId) throw protocolError();
    if (self?.state === "revoked") throw protocolError("This phone was disconnected. Pair it again with a new device key.");
    if (self?.state === "trusted" && inviter.state === "trusted") {
      if (self.approvedBy === inviter.id) {
        // The scan identifies the direct approver. Wait for the desktop to finish this request before pinning it.
        const trust = await trustedDevices(devices, { ...reader, ...reader.holder });
        const message = approvalOf(self);
        const signature = self.approvalSignature && fromB64u(self.approvalSignature);
        if (trust.revoked.has(inviter.id) || !message || !signature
            || !await verify(binary(invitation.signingKey, 32), message, signature)) {
          throw protocolError("This computer’s approval of the phone could not be verified.");
        }
      } else {
        await trustedRunner(devices, reader, inviter.id);
      }
    }
  }
  return inviter;
}

export function phonePairingDecisionMessage(decision: PhonePairingDecision): string {
  const approved = decision.state === "approved";
  return frame("nexus-phone-pairing-decision-v1", [
    ...scopeParts(decision.scope), decision.computerId, decision.inviteId, decision.inviterDeviceId,
    String(decision.expiresAt), decision.state, approved ? decision.requestId : "", approved ? decision.senderDeviceId : "",
  ]);
}

/** Only the keys from the scanned invitation can complete or cancel this request. */
export async function verifyPhonePairingDecision(
  invitation: PhoneInvitation,
  request: PhonePairingEnvelope,
  decision: PhonePairingDecision,
): Promise<PhonePairingDecision["state"]> {
  if ((decision.state !== "approved" && decision.state !== "cancelled")
      || !sameScope(readScope(decision.scope), invitation.scope)
      || decision.computerId !== invitation.computerId || decision.inviteId !== invitation.inviteId
      || decision.inviterDeviceId !== invitation.inviterDeviceId || decision.expiresAt !== invitation.expiresAt
      || (decision.state === "approved" && (decision.requestId !== request.id || decision.senderDeviceId !== request.senderDeviceId))
      || !await verify(binary(invitation.signingKey, 32), phonePairingDecisionMessage(decision), binary(decision.signature, 64))) {
    throw protocolError("This computer’s response could not be verified. Start a new connection on the computer.");
  }
  return decision.state;
}

export async function sealPhonePairing({ invitation, sender, devices, scope = invitation.scope, id = randomUuid(), now = Date.now() }: {
  invitation: PhoneInvitation; sender: { deviceId: string; holder: DeviceKey }; devices: E2eDevice[];
  scope?: ComputerScope; id?: string; now?: number;
}): Promise<PhonePairingEnvelope> {
  const inviter = await verifyPhoneInvitation({ invitation, scope, devices, reader: sender, now });
  const self = thisDevice(devices, sender.holder);
  if (!self || self.id !== sender.deviceId || holderKind(self) !== "device" || (self.state !== "pending" && self.state !== "trusted")) throw protocolError("Set up this phone's device key before connecting.");
  const route = {
    version: 1 as const, scope: invitation.scope, id: uuid(id), inviteId: invitation.inviteId,
    inviterDeviceId: inviter.id, senderDeviceId: sender.deviceId, expiresAt: invitation.expiresAt,
  };
  const box = await sealObject(route.id, inviter.id, binary(invitation.encryptionKey, 32), {
    ...route, kind: "phonePairing", computerId: invitation.computerId, secret: invitation.secret,
    encryptionKey: b64u(sender.holder.encryptionKey), signingKey: b64u(sender.holder.signingKey),
  }, MAX_PHONE_PAIRING_BYTES);
  const envelope = { ...route, ...box };
  return { ...envelope, signature: b64u(await sign(sender.holder, phonePairingMessage(envelope))) };
}
