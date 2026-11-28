import { describe, expect, it } from "vitest";
import { asker, nativeCall, replyScript, type Choice } from "./page-bridge";

const FRAME = "https://abc123.nexususercontent.dev/v/pag_1/live?exp=1&sig=00";

describe("a page's call from the WebView", () => {
  it("is the message's JSON, while the WebView shows the frame's origin", () => {
    const call = { nexus: 1, id: "d.1", method: "complete", params: "Hi" };
    expect(nativeCall(FRAME, "https://abc123.nexususercontent.dev/v/pag_1/live?exp=1&sig=00&r=v2", JSON.stringify(call))).toEqual(call);
  });

  it("is nothing from another origin, or when it is not JSON", () => {
    const text = JSON.stringify({ nexus: 1, id: "d.1", method: "complete", params: "Hi" });
    expect(nativeCall(FRAME, "https://other.nexususercontent.dev/v/pag_2/1", text)).toBeUndefined();
    expect(nativeCall(FRAME, "https://abc123.nexususercontent.dev@evil.example/", text)).toBeUndefined();
    expect(nativeCall(FRAME, "about:blank", text)).toBeUndefined();
    expect(nativeCall(FRAME, FRAME, "not json")).toBeUndefined();
  });
});

describe("the reply script", () => {
  /** Runs the script against a fake window, and returns what it posted. */
  function run(script: string) {
    const posted: unknown[] = [];
    const result = new Function("window", `return eval(${JSON.stringify(script)})`)({ postMessage: (data: unknown, target: string) => posted.push({ data, target }) });
    return { posted, result };
  }

  it("posts the reply to the page's own window, as data", () => {
    const answer = `"); window.stolen = true; ("\u2028</script>`;
    const script = replyScript({ nexus: 1, id: "d.1", result: answer });
    expect(script).not.toContain("\u2028");
    const { posted, result } = run(script);
    expect(posted).toEqual([{ data: { nexus: 1, id: "d.1", result: answer }, target: "*" }]);
    expect(result).toBe(true);
  });

  it("carries a file's bytes as a Uint8Array, even a large one", () => {
    const bytes = new Uint8Array(100_000).map((_, index) => (index * 7) % 256);
    const { posted, result } = run(replyScript({ nexus: 1, id: "d.3", result: bytes }));
    expect(result).toBe(true);
    const [{ data, target }] = posted as { data: { nexus: number; id: string; result: Uint8Array }; target: string }[];
    expect(target).toBe("*");
    expect({ nexus: data.nexus, id: data.id }).toEqual({ nexus: 1, id: "d.3" });
    expect(data.result).toBeInstanceOf(Uint8Array);
    expect(data.result).toEqual(bytes);
  });

  it("carries an error's code and message", () => {
    const { posted } = run(replyScript({ nexus: 1, id: "d.2", error: { code: "denied", message: "No." } }));
    expect(posted).toEqual([{ data: { nexus: 1, id: "d.2", error: { code: "denied", message: "No." } }, target: "*" }]);
  });
});

describe("asking the reader", () => {
  /** A kept choice, as the device store keeps it, and the prompts shown so far. */
  function setup(kept?: boolean) {
    let value = kept;
    const saved: boolean[] = [];
    const choice: Choice = { load: () => value, save: (allow) => void (saved.push(allow), (value = allow)) };
    const prompts: ((allow: boolean) => void)[] = [];
    return { ask: asker(() => choice, (decide) => prompts.push(decide)), saved, prompts };
  }

  it("answers from a kept choice without asking", async () => {
    for (const kept of [true, false]) {
      const { ask, prompts } = setup(kept);
      await expect(ask()).resolves.toBe(kept);
      expect(prompts).toHaveLength(0);
    }
  });

  it("asks once for calls that arrive together, keeps the answer, and asks no more", async () => {
    const { ask, saved, prompts } = setup();
    const first = ask();
    const second = ask();
    expect(prompts).toHaveLength(1);
    prompts[0]!(true);
    await expect(Promise.all([first, second])).resolves.toEqual([true, true]);
    expect(saved).toEqual([true]);
    await expect(ask()).resolves.toBe(true);
    expect(prompts).toHaveLength(1);
  });

  it("keeps a Deny, so the page is refused without asking again", async () => {
    const { ask, saved, prompts } = setup();
    const answer = ask();
    prompts[0]!(false);
    await expect(answer).resolves.toBe(false);
    expect(saved).toEqual([false]);
    await expect(ask()).resolves.toBe(false);
    expect(prompts).toHaveLength(1);
  });
});
