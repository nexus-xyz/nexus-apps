import type { CoreClient, DeviceThreadKey, E2eDevice } from "@nexus/kit";
import type { PhoneInvitation, ComputerScope, PhonePairingEnvelope, PhonePairingRequest } from "@nexus/kit/computer-types";
import { handoverOf, trustedDevices } from "@nexus/kit/device-trust";
import { b64u, fromB64u, lockContentKey, type DeviceKey } from "@nexus/kit/e2e";
import { approvePending, deviceFingerprint, deviceStatus, thisDevice, webCryptoSigner } from "@nexus/kit/pairing";
import { phoneInvitationMessage, sealPhonePairing, verifyPhoneInvitation, verifyPhonePairingDecision } from "@nexus/kit/phone-link";
import { unlockHistory } from "@nexus/kit/recovery";
import { createDeviceKey, loadDeviceKey } from "./device-key";
import { DEVICE_NAME } from "./code-reader";
import { connectionSignal } from "./reconnect";
import { load, Storage } from "./store";

/** Validates the current account and scanned identity before registering or approving anything. Keeps the phone key. */
export class PhoneConnectionScopeError extends Error {
  constructor(readonly organization: string) {
    super("This computer is in another Nexus organization. Switch to that organization to connect.");
  }
}

type PhonePairingReceipt = {
  deviceId: string;
  holder: DeviceKey;
  computerId: string;
  envelope: PhonePairingEnvelope;
  code: string;
};

export type PhonePairingDelivery = {
  submitted?: PhonePairingReceipt;
};

// Keep one signed, encrypted request across a restart. The invitation secret and the phone key stay off disk here.
const PAIRING_DELIVERY_KEY = "phone-pairing:delivery";
type SavedDelivery = { invitationHash: string; envelope: PhonePairingEnvelope };

async function pairingReceipt(invitation: PhoneInvitation, self: E2eDevice, holder: DeviceKey, envelope: PhonePairingEnvelope): Promise<PhonePairingReceipt> {
  return { deviceId: self.id, holder, computerId: invitation.computerId, envelope, code: (await deviceFingerprint(self)).code };
}

async function trustInviter(core: CoreClient, invitation: PhoneInvitation, devices: E2eDevice[], inviter: E2eDevice,
  sender: { deviceId: string; holder: DeviceKey }, signal?: AbortSignal) {
  const phone = thisDevice(devices, sender.holder);
  if (phone?.id !== sender.deviceId || phone.state !== "trusted") throw new Error("Approve this phone on a trusted device first.");
  const signer = webCryptoSigner(sender.holder);
  const fingerprint = await deviceFingerprint(inviter);
  const trust = await trustedDevices(devices, { ...sender, ...sender.holder }).catch(() => undefined);
  if (trust?.revoked.has(inviter.id)) throw new Error("This computer was disconnected. Set it up again on the computer.");
  return approvePending({
    approveE2eDevice(id, body) {
      signal?.throwIfAborted();
      if (Date.now() >= invitation.expiresAt) throw new Error("The connection invitation expired. Show a new QR on the computer.");
      return core.approveE2eDevice(id, body);
    },
  }, sender.deviceId, signer, inviter, { deviceId: inviter.id, ...fingerprint }, async (recipient) => {
    if (!trust) return [];
    const encryptionKey = fromB64u(recipient.encryptionKey);
    if (!encryptionKey) throw new Error("The computer's key could not be read.");
    const history = await unlockHistory(core, devices, phone, sender.holder, trust);
    const copies: DeviceThreadKey[] = [];
    try {
      for (const key of history.keys) {
        const epoch = key.keyVersion - 1;
        const locked = await lockContentKey(key.raw, key.threadId, epoch, recipient.id, encryptionKey);
        copies.push({
          threadId: key.threadId,
          keyVersion: key.keyVersion,
          lockedKey: b64u(locked),
          signature: b64u(await signer.signThreadKey(key.threadId, epoch, recipient.id, locked)),
        });
      }
      return copies;
    } finally {
      for (const key of history.keys) key.raw.fill(0);
    }
  }, trust && handoverOf(devices, trust));
}

async function continueDelivery(core: CoreClient, invitation: PhoneInvitation, receipt: PhonePairingReceipt,
  reply: PhonePairingRequest, signal?: AbortSignal, knownDevices?: E2eDevice[]) {
  if (invitation.version === 1) return receipt;
  if (reply.decision) {
    const state = await verifyPhonePairingDecision(invitation, receipt.envelope, reply.decision);
    if (state === "cancelled") throw new Error("The computer cancelled the connection. Show a new QR code there.");
    return receipt;
  }
  if (Date.now() >= invitation.expiresAt) return receipt;
  const devices = knownDevices ?? (await core.e2eDevices(connectionSignal(signal))).devices;
  const self = thisDevice(devices, receipt.holder);
  if (self?.state === "trusted" && self.id === receipt.deviceId
      && devices.some((device) => device.id === invitation.inviterDeviceId && device.state === "pending")) {
    const sender = { deviceId: receipt.deviceId, holder: receipt.holder };
    const inviter = await verifyPhoneInvitation({ invitation, scope: receipt.envelope.scope, devices, reader: sender });
    await trustInviter(core, invitation, devices, inviter, sender, signal);
  }
  return receipt;
}

