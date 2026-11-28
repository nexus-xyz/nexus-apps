import { useLocalSearchParams } from "expo-router";
import { clientIdentity, projectStart } from "@/connection-intent";
import { useCore, useStored } from "@/auth";
import ChatConversation from "@/chat-conversation";
import { NativeConversation } from "@/native-conversation";
import { ComputerComposer } from "@/computer-composer";

export default function Conversation() {
  const { id, source, computer, runOn, projectStart: start } = useLocalSearchParams<{ id?: string; source?: string; computer?: string; runOn?: string; projectStart?: string }>();
  const core = useCore();
  const account = clientIdentity(core);
  const last = useStored<string>("choice", "run-on");
  const target = runOn === "chat" ? undefined : computer ?? (last.load() !== "chat" ? last.load() : undefined);
  if (source === "synced" && id && id !== "new") return <NativeConversation key={`${account}:${id}`} threadId={id} />;
  if ((!id || id === "new") && target) return <ComputerComposer key={`${account}:${target}:${start ?? ""}`} computerId={target} startProject={projectStart(core, target, start)} />;
  return <ChatConversation key={`${account}:${id ?? "new"}`} />;
}
