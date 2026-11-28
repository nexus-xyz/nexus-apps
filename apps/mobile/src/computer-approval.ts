import type { CoreClient } from "@nexus/kit";
import { handoverOf, trustedDevices } from "@nexus/kit/device-trust";
import { approvePending, deviceFingerprint, findPending, matchesInput, thisDevice, webCryptoSigner, type PairingInput } from "@nexus/kit/pairing";
import { loadDeviceKey } from "./device-key";
import { connectionSignal } from "./reconnect";

async function approvalContext(core: CoreClient, input: PairingInput, signal: AbortSignal) {
  const [holder, { devices }] = await Promise.all([loadDeviceKey(), core.e2eDevices(connectionSignal(signal))]);
  const phone = holder && thisDevice(devices, holder);
  if (!holder || phone?.state !== "trusted") {
    throw new Error("Use a phone that’s already trusted, or enter this code in Settings → Devices on a trusted computer.");
  }
  const trust = await trustedDevices(devices, {
    deviceId: phone.id, encryptionKey: holder.encryptionKey, signingKey: holder.signingKey, holder,
  });

  const listed = devices.find((device) => device.id === input.deviceId);
  // The approval may have reached Gateway even when the phone missed the response.
  const alreadyApproved = listed?.approvedBy === phone.id && trust.trusted.has(listed.id) && await matchesInput(listed, input);
  const device = alreadyApproved ? listed : await findPending(devices, input);
  if (trust.revoked.has(device.id)) throw new Error("Start a new pairing on the computer. This device was revoked.");
  signal.throwIfAborted();
  return { holder, phone, devices, trust, device, alreadyApproved };
}

export async function reviewComputerApproval(core: CoreClient, input: PairingInput, signal: AbortSignal) {
  const { phone, device, alreadyApproved } = await approvalContext(core, input, signal);
  return { phoneId: phone.id, deviceId: device.id, name: device.name, phoneCode: (await deviceFingerprint(phone)).code, alreadyApproved };
}

export async function approveComputer(core: CoreClient, input: PairingInput, phoneId: string, signal: AbortSignal): Promise<void> {
  const { holder, phone, devices, trust, device, alreadyApproved } = await approvalContext(core, input, signal);
  if (phoneId !== phone.id) throw new Error("The phone’s device key changed. Scan the computer’s code again.");
  if (alreadyApproved) return;
  await approvePending({
    approveE2eDevice(id, body) {
      signal.throwIfAborted();
      return core.approveE2eDevice(id, body);
    },
  }, phone.id, webCryptoSigner(holder), device, input, undefined, handoverOf(devices, trust));
}
