import { CHAT_IMAGE_LIMITS } from "@nexus/kit";
import { afterEach, describe, expect, it, vi } from "vitest";
import { base64Bytes, fit, forgetImages, KEPT_CHARS, keepSent, readImage, sendName, sendType, toInput, within, type ComposerImage } from "./chat-images";

const MB = 1024 * 1024;
const image = (id: string, bytes = 1000): ComposerImage => ({ id, name: `${id}.png`, mediaType: "image/png", uri: "data:image/png;base64,aGk=", bytes });

describe("preparing an image to send", () => {
  it("sends an iPhone's HEIC photo as JPEG, WebP as PNG, and a GIF as it is", () => {
    expect(sendType("image/heic")).toBe("image/jpeg");
    expect(sendType(undefined)).toBe("image/jpeg");
    expect(sendType("image/webp")).toBe("image/png");
    expect(sendType("image/gif")).toBe("image/gif");
    expect(sendName("IMG_0001.HEIC", "image/jpeg")).toBe("IMG_0001.jpg");
    expect(sendName(null, "image/png")).toBe("image.png");
  });

  it("counts the bytes base64 holds and sends a data URL", () => {
    expect(base64Bytes("aGk=")).toBe(2);
    expect(base64Bytes("aGVsbG8=")).toBe(5);
    expect(toInput(image("a"))).toEqual({ data: "data:image/png;base64,aGk=", name: "a.png" });
  });

  it("draws one image alone within its box, keeping its shape", () => {
    expect(within(4000, 3000, 260, 300)).toEqual({ width: 260, height: 195 });
    expect(within(100, 80, 260, 300)).toEqual({ width: 100, height: 80 });
    expect(within(0, 0, 260, 300)).toEqual({ width: 260, height: 195 });
  });
});

describe("fit", () => {
  it("keeps what fits and says, once, why the rest was left out", () => {
    const eight = Array.from({ length: CHAT_IMAGE_LIMITS.count - 1 }, (_, i) => image(`i${i}`));
    const result = fit(eight, [new Error("b.png is too large."), image("x"), image("y"), image("z")]);
    expect(result.images.map((i) => i.id)).toEqual([...eight.map((i) => i.id), "x"]);
    expect(result.problem).toBe("b.png is too large. A message takes at most 8 images.");
  });

  it("stops at Gateway's total for one message", () => {
    const result = fit([image("a", 5 * MB), image("b", 5 * MB)], [image("c", 5 * MB), image("d", 5 * MB), image("e", 1)]);
    expect(result.images.map((i) => i.id)).toEqual(["a", "b", "c", "d"]);
    expect(result.problem).toBe("A message's images may total at most 20 MB.");
  });

  it("has no problem when everything fits", () => {
    expect(fit([], [image("a")])).toEqual({ images: [image("a")] });
  });
});

describe("readImage", () => {
  afterEach(forgetImages);

  it("reads a stored image once, and again after a failed read", async () => {
    const load = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue({ mediaType: "image/png", data: "aGk=" });
    await expect(readImage("session:s1:a1", load)).rejects.toThrow("offline");
    await expect(readImage("session:s1:a1", load)).resolves.toEqual({ mediaType: "image/png", data: "aGk=" });
    await readImage("session:s1:a1", load);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("shows a question just sent from what was attached, without reading it back", async () => {
    keepSent("s1", [{ id: "att_1", type: "image", mediaType: "image/png", bytes: 2 }], [image("local-1")]);
    const load = vi.fn();
    await expect(readImage("session:s1:att_1", load)).resolves.toEqual({ mediaType: "image/png", data: "aGk=" });
    expect(load).not.toHaveBeenCalled();
  });

  it("lets the oldest images go once they pass the memory it may hold", async () => {
    const big = { mediaType: "image/png", data: "a".repeat(KEPT_CHARS / 2) };
    const load = vi.fn().mockResolvedValue(big);
    for (const key of ["k1", "k2", "k3"]) await readImage(key, load);
    await readImage("k3", load);
    await readImage("k1", load);
    expect(load).toHaveBeenCalledTimes(4);
  });

  it("forgets every image on sign-out and on an organization switch", async () => {
    keepSent("s1", [{ id: "att_1", type: "image", mediaType: "image/png", bytes: 2 }], [image("local-1")]);
    forgetImages();
    const load = vi.fn().mockResolvedValue({ mediaType: "image/png", data: "aGk=" });
    await readImage("session:s1:att_1", load);
    expect(load).toHaveBeenCalledTimes(1);
  });
});
