import { exportName, shareArtifactFile, sharePageExport } from "@/artifact-export";
import { ArtifactExportSheet } from "@/artifact-export-sheet";
import { useArtifacts } from "@/auth";

export function DocExportSheet({ pageId, title, version, visible, onClose }: {
  pageId: string;
  title: string;
  version: number;
  visible: boolean;
  onClose: () => void;
}) {
  const artifacts = useArtifacts();

  async function shareDoc(format: "md" | "docx", signal: AbortSignal) {
    const source = await artifacts.pageSource(pageId, version);
    if (signal.aborted) return;
    const bytes = format === "md"
      ? new TextEncoder().encode(source)
      : await (await import("@/doc-word")).wordDocument(title, source);
    await shareArtifactFile({
      filename: exportName(title, format),
      bytes,
      mimeType: format === "md" ? "text/markdown" : "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    }, signal);
  }

  return <ArtifactExportSheet key={version} title={`Export v${version}`} visible={visible} onClose={onClose} options={[
    { label: "PDF", run: (signal) => sharePageExport(artifacts, pageId, "pdf", { version }, signal) },
    { label: "Markdown", run: (signal) => shareDoc("md", signal) },
    { label: "Word (.docx)", run: (signal) => shareDoc("docx", signal) },
  ]} />;
}
