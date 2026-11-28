import { CoreError, type CoreClient } from "@nexus/kit";
import type { PhoneInvitation } from "@nexus/kit/computer-types";
import { confirmDeviceApprover, trustedDevices } from "@nexus/kit/device-trust";
import { verifyPhonePairingDecision } from "@nexus/kit/phone-link";
import { deviceFingerprint, pairingQrText, thisDevice } from "@nexus/kit/pairing";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ReaderState } from "./code-sessions";
import { transientError } from "./connection-errors";
import { haptic } from "./haptics";
import { connectPhone, PhoneConnectionScopeError, type PhonePairingDelivery } from "./phone-connect";
import { connectionSignal } from "./reconnect";

type Attempt =
  | { kind: "connecting" }
  | { kind: "approval"; code: string; reconnecting?: boolean }
  | { kind: "confirm"; computerId: string; approver?: string; ready: Extract<ReaderState, { kind: "ready" }> }
  | { kind: "connected"; computerId: string }
  | { kind: "failed"; message: string; retryable: boolean; organization?: string };

export const EXPIRED = "This code expired. Choose New QR code on your computer.";
const POLL_MS = 2000;

export function usePhoneConnection({ invitation, core, gateway, active }: {
  invitation?: PhoneInvitation;
  core?: CoreClient | null;
  gateway?: string;
  active: boolean;
}) {
  const owner = useMemo(() => ({}), [core, gateway, invitation]);
  const [result, setResult] = useState<{ owner: object; attempt: Attempt }>();
  const attempt: Attempt = result?.owner === owner ? result.attempt : { kind: "connecting" };
  const [retry, setRetry] = useState(0);
  const work = useRef<AbortController | undefined>(undefined);
  const request = useRef<{
    owner: object;
    delivery: PhonePairingDelivery;
    receipt?: Awaited<ReturnType<typeof connectPhone>>;
  } | undefined>(undefined);

  const settled = attempt.kind === "connected" || attempt.kind === "confirm" || attempt.kind === "failed";
  useEffect(() => {
    if (!invitation || !core || !gateway || !active || settled) return;
    const controller = new AbortController();
    work.current = controller;
    const { signal } = controller;
    if (request.current?.owner !== owner) request.current = { owner, delivery: {} };
    const setAttempt = (attempt: Attempt) => setResult({ owner, attempt });
    const held = request.current;
    void (async () => {
      const paired = held.receipt ?? await connectPhone(core, gateway, invitation, held.delivery, signal);
      if (signal.aborted) return;
      held.receipt = paired;
      setAttempt({ kind: "approval", code: paired.code });
      while (!signal.aborted) {
        try {
          const deadline = connectionSignal(signal);
          const [directory, pending] = await Promise.all([
            core.e2eDevices(deadline),
            invitation.version === 1 ? core.phonePairingRequests(invitation.inviterDeviceId, deadline) : undefined,
          ]);
          if (signal.aborted) return;
          let devices = directory.devices;
          let self = thisDevice(devices, paired.holder);
          const expired = Date.now() >= invitation.expiresAt;
          let finished: boolean;
          if (invitation.version === 2) {
            const receipt = await core.phonePairingReceipt(paired.envelope, connectionSignal(signal));
            const decision = receipt.decision && await verifyPhonePairingDecision(invitation, paired.envelope, receipt.decision);
            if (decision === "cancelled") throw new Error("The computer cancelled the connection. Show a new QR code there.");
            finished = decision === "approved";
          } else {
            const waiting = pending?.requests.some(({ envelope, state }) => envelope.id === paired.envelope.id && state === "pending");
            const checkReceipt = expired || (self?.state === "trusted" && !waiting);
            finished = checkReceipt && (await core.phonePairingReceipt(paired.envelope, connectionSignal(signal))).state === "finished";
          }
          if (finished && self?.state !== "trusted") {
            // The desktop can approve the phone between the directory read and the receipt.
            devices = (await core.e2eDevices(connectionSignal(signal))).devices;
            self = thisDevice(devices, paired.holder);
          }
          if (signal.aborted) return;
          if (finished && self?.id === paired.deviceId && self.state === "trusted") {
            const reader = { deviceId: self.id, holder: paired.holder };
            const trust = await trustedDevices(devices, { ...reader, ...paired.holder });
            if (signal.aborted) return;
            if (self.approvedBy && !trust.trusted.has(self.approvedBy)) {
              if (invitation.version === 1 || self.approvedBy !== invitation.inviterDeviceId) {
                setAttempt({
                  kind: "confirm",
                  computerId: paired.computerId,
                  approver: devices.find((device) => device.id === self.approvedBy)?.name,
                  ready: { kind: "ready", reader, devices },
                });
                return;
              }
              // The verified invitation carries the same fingerprint the Mac would show for manual confirmation.
              const { fingerprint } = await deviceFingerprint(invitation);
              if (signal.aborted) return;
              await confirmDeviceApprover(devices, { ...reader, ...paired.holder }, pairingQrText(invitation.inviterDeviceId, fingerprint));
            }
            if (signal.aborted) return;
            setAttempt({ kind: "connected", computerId: paired.computerId });
            haptic("success");
            return;
          }
          if (self?.state === "revoked") throw new Error("This phone was disconnected. Pair it again from a trusted device.");
          if (expired) throw new Error(EXPIRED);
          setAttempt({ kind: "approval", code: paired.code });
        } catch (error) {
          if (signal.aborted) return;
          if (error instanceof CoreError && error.code === "phone_invitation_expired") throw new Error(EXPIRED);
          if (!transientError(error) || Date.now() >= invitation.expiresAt) throw error;
          setAttempt({ kind: "approval", code: paired.code, reconnecting: true });
        }
        await waitForPoll(signal);
      }
    })().catch((error: unknown) => {
      if (signal.aborted) return;
      const message = error instanceof Error ? error.message : String(error);
      setAttempt({
        kind: "failed",
        message,
        retryable: transientError(error),
        organization: error instanceof PhoneConnectionScopeError ? error.organization : undefined,
      });
      haptic(message === EXPIRED ? "warning" : "error");
    });
    return () => controller.abort();
  }, [owner, invitation, core, gateway, active, retry, settled]);

  return {
    attempt,
    reset() {
      work.current?.abort();
      request.current = undefined;
      setResult(undefined);
    },
    retry() {
      setResult(undefined);
      setRetry((value) => value + 1);
    },
    confirm() {
      if (attempt.kind !== "confirm") return;
      setResult({ owner, attempt: { kind: "connected", computerId: attempt.computerId } });
      haptic("success");
    },
  };
}

function waitForPoll(signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) { resolve(); return; }
    const done = () => {
      clearTimeout(timer);
      signal.removeEventListener("abort", done);
      resolve();
    };
    const timer = setTimeout(done, POLL_MS);
    signal.addEventListener("abort", done, { once: true });
  });
}
