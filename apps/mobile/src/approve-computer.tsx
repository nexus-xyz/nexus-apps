import type { CoreClient } from "@nexus/kit";
import { formatCode, type PairingInput } from "@nexus/kit/pairing";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator } from "react-native";
import { approveComputer, reviewComputerApproval } from "./computer-approval";
import { ConnectedComputer } from "./connected-computer";
import { ConnectButton, ConnectCode, ConnectHeader, ConnectMessage } from "./connect-controls";
import { haptic } from "./haptics";

type Review = Awaited<ReturnType<typeof reviewComputerApproval>>;
type Result =
  | { kind: "failed"; message: string }
  | { kind: "review"; review: Review; error?: string }
  | { kind: "approving" | "approved"; review: Review };

export function ApproveComputer({ core, input, onScan, onOpen, onDone }: {
  core: CoreClient;
  input: PairingInput;
  onScan: () => void;
  onOpen?: (computerId: string) => void;
  onDone: () => void;
}) {
  const [lookup, setLookup] = useState<{ core: CoreClient; result: Result }>();
  const [retry, setRetry] = useState(0);
  const submission = useRef<AbortController | undefined>(undefined);
  const result = lookup?.core === core ? lookup.result : undefined;

  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    void reviewComputerApproval(core, input, signal).then(
      (review) => {
        if (!signal.aborted) setLookup({ core, result: { kind: review.alreadyApproved ? "approved" : "review", review } });
      },
      (failure: unknown) => {
        if (signal.aborted) return;
        setLookup({ core, result: { kind: "failed", message: message(failure) } });
        haptic("error");
      },
    );
    return () => {
      controller.abort();
      submission.current?.abort();
      submission.current = undefined;
    };
  }, [core, input, retry]);

  async function approve(review: Review) {
    if (submission.current) return;
    const controller = new AbortController();
    submission.current = controller;
    setLookup({ core, result: { kind: "approving", review } });
    try {
      await approveComputer(core, input, review.phoneId, controller.signal);
      if (!controller.signal.aborted) {
        setLookup({ core, result: { kind: "approved", review } });
        haptic("success");
      }
    } catch (failure) {
      if (!controller.signal.aborted) {
        setLookup({ core, result: { kind: "review", review, error: message(failure) } });
        haptic("error");
      }
    } finally {
      if (submission.current === controller) submission.current = undefined;
    }
  }

  if (!result) return <>
    <ConnectHeader title="Checking the approval code…" icon="computer" />
    <ActivityIndicator />
    <ConnectButton label="Scan again" icon="scan" onPress={onScan} />
  </>;
  if (result.kind === "failed") return <>
    <ConnectHeader title="Couldn’t check this code" icon="keys">{result.message}</ConnectHeader>
    <ConnectButton label="Try again" icon="restore" primary onPress={() => { setLookup(undefined); setRetry((value) => value + 1); }} />
    <ConnectButton label="Scan again" icon="scan" onPress={onScan} />
  </>;

  const { review } = result;
  if (result.kind === "approved") return <ConnectedComputer
    target={{ kind: "device", id: review.deviceId }}
    name={review.name}
    onOpen={onOpen}
    onDone={onDone}
    waiting={<>
      <ConnectMessage>Computer approved. On {review.name}, match this fingerprint under This device, then click Confirm.</ConnectMessage>
      <ConnectCode code={formatCode(review.phoneCode)} />
    </>}
  />;

  const busy = result.kind === "approving";
  return <>
    <ConnectHeader title="Approve this computer" icon="computer">
      Trust {review.name}? Compare this code with the one on the computer.
    </ConnectHeader>
    <ConnectCode code={formatCode(input.code)} />
    {result.kind === "review" && result.error && <ConnectMessage error>{result.error}</ConnectMessage>}
    <ConnectButton label="Approve computer" icon="check" primary busy={busy} onPress={() => { void approve(review); }} />
    <ConnectButton label="Scan again" icon="scan" disabled={busy} onPress={onScan} />
  </>;
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
