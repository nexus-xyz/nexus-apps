import { CoreError, isArtifactId, type ArtifactDetail } from "@nexus/kit";
import { describe, expect, it, vi } from "vitest";
import { attachmentShared, frameShared, linkProblem, readShared } from "./shared-link";

const detail = { id: "art_1" } as ArtifactDetail;

describe("a shared link opened in the app", () => {
  it("reads an artifact's id with the sign-in, and anything else as a public link's token", async () => {
    const client = { artifact: vi.fn(async () => detail), publicArtifact: vi.fn(async () => detail) };
    await readShared(client, "art_1");
    expect(client.artifact).toHaveBeenCalledWith("art_1");
    await readShared(client, "Xy9_token-0123456789abcdefghijklmnopq");
    expect(client.publicArtifact).toHaveBeenCalledWith("Xy9_token-0123456789abcdefghijklmnopq");
    expect(isArtifactId("art_")).toBe(false);
    const frames = { frameArtifact: vi.fn(async () => ({ url: "https://p.test/1", expiresAt: "" })), framePublicArtifact: vi.fn(async () => ({ url: "https://p.test/2", expiresAt: "" })) };
    await frameShared(frames, "art_1");
    await frameShared(frames, "Xy9_token");
    expect(frames.frameArtifact).toHaveBeenCalledWith("art_1");
    expect(frames.framePublicArtifact).toHaveBeenCalledWith("Xy9_token");
    await frameShared(frames, "art_1", 5);
    await frameShared(frames, "Xy9_token", 5);
    expect(frames.frameArtifact).toHaveBeenLastCalledWith("art_1", 5);
    expect(frames.framePublicArtifact).toHaveBeenLastCalledWith("Xy9_token", 5);
    const image = { artifactId: "art_1", messageId: "m", id: "att_1", mediaType: "image/png", data: "", createdAt: "" } as never;
    const images = { artifactAttachment: vi.fn(async () => image), publicArtifactAttachment: vi.fn(async () => image) };
    await attachmentShared(images, "art_1", "att_1");
    await attachmentShared(images, "Xy9_token", "att_2");
    expect(images.artifactAttachment).toHaveBeenCalledWith("art_1", "att_1");
    expect(images.publicArtifactAttachment).toHaveBeenCalledWith("Xy9_token", "att_2");
  });

  it("asks for access as the viewer does, naming a page or a chat and the organization", () => {
    const problem = linkProblem(new CoreError(403, "ask", "artifact_access_required", undefined, { kind: "page", organization: "Acme" }));
    expect(problem).toEqual({
      kind: "access",
      title: "You need access to open this page",
      text: "It was shared from Acme with specific people, and your account isn't one of them.",
      requested: false,
    });
    const asked = linkProblem(new CoreError(403, "ask", "artifact_access_required", undefined, { requested: true }));
    expect(asked).toMatchObject({ kind: "access", title: "You need access to open this chat", text: "It was shared with specific people, and your account isn't one of them.", requested: true });
  });

  it("says a revoked, expired or unknown link doesn't work, and passes other failures on", () => {
    expect(linkProblem(new CoreError(404, "no artifact at this link", "artifact_not_found"))).toMatchObject({ kind: "gone", title: "This link doesn't work" });
    expect(linkProblem(new CoreError(410, "expired"))).toMatchObject({ kind: "gone" });
    expect(linkProblem(new CoreError(403, "verify your email", "email_not_verified"))).toEqual({ kind: "error", title: "This link didn't open", text: "verify your email" });
    expect(linkProblem(new Error("Network request failed"))).toEqual({ kind: "error", title: "This link didn't open", text: "Network request failed" });
  });
});
