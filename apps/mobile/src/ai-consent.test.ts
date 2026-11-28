import { describe, expect, it } from "vitest";
import { agreeTo, consentText, providerName, providersFor, withdrawable, type Agreed } from "./ai-consent";
import { AUTO } from "./meta";

const MODELS = [
  { id: AUTO, owned_by: "nexus" },
  { id: "anthropic/claude-opus-5.5", owned_by: "anthropic", provider: "anthropic" },
  { id: "deepseek/deepseek-v4.1-flash-1017", owned_by: "deepseek", provider: "baseten" },
  { id: "meta/muse-spark-open", owned_by: "meta", provider: "baseten" },
  // A model listed without its host (the TypeScript gateway's): its maker.
  { id: "openai/gpt-6-sol", owned_by: "openai" },
];

/** The kept list as `store.ts` keeps it, and how many times the question was asked. */
function device(kept?: string[]) {
  const store: Agreed & { kept?: string[] } = { kept, load: () => store.kept, save: (p) => (store.kept = p) };
  let asked = 0;
  const answer = (allow: boolean) => async () => (asked++, allow);
  return { store, answer, asked: () => asked };
}

describe("ai-consent", () => {
  it("names each host a message may reach, not the models' makers, whichever model is chosen", () => {
    const all = ["anthropic", "baseten", "openai"];
    expect(providersFor(AUTO, MODELS)).toEqual(all);
    expect(providersFor("deepseek/deepseek-v4.1-flash-1017", MODELS)).toEqual(all);
    // An older chat's model the list no longer has: its provider too.
    expect(providersFor("google/gemini-3.8-flash", MODELS)).toEqual([...all, "google"]);
    expect(providersFor(AUTO, [MODELS[0]])).toEqual([]);
  });

  it("asks once for every provider, and again only when a new one appears", async () => {
    const { store, answer, asked } = device();
    expect(await agreeTo(["anthropic", "baseten"], store, answer(true))).toBe(true);
    expect(store.kept).toEqual(["anthropic", "baseten"]);
    expect(await agreeTo(["anthropic", "baseten"], store, answer(false))).toBe(true);
    expect(asked()).toBe(1);
    expect(await agreeTo(["anthropic", "baseten", "deepinfra"], store, answer(true))).toBe(true);
    expect(asked()).toBe(2);
    expect(store.kept).toEqual(["anthropic", "baseten", "deepinfra"]);
  });

  it("keeps nothing on a decline, and asks every time while no provider is known", async () => {
    const { store, answer, asked } = device(["anthropic"]);
    expect(await agreeTo(["anthropic", "baseten"], store, answer(false))).toBe(false);
    expect(await agreeTo([], store, answer(true))).toBe(true);
    expect(await agreeTo([], store, answer(false))).toBe(false);
    expect(asked()).toBe(3);
    expect(store.kept).toEqual(["anthropic"]);
  });

  it("asks again after a withdrawal, and says it was not saved when storage refuses it", async () => {
    const gone = device(["anthropic"]);
    const removed = withdrawable({ ...gone.store, remove: () => (gone.store.kept = undefined) });
    expect(removed.withdraw()).toBe(true);
    expect(await agreeTo(["anthropic"], removed, gone.answer(false))).toBe(false);
    expect(gone.asked()).toBe(1);

    // Storage refuses the removal, or leaves the Allow in place: the chat still asks, until the next Allow.
    const refused = device(["anthropic"]);
    const busy = withdrawable({
      ...refused.store,
      remove: () => {
        throw new Error("SQLITE_BUSY");
      },
    });
    expect(busy.withdraw()).toBe(false);
    expect(await agreeTo(["anthropic"], busy, refused.answer(true))).toBe(true);
    expect(await agreeTo(["anthropic"], busy, refused.answer(false))).toBe(true);
    expect(refused.asked()).toBe(1);
    const left = device(["anthropic"]);
    const ignored = withdrawable({ ...left.store, remove: () => undefined });
    expect(ignored.withdraw()).toBe(false);
    expect(await agreeTo(["anthropic"], ignored, left.answer(false))).toBe(false);
    expect(left.asked()).toBe(1);
  });

  it("asks in the providers' own names", () => {
    const names = ["anthropic", "deepinfra", "baseten"].map((id) => providerName(id, [{ id: "deepinfra", name: "DeepInfra" }]));
    expect(names).toEqual(["Anthropic", "DeepInfra", "Baseten"]);
    expect(consentText(names).message).toBe(
      "To answer, Nexus sends your messages and any photos or other files you attach to the AI provider whose model answers: Anthropic, DeepInfra or Baseten.",
    );
    expect(consentText([], false).message).toBe("To answer, Nexus sends your messages to the AI provider whose model answers.");
    expect(consentText(["Baseten"]).title).toContain("Baseten");
  });
});
