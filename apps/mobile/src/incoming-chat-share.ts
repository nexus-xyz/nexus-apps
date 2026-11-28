import { CHAT_FILE_LIMITS, CHAT_IMAGE_LIMITS } from "@nexus/kit";
import { clearSharedPayloads, getSharedPayloads } from "expo-sharing";
import { Platform } from "react-native";

type ImageShare = { kind: "image"; uri: string; name: string; mimeType: string };
type FileShare = { kind: "file"; uri: string; name: string };
type ChatShare = ImageShare | FileShare;

export function incomingChatShares(): ChatShare[] {
  if (Platform.OS !== "android") return [];
  const images: ImageShare[] = [];
  const files: FileShare[] = [];
  for (const payload of getSharedPayloads()) {
    const { value: uri, mimeType, shareType } = payload;
    if (!uri?.startsWith("content://") || !mimeType) continue;
    if (shareType === "image" && mimeType.startsWith("image/") && images.length < CHAT_IMAGE_LIMITS.count) {
      images.push({ kind: "image", uri, name: images.length ? `Shared image ${images.length + 1}` : "Shared image", mimeType });
      continue;
    }
    const extension = Object.entries(CHAT_FILE_LIMITS.types).find(([, type]) => type === mimeType)?.[0];
    if (extension && files.length < CHAT_FILE_LIMITS.count) {
      files.push({ kind: "file", uri, name: `Shared file${files.length ? ` ${files.length + 1}` : ""}.${extension}` });
    }
  }
  return [...images, ...files];
}

export function clearIncomingShare() {
  if (Platform.OS === "android") clearSharedPayloads();
}
