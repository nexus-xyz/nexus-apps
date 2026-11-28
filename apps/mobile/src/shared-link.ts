/**
 * A shared link opened in the app (`/a/<key>`): the key read as Nexus UI's viewer reads it
 * (`ui/src/app/a/[key]/route.ts`), and what the screen says when it does not open, in the viewer's words.
 */
import { CoreError, isArtifactId, type ArtifactAttachment, type ArtifactDetail, type ArtifactsClient, type PageFrame } from "@nexus/kit";

/** The artifact behind a link's key, as this reader may read it. */
export const readShared = (client: Pick<ArtifactsClient, "artifact" | "publicArtifact">, key: string): Promise<ArtifactDetail> =>
  isArtifactId(key) ? client.artifact(key) : client.publicArtifact(key);

/** A shared page's frame URL, minted for this reader: by id with the sign-in, or by a public link's token. */
export const frameShared = (client: Pick<ArtifactsClient, "frameArtifact" | "framePublicArtifact">, key: string, siteVersion?: number): Promise<PageFrame> => {
  const open = isArtifactId(key) ? client.frameArtifact : client.framePublicArtifact;
  return siteVersion === undefined ? open(key) : open(key, siteVersion);
};

/** An image of a question, as the frame: by id with the sign-in, or by a public link's token (an outsider's id read fails). */
export const attachmentShared = (client: Pick<ArtifactsClient, "artifactAttachment" | "publicArtifactAttachment">, key: string, attachmentId: string): Promise<ArtifactAttachment> =>
  isArtifactId(key) ? client.artifactAttachment(key, attachmentId) : client.publicArtifactAttachment(key, attachmentId);

/** Why a link did not open: shared with others (the reader may ask), gone, or another failure. */
export type LinkProblem = { kind: "access"; title: string; text: string; requested: boolean } | { kind: "gone" | "error"; title: string; text: string };

export function linkProblem(error: unknown): LinkProblem {
  if (error instanceof CoreError && error.status === 403 && error.code === "artifact_access_required") {
    const organization = typeof error.details?.organization === "string" ? error.details.organization : undefined;
    return {
      kind: "access",
      title: `You need access to open this ${error.details?.kind === "page" ? "page" : "chat"}`,
      text: `It was shared${organization ? ` from ${organization}` : ""} with specific people, and your account isn't one of them.`,
      requested: Boolean(error.details?.requested),
    };
  }
  if (error instanceof CoreError && (error.status === 404 || error.status === 410)) {
    return {
      kind: "gone",
      title: "This link doesn't work",
      text: "It may have been revoked by the person who shared it, it may have expired, or the address may be mistyped.",
    };
  }
  return { kind: "error", title: "This link didn't open", text: error instanceof Error ? error.message : String(error) };
}
