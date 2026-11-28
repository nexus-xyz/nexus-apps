import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CONNECTOR_PURPOSES, connectorPurpose } from "./connector-names";

/** GPUI's mirror of the map (`nexus-desktop-core`'s `agent_cards.rs`). */
const RUST = new URL("../../../crates/nexus-desktop-core/src/agent_cards.rs", import.meta.url);

describe("a connector's plain words (A6·e)", () => {
  it("says what Nexus reads, then that a change asks first, in one sentence", () => {
    for (const line of Object.values(CONNECTOR_PURPOSES)) expect(line).toMatch(/^It (reads|finds and reads) your [^.;]+; [a-z ]+ asks first\.$/);
    expect(connectorPurpose("gmail")).toBe("It reads your mail; sending asks first.");
    expect(connectorPurpose("resy")).toBe("Nexus can read it, and each write asks you first.");
    expect(connectorPurpose("toString", "Ada")).toBe("Ada can read it, and each write asks you first.");
  });

  it("is the same in GPUI's mirror", () => {
    const rust = readFileSync(RUST, "utf8");
    for (const [id, line] of Object.entries(CONNECTOR_PURPOSES)) expect(rust).toContain(`"${id}" => "${line}"`);
  });
});
