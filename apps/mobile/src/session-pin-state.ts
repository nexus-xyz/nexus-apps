import type { SessionPin, SessionPinSource } from "@nexus/kit";

export type PinTarget = Pick<SessionPin, "source" | "sessionId">;
export type PinChange = PinTarget & { pinned: boolean; expectedVersion: number };
export type PinSnapshot = { pins: SessionPin[]; pending: PinChange[] };

export const pinKey = ({ source, sessionId }: PinTarget) => `${source}:${sessionId}`;

export function pinTarget(key: string): PinTarget | undefined {
  const parts = key.split(":");
  if (parts.length !== 2) return;
  const [source, sessionId] = parts;
  if ((source === "chat" || source === "synced") && sessionId) return { source, sessionId };
}

export function visiblePinKeys({ pins, pending }: PinSnapshot): string[] {
  const pendingKeys = new Set(pending.map(pinKey));
  return [
    ...[...pending].reverse().filter(({ pinned }) => pinned).map(pinKey),
    ...pins.filter((pin) => pin.pinned && !pendingKeys.has(pinKey(pin))).map(pinKey),
  ];
}

export function acceptPin(snapshot: PinSnapshot, sent: PinChange, remote: SessionPin): PinSnapshot {
  const key = pinKey(sent);
  const pins = [remote, ...snapshot.pins.filter((pin) => pinKey(pin) !== key)]
    .sort((a, b) => (b.updatedAt ?? "").localeCompare(a.updatedAt ?? ""));
  const pending = snapshot.pending.flatMap((change) => {
    if (change === sent) return [];
    if (pinKey(change) === key && change.expectedVersion === sent.expectedVersion) {
      return [{ ...change, expectedVersion: remote.version }];
    }
    return [change];
  });
  return { pins, pending };
}

export function queuePin(snapshot: PinSnapshot, source: SessionPinSource, sessionId: string, pinned: boolean): PinSnapshot {
  const key = pinKey({ source, sessionId });
  const last = snapshot.pending.find((change) => pinKey(change) === key);
  const version = snapshot.pins.find((pin) => pinKey(pin) === key)?.version ?? 0;
  return {
    ...snapshot,
    pending: [...snapshot.pending.filter((change) => pinKey(change) !== key), { source, sessionId, pinned, expectedVersion: last?.expectedVersion ?? version }],
  };
}

export function migrateLocalPins(pins: SessionPin[], local: readonly string[]): PinSnapshot {
  const savedKeys = new Set(pins.map(pinKey));
  const pending = [...local].reverse().flatMap((key) => {
    const target = pinTarget(key);
    return target && !savedKeys.has(key) ? [{ ...target, pinned: true, expectedVersion: 0 }] : [];
  });
  return { pins, pending };
}