export async function connectPhone(
  core: CoreClient,
  gateway: string,
  invitation: PhoneInvitation,
  delivery: PhonePairingDelivery,
  signal?: AbortSignal,
): Promise<PhonePairingReceipt> {
  if (delivery.submitted) {
    const receipt = delivery.submitted;
    const reply = invitation.version === 2
      ? await core.phonePairingReceipt(receipt.envelope, connectionSignal(signal))
      : await core.submitPhonePairingRequest(receipt.envelope.inviterDeviceId, receipt.envelope, connectionSignal(signal));
    return continueDelivery(core, invitation, receipt, reply, signal);
  }
  const me = await core.me(connectionSignal(signal));
  if (!me.user?.id) throw new Error("Sign in with your personal account to connect this phone.");
  const scope: ComputerScope = { gateway: gateway.replace(/\/+$/, ""), tenantId: me.tenant.id, personId: me.user.id };
  if (scope.personId !== invitation.scope.personId) throw new Error("Use the same Nexus account as the computer, then scan again.");
  let devices = (await core.e2eDevices(connectionSignal(signal))).devices;
  if (scope.tenantId !== invitation.scope.tenantId) {
    await verifyPhoneInvitation({ invitation, scope: { ...scope, tenantId: invitation.scope.tenantId }, devices });
    throw new PhoneConnectionScopeError(invitation.scope.tenantId);
  }
  let holder = await loadDeviceKey();
  let self = holder ? thisDevice(devices, holder) : undefined;
  const signedInvitation = invitation.signature + "\n" + await phoneInvitationMessage(invitation);
  const invitationHash = b64u(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(signedInvitation))));
  const saved = load<SavedDelivery>(PAIRING_DELIVERY_KEY);
  const savedEnvelope = saved?.envelope;
  if (holder && self && (self.state === "trusted" || self.state === "pending")
      && saved?.invitationHash === invitationHash && savedEnvelope?.senderDeviceId === self.id
      && savedEnvelope.inviteId === invitation.inviteId && savedEnvelope.inviterDeviceId === invitation.inviterDeviceId
      && savedEnvelope.expiresAt === invitation.expiresAt && savedEnvelope.scope?.gateway === scope.gateway
      && savedEnvelope.scope.tenantId === scope.tenantId && savedEnvelope.scope.personId === scope.personId) {
    const receipt = await pairingReceipt(invitation, self, holder, savedEnvelope);
    signal?.throwIfAborted();
    const reply = await core.phonePairingReceipt(savedEnvelope, connectionSignal(signal));
    return continueDelivery(core, invitation, receipt, reply, signal, devices);
  }
  let inviter = await verifyPhoneInvitation({ invitation, scope, devices, ...(self && holder ? { reader: { deviceId: self.id, holder } } : {}) });
  if (inviter.state === "pending" && self?.state !== "trusted") {
    throw new Error("This computer needs a trusted device to approve it first. Use a phone that’s already trusted.");
  }
  if (self && deviceStatus(self) === "expired") {
    throw new Error("This phone’s old pairing code expired. Choose Pair with a code to make a new one.");
  }
  signal?.throwIfAborted();
  holder ??= await createDeviceKey();
  if (!self) {
    self = await core.registerE2eDevice(
      { name: DEVICE_NAME, encryptionKey: b64u(holder.encryptionKey), signingKey: b64u(holder.signingKey) },
      connectionSignal(signal),
    );
    devices = [...devices, self];
  }
  if (!self) throw new Error("This phone's registration was not found. Try scanning again.");
  const sender = { deviceId: self.id, holder };
  if (invitation.version === 1 && inviter.state === "pending" && self.state === "trusted") {
    inviter = await trustInviter(core, invitation, devices, inviter, sender, signal);
    devices = devices.map((device) => device.id === inviter.id ? inviter : device);
  }
  // A previous submission may have reached the desktop even if its response timed out.
  const { requests } = await core.phonePairingRequests(invitation.inviterDeviceId, connectionSignal(signal));
  const existing = requests.find(({ envelope, state }) => state === "pending"
    && envelope.inviteId === invitation.inviteId && envelope.senderDeviceId === sender.deviceId);
  const envelope = existing?.envelope ?? await sealPhonePairing({ invitation, sender, devices });
  const receipt = await pairingReceipt(invitation, self, holder, envelope);
  signal?.throwIfAborted();
  Storage.setItemSync(PAIRING_DELIVERY_KEY, JSON.stringify({ invitationHash, envelope } satisfies SavedDelivery));
  delivery.submitted = receipt;
  if (existing && invitation.version === 1) return receipt;
  const reply = existing
    ? await core.phonePairingReceipt(envelope, connectionSignal(signal))
    : await core.submitPhonePairingRequest(envelope.inviterDeviceId, envelope, connectionSignal(signal));
  return continueDelivery(core, invitation, receipt, reply, signal, devices);
}
