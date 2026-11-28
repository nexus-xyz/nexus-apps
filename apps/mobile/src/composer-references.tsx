import { CHAT_FILE_LIMITS, type AgentClient, type AgentProject, type ChatAttachmentRef, type CoreClient } from "@nexus/kit";
import { getDocumentAsync } from "expo-document-picker";
import { File, Paths } from "expo-file-system";
import { useEffect, useMemo, useState } from "react";
import { ActivityIndicator, Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import { useArtifacts, useAuth, useCore } from "@/auth";
import { connectorLabel } from "@/activity";
import { pastedFile } from "@/composer-text";
import { Glass } from "@/glass";
import { haptic } from "@/haptics";
import { Icon } from "@/icon";
import { readProjectReference } from "@/project-reference";
import { font, text, useTheme } from "@/theme";

export type ContextFile = { name: string; data: string; bytes: number; mediaType: ChatAttachmentRef["mediaType"] };
type LoadReference = () => Promise<ContextFile | undefined>;
type Reference = { id: string; title: string; detail?: string } & ({ read: LoadReference } | { project: AgentProject });
type Scope = "artifacts" | "sessions" | "projects" | "people" | "connectors" | "repositories";
const SCOPES: { id: Scope; label: string }[] = [
  { id: "artifacts", label: "Artifacts" },
  { id: "sessions", label: "Sessions" },
  { id: "projects", label: "Projects" },
  { id: "people", label: "People" },
  { id: "connectors", label: "Connectors" },
  { id: "repositories", label: "Repositories" },
];

function markdown(title: string, content: string): ContextFile {
  const name = title.normalize("NFKD").replace(/[^\w\s-]/g, "").trim().replace(/\s+/g, "-").slice(0, 60);
  return { ...pastedFile(content), mediaType: "text/markdown", name: `${name || "context"}.md` };
}

export async function pickContextFile(): Promise<ContextFile | undefined> {
  const result = await getDocumentAsync({ type: Object.values(CHAT_FILE_LIMITS.types), copyToCacheDirectory: true });
  if (result.canceled) return;
  const [asset] = result.assets;
  if (!asset) return;
  try {
    return await readContextFile(asset.uri, asset.name, asset.size);
  } finally {
    // DocumentPicker copied the file into the app cache.
    try {
      const file = new File(asset.uri);
      if (asset.uri.startsWith(Paths.cache.uri) && file.exists) file.delete();
    } catch {
      // The cache can be cleared while the picker is open.
    }
  }
}

export async function readContextFile(uri: string, name: string, knownBytes?: number): Promise<ContextFile> {
  const file = new File(uri);
  const extension = name.split(".").pop()?.toLowerCase();
  const mediaType = Object.entries(CHAT_FILE_LIMITS.types).find(([key]) => key === extension)?.[1];
  if (!mediaType) throw new Error("Choose a CSV, TSV, JSON, text, Markdown, or XLSX file.");
  const bytes = knownBytes ?? file.size;
  if (!bytes || bytes > CHAT_FILE_LIMITS.bytes) throw new Error("Choose a nonempty file up to 5 MB.");
  const data = await file.base64();
  const decoded = Math.floor(data.length * 3 / 4) - (data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0);
  if (!decoded || decoded > CHAT_FILE_LIMITS.bytes) throw new Error("Choose a nonempty file up to 5 MB.");
  return { name, data: `data:${mediaType};base64,${data}`, bytes: decoded, mediaType };
}

type Results = { kind: "loading" } | { kind: "ready"; rows: Reference[] } | { kind: "error"; message: string };

export function ComposerReferences({ query, pending, onPick, onClose }: {
  query: string;
  pending: boolean;
  onPick: (load: LoadReference) => void;
  onClose: () => void;
}) {
  const theme = useTheme();
  const core = useCore();
  const artifacts = useArtifacts();
  const { agent } = useAuth();
  const [scope, setScope] = useState<Scope>("artifacts");
  const owner = useMemo(() => ({}), [agent, artifacts, core, query, scope]);
  const [held, setHeld] = useState<{ owner: object; results: Results }>();
  const [selected, setSelected] = useState<{ owner: object; project: AgentProject }>();
  const [retry, setRetry] = useState(0);
  const results: Results = held?.owner === owner ? held.results : { kind: "loading" };
  useEffect(() => {
    setHeld({ owner, results: { kind: "loading" } });
    const controller = new AbortController();
    const { signal } = controller;
    const q = query.trim();
    const contains = (name: string) => name.toLowerCase().includes(q.toLowerCase());
    async function list(): Promise<Reference[]> {
      switch (scope) {
        case "artifacts": {
          const response = await artifacts.artifacts({ status: "active", limit: 8, ...(q ? { q } : {}) });
          return response.artifacts.map(({ id, title }) => ({
            id, title, detail: "Attach as context",
            read: async () => markdown(title, await artifacts.exportArtifact(id)),
          }));
        }
        case "sessions": {
          const sessions = q
            ? (await core.search({ q, types: "chats", limit: 8 }, signal)).groups.flatMap((group) => group.results)
            : (await core.chatSessions({ limit: 8 }, signal)).sessions;
          return [...new Map(sessions.map(({ id, title }) => [id, { id, title }])).values()].map(({ id, title }) => ({
            id, title, detail: "Attach transcript",
            read: async () => markdown(title, await artifacts.exportChatSession(id)),
          }));
        }
        case "projects": {
          if (!agent) return [];
          const response = await agent.projects(signal);
          if (response.kind === "failed") throw new Error("Couldn’t read projects. Try again.");
          if (response.kind !== "list") return [];
          return response.list.projects
            .filter((project) => contains(project.name) || project.repos.some(contains))
            .slice(0, 8)
            .map((project) => ({
              id: project.id ?? `name:${project.name}`,
              title: project.name,
              detail: project.repos[0] ?? "Assistant project",
              project,
            }));
        }
        case "people": {
          if (!q) return [];
          const response = await core.search({ q, types: "people", limit: 8 }, signal);
          return response.groups.flatMap((group) => group.results).filter(({ entity }) => entity === "member")
            .map(({ id, title }) => ({
              id, title, detail: "Organization member",
              read: async () => markdown(title, `# Organization member\n\nName: ${title}\n`),
            }));
        }
        case "connectors": {
          const response = await core.connectors();
          return response.connectors.filter((entry) => entry.offered !== false && entry.usable === true && contains(connectorLabel(entry.name))).slice(0, 8)
            .map(({ name }) => ({
              id: name, title: connectorLabel(name), detail: "Connector",
              read: async () => markdown(name, `# Connector\n\nService: ${connectorLabel(name)}\n`),
            }));
        }
        case "repositories": {
          const response = await core.repositories();
          return response.repositories.filter((repo) => contains(repo.fullName)).slice(0, 8).map(({ id, fullName, url, defaultBranch }) => ({
            id, title: fullName, detail: defaultBranch,
            read: async () => markdown(fullName, `# Linked repository\n\nRepository: ${fullName}\nURL: ${url}\n${defaultBranch ? `Default branch: ${defaultBranch}\n` : ""}`),
          }));
        }
      }
    }

    const tick = setTimeout(() => {
      void list().then(
        (rows) => { if (!signal.aborted) setHeld({ owner, results: { kind: "ready", rows } }); },
        (error: unknown) => {
          if (!signal.aborted) setHeld({ owner, results: { kind: "error", message: error instanceof Error ? error.message : "Couldn’t load references." } });
        },
      );
    }, q ? 180 : 0);
    return () => { clearTimeout(tick); controller.abort(); };
  }, [agent, artifacts, core, owner, query, retry, scope]);

  if (agent && selected?.owner === owner) {
    return <ProjectReferencePreview key={selected.project.id ?? selected.project.name} agent={agent} core={core}
      project={selected.project} onBack={() => setSelected(undefined)} onAttach={(content) => {
        onPick(async () => markdown(selected.project.name, content));
        setSelected(undefined);
      }} />;
  }

  return (
    <Glass style={styles.card}>
      <View style={styles.head}>
        <Text style={[styles.heading, { color: theme.foreground }]} accessibilityRole="header">Add context</Text>
        <Pressable onPress={() => { haptic("tap"); onClose(); }} accessibilityRole="button" accessibilityLabel="Close context picker" style={styles.iconButton}>
          <Icon name="close" size={17} color={theme.muted} />
        </Pressable>
      </View>
      <ScrollView horizontal keyboardShouldPersistTaps="always" showsHorizontalScrollIndicator={false} contentContainerStyle={styles.scopes}>
        {SCOPES.filter(({ id }) => id !== "projects" || agent).map(({ id, label }) => (
          <Pressable key={id} onPress={() => { haptic("selection"); setScope(id); }} style={[styles.scope, id === scope && { backgroundColor: theme.fill }]}
            accessibilityRole="button" accessibilityState={{ selected: id === scope }}>
            <Text style={[styles.detail, { color: id === scope ? theme.foreground : theme.muted }]}>{label}</Text>
          </Pressable>
        ))}
      </ScrollView>
      <ScrollView style={styles.results} keyboardShouldPersistTaps="always">
        {results.kind === "loading" || pending ? <ActivityIndicator style={styles.status} color={theme.muted} /> : null}
        {results.kind === "error" ? <>
          <Text style={[styles.status, styles.detail, { color: theme.muted }]}>{results.message}</Text>
          <Pressable onPress={() => { haptic("tap"); setRetry((n) => n + 1); }} accessibilityRole="button" style={styles.row}>
            <Text style={[styles.label, { color: theme.foreground }]}>Try again</Text>
          </Pressable>
        </> : null}
        {results.kind === "ready" && !pending ? <>
          {results.rows.map((row) => (
            <Pressable key={row.id} onPress={() => {
              if ("read" in row) onPick(row.read);
              else { haptic("selection"); setSelected({ owner, project: row.project }); }
            }} accessibilityRole="button" style={styles.row}>
              <Text numberOfLines={1} style={[styles.label, { color: theme.foreground }]}>{row.title || "Untitled"}</Text>
              {row.detail ? <Text numberOfLines={1} style={[styles.detail, { color: theme.muted }]}>{row.detail}</Text> : null}
            </Pressable>
          ))}
          {results.rows.length ? null : <Text style={[styles.status, styles.detail, { color: theme.muted }]}>
            {scope === "people" && !query.trim() ? "Type a person’s name." : "No matching references."}
          </Text>}
        </> : null}
      </ScrollView>
      <Pressable style={styles.file} disabled={pending} onPress={() => onPick(pickContextFile)} accessibilityRole="button">
        <Icon name="file" size={17} color={theme.foreground} />
        <Text style={[styles.label, { color: theme.foreground }]}>Attach a file</Text>
      </Pressable>
    </Glass>
  );
}

function ProjectReferencePreview({ agent, core, project, onBack, onAttach }: {
  agent: AgentClient;
  core: CoreClient;
  project: AgentProject;
  onBack: () => void;
  onAttach: (content: string) => void;
}) {
  const theme = useTheme();
  const [content, setContent] = useState<string>();
  const [error, setError] = useState<string>();
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    const { signal } = controller;
    setError(undefined);
    void readProjectReference(agent, core, project, signal).then(
      (preview) => { if (!signal.aborted) setContent(preview); },
      (failure: unknown) => {
        if (!signal.aborted) setError(failure instanceof Error ? failure.message : "Couldn’t read this project.");
      },
    );
    return () => controller.abort();
  }, [agent, core, project, retry]);

  return (
    <Glass style={styles.card}>
      <View style={styles.head}>
        <Text style={[styles.heading, styles.previewTitle, { color: theme.foreground }]} accessibilityRole="header" numberOfLines={2}>Review project reference</Text>
        <Pressable style={styles.iconButton} accessibilityRole="button" accessibilityLabel="Back to project results"
          onPress={() => { haptic("tap"); onBack(); }}>
          <Icon name="close" size={17} color={theme.muted} />
        </Pressable>
      </View>
      {content ? <>
        <ScrollView style={styles.results} keyboardShouldPersistTaps="always">
          <Text style={[styles.status, styles.detail, { color: theme.foreground }]} selectable>{content}</Text>
        </ScrollView>
        <Pressable style={({ pressed }) => [styles.row, styles.previewConfirm, { backgroundColor: theme.fill }, pressed && { opacity: 0.8 }]}
          onPress={() => onAttach(content)} accessibilityRole="button">
          <Text style={[styles.label, { color: theme.primary }]}>Attach project reference</Text>
        </Pressable>
      </> : error ? <>
        <Text style={[styles.status, styles.detail, { color: theme.muted }]}>{error}</Text>
        <Pressable style={styles.row} onPress={() => { haptic("tap"); setRetry((value) => value + 1); }} accessibilityRole="button">
          <Text style={[styles.label, { color: theme.primary }]}>Try again</Text>
        </Pressable>
      </> : <ActivityIndicator style={styles.status} color={theme.muted} accessibilityLabel="Reading project notes and file paths" />}
    </Glass>
  );
}

const styles = StyleSheet.create({
  card: { marginHorizontal: 12, marginBottom: 8, borderRadius: 20, borderCurve: "continuous", overflow: "hidden" },
  head: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", paddingLeft: 16, paddingRight: 4 },
  heading: { ...text.base, fontFamily: font.medium },
  previewTitle: { flex: 1 },
  previewConfirm: { marginHorizontal: 12, marginBottom: 8, borderRadius: 12 },
  iconButton: { width: 44, height: 44, alignItems: "center", justifyContent: "center" },
  scopes: { paddingHorizontal: 12, gap: 4 },
  scope: { minHeight: 44, paddingHorizontal: 10, justifyContent: "center", borderRadius: 16 },
  results: { maxHeight: 152 },
  row: { minHeight: 44, justifyContent: "center", paddingHorizontal: 16, paddingVertical: 4 },
  status: { paddingVertical: 12, paddingHorizontal: 16 },
  label: { ...text.secondary, fontFamily: font.medium },
  detail: { ...text.caption, fontFamily: font.regular },
  file: { flexDirection: "row", alignItems: "center", gap: 8, minHeight: 44, paddingHorizontal: 16 },
});
