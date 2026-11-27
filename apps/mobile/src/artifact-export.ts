import type { ArtifactsClient, PageExportFormat } from "@nexus/kit";
import { randomUUID } from "expo-crypto";
import { Directory, File, Paths } from "expo-file-system";
import { isAvailableAsync, shareAsync } from "expo-sharing";

type ExportFile = { filename: string; bytes: Uint8Array<ArrayBuffer>; mimeType: string };

export function arrayBufferOf(blob: Blob): Promise<ArrayBuffer> {
  if (typeof blob.arrayBuffer === "function") return blob.arrayBuffer();
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("Couldn’t read the exported file."));
    reader.onload = () => {
      if (reader.result instanceof ArrayBuffer) resolve(reader.result);
      else reject(new Error("Couldn’t read the exported file."));
    };
    reader.readAsArrayBuffer(blob);
  });
}

export function exportName(title: string, extension: string): string {
  const stem = title.replace(/[\\/:*?"<>|\u0000-\u001f]/g, "_").trim().replace(/^\.+|\.+$/g, "").slice(0, 96);
  return `${stem || "Artifact"}.${extension}`;
}

export async function shareArtifactFile(file: ExportFile, signal: AbortSignal): Promise<void> {
  if (!await isAvailableAsync()) throw new Error("File sharing is unavailable on this device.");
  if (signal.aborted) return;
  const folder = new Directory(Paths.cache, `nexus-artifact-${randomUUID()}`);
  folder.create();
  try {
    const safeName = file.filename.replace(/[\\/:*?"<>|\u0000-\u001f]/g, "_").replace(/^\.+/, "") || "Artifact";
    const saved = new File(folder, safeName);
    saved.writeSync(file.bytes);
    await shareAsync(saved.uri, { mimeType: file.mimeType, dialogTitle: safeName });
  } finally {
    if (folder.exists) folder.delete();
  }
}

export async function sharePageExport(
  artifacts: ArtifactsClient,
  id: string,
  format: PageExportFormat,
  options: Parameters<ArtifactsClient["exportPage"]>[2],
  signal: AbortSignal,
  filename?: string,
): Promise<void> {
  const file = await artifacts.exportPage(id, format, { ...options, signal });
  if (signal.aborted) return;
  await shareArtifactFile({
    filename: filename ?? file.filename,
    bytes: new Uint8Array(await arrayBufferOf(file.blob)),
    mimeType: file.blob.type || (format === "zip" ? "application/zip" : "application/octet-stream"),
  }, signal);
}
