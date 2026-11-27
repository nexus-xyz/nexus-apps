import { CoreError } from "@nexus/kit";
import { describe, expect, it } from "vitest";
import { failureCause, failureTitle, PairingError, transientError } from "./connection-errors";

describe("phone-control failures", () => {
  it("retries a lost connection, a timeout or a busy Gateway, and stops on a definite refusal", () => {
    expect(transientError(new TypeError("Network request failed"))).toBe(true);
    expect(transientError(Object.assign(new Error("deadline"), { name: "TimeoutError" }))).toBe(true);
    expect(transientError(new CoreError(503, "Unavailable"))).toBe(true);
    expect(transientError(new CoreError(429, "Slow down"))).toBe(true);
    expect(transientError(new CoreError(403, "Not allowed"))).toBe(false);
    expect(transientError(new CoreError(410, "Expired"))).toBe(false);
    expect(transientError(new Error("This phone was revoked."))).toBe(false);
  });

  it("titles an error by its cause, not always as a pairing problem", () => {
    expect(failureTitle(failureCause(new PairingError("Scan Connect phone first.")))).toBe("Connect this phone");
    expect(failureTitle(failureCause(new TypeError("Network request failed")))).toBe("Can't reach Nexus right now");
    expect(failureTitle(failureCause(new CoreError(403, "Not allowed")))).toBe("Couldn't open this computer");
  });
});
