import { describe, expect, it } from "vitest";
import { afterMessages, contactMatches, nameScore, phoneChoices, phoneLabel, textCardTitle, textToSend, type DeviceContact } from "./contact-text";

const ana: DeviceContact = { id: "1", fullName: "Ana Lima", phones: [{ label: "_$!<Mobile>!$_", number: "+1 (555) 010-2000" }, { label: "work", number: "+1 555 010 2000" }] };
const anabel: DeviceContact = { id: "2", givenName: "Anabel", familyName: "Ruiz", phones: [{ label: "home", number: "+52 55 1234 5678" }] };
const noPhone: DeviceContact = { id: "3", fullName: "Ana", phones: [] };
const exact: DeviceContact = { id: "4", fullName: "Ana", phones: [{ number: "+34 600 000 000" }] };

describe("contact matching", () => {
  it("ranks the whole name first, then the first name, then word starts", () => {
    expect(nameScore("Ana", "ana")).toBe(3);
    expect(nameScore("Ana Lima", "Ana")).toBe(2);
    expect(nameScore("Anabel Ruiz", "Ana")).toBe(1);
    expect(nameScore("Bruno", "Ana")).toBe(0);
  });

  it("ignores accents and case", () => {
    expect(nameScore("José Núñez", "jose nunez")).toBe(3);
  });

  it("keeps only contacts with a number, best first", () => {
    expect(contactMatches([anabel, ana, noPhone, exact], "Ana").map((match) => match.id)).toEqual(["4", "1", "2"]);
  });

  it("lists a number once, with its label as people read it", () => {
    expect(phoneChoices(ana)).toEqual([{ label: "mobile", number: "+1 (555) 010-2000" }]);
    expect(phoneLabel(undefined)).toBe("phone");
  });
});

describe("the text the person sends", () => {
  it("sends a text as Nexus wrote it", () => {
    expect(textToSend({ to: "Ana", text: "Running late" }, "https://nexus.inc/r/x")).toEqual({ text: "Running late", linkMissing: false });
  });

  it("adds the referral link to an invitation, once", () => {
    const card = { to: "Ana", text: "Try Nexus with me", invite: true as const };
    expect(textToSend(card, "https://nexus.inc/r/x").text).toBe("Try Nexus with me\n\nhttps://nexus.inc/r/x");
    expect(textToSend({ ...card, text: "Join: https://nexus.inc/r/x" }, "https://nexus.inc/r/x").text).toBe("Join: https://nexus.inc/r/x");
  });

  it("says when an invitation has no link to add", () => {
    expect(textToSend({ to: "Ana", text: "Try Nexus", invite: true }, undefined)).toEqual({ text: "Try Nexus", linkMissing: true });
  });

  it("names the card with the verb first, and what happened in Messages", () => {
    expect(textCardTitle({ to: "Ana", text: "x", invite: true })).toBe("Invite Ana to Nexus");
    expect(textCardTitle({ to: "Ana", text: "x" })).toBe("Text Ana");
    expect(afterMessages("unknown")).toBe("Opened in Messages.");
    expect(afterMessages("cancelled")).toBeUndefined();
  });
});
