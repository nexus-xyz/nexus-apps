import { CHAT_IMAGE_LIMITS, type ChatAttachmentRef } from "@nexus/kit";
import * as Clipboard from "expo-clipboard";
import { File, Paths } from "expo-file-system";
import { ImageManipulator, SaveFormat } from "expo-image-manipulator";
import * as ImagePicker from "expo-image-picker";
import { useEffect, useRef, useState } from "react";
import { ActivityIndicator, FlatList, Image, Modal, Platform, Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import {
  base64Bytes,
  dataUrl,
  detailOf,
  fit,
  readImage,
  room,
  SEND_SIDE,
  sendName,
  sendType,
  TOO_MANY,
  tooLarge,
  within,
  type ComposerImage,
  type ImageData,
} from "@/chat-images";
import type { DropdownOption } from "@/dropdown";
import { Icon } from "@/icon";
import { haptic } from "@/haptics";
import { font, text, type Theme } from "@/theme";

/**
 * Images in chat, as Nexus UI and the SwiftUI app had them. The composer attaches them from Photos, the
 * camera or the clipboard; each waits above the text as a thumbnail you can open or remove, and a question may be
 * images alone. A question's images show above it, one large or several as tiles, and each opens full size. Every image
 * but a GIF is drawn again before it is sent (`chat-images.ts` says how), so a photo's location never leaves the phone.
 * A GIF goes as it is, so it keeps moving; the format has no EXIF block, so it carries no camera location.
 */

export type AttachFrom = "photos" | "camera" | "paste";

/** The attach menu's rows. */
export const attachFrom = (theme: Theme): DropdownOption<AttachFrom>[] => [
  { value: "photos", label: "Photos", leading: <Icon name="photo" size={18} color={theme.foreground} /> },
  { value: "camera", label: "Camera", leading: <Icon name="camera" size={18} color={theme.foreground} /> },
  { value: "paste", label: "Paste image", leading: <Icon name="paste" size={18} color={theme.foreground} /> },
];

let next = 0;

/** A file to prepare: picked, taken, or a paste written to the cache. */
type Picked = { uri: string; name?: string | null; mimeType?: string | null };

/**
 * A file read and no longer needed: the picker's copy, the manipulator's output, a paste. Only files in the app's own
 * cache; nothing of an image stays on the device.
 */
function discard(uri: string) {
  try {
    const file = new File(uri);
    if (uri.startsWith(Paths.cache.uri) && file.exists) file.delete();
  } catch {
    // Left in the cache, which the system clears.
  }
}

/** A picked or pasted file as an image to send, or why not. */
async function prepare(file: Picked): Promise<ComposerImage> {
  const mediaType = sendType(file.mimeType);
  const name = sendName(file.name, mediaType);
  next += 1;
  const id = `local-${Date.now()}-${next}`;
  if (mediaType === "image/gif") {
    const gif = new File(file.uri);
    if ((gif.size ?? 0) > CHAT_IMAGE_LIMITS.bytes) throw new Error(tooLarge(name));
    const data = await gif.base64();
    return { id, name, mediaType, uri: dataUrl({ mediaType, data }), bytes: base64Bytes(data) };
  }
  // Decoded no larger than SEND_SIDE (never whole: a 24 MP photo is about 100 MB decoded), upright, and saved again
  // without its metadata.
  const image = await ImageManipulator.manipulate(file.uri, { maxWidth: SEND_SIDE, maxHeight: SEND_SIDE }).renderAsync();
  try {
    let type = mediaType;
    let saved = await image.saveAsync({ format: type === "image/png" ? SaveFormat.PNG : SaveFormat.JPEG, compress: 0.92, base64: true });
    discard(saved.uri);
    if (type === "image/png" && base64Bytes(saved.base64 ?? "") > CHAT_IMAGE_LIMITS.bytes) {
      type = "image/jpeg";
      saved = await image.saveAsync({ format: SaveFormat.JPEG, compress: 0.85, base64: true });
      discard(saved.uri);
    }
    const data = saved.base64 ?? "";
    const bytes = base64Bytes(data);
    if (!data) throw new Error(`${name} could not be read.`);
    if (bytes > CHAT_IMAGE_LIMITS.bytes) throw new Error(tooLarge(name));
    return { id, name: type === mediaType ? name : sendName(name, type), mediaType: type, uri: dataUrl({ mediaType: type, data }), bytes, width: saved.width, height: saved.height };
  } finally {
    image.release();
  }
}

/** Files from `from`, at most `room` of them; none when the picker was cancelled. */
async function pick(from: AttachFrom, room: number): Promise<Picked[]> {
  if (from === "paste") {
    const image = await Clipboard.getImageAsync({ format: "png" });
    if (!image) throw new Error("There is no image to paste.");
    // Written to a file, so a paste is prepared as a picked image is.
    const pasted = new File(Paths.cache, `pasted-${Date.now()}.png`);
    pasted.writeSync(image.data.slice(image.data.indexOf(",") + 1), { encoding: "base64" });
    return [{ uri: pasted.uri, name: "Pasted image", mimeType: "image/png" }];
  }
  if (from === "camera" && !(await ImagePicker.requestCameraPermissionsAsync()).granted) {
    throw new Error("Nexus may not use the camera. Allow it in Settings.");
  }
  const options: ImagePicker.ImagePickerOptions = { mediaTypes: "images", quality: 1, exif: false };
  const picked =
    from === "camera"
      ? await ImagePicker.launchCameraAsync(options)
      : await ImagePicker.launchImageLibraryAsync({ ...options, allowsMultipleSelection: true, selectionLimit: room, orderedSelection: true });
  return picked.canceled ? [] : picked.assets.map((asset) => ({ uri: asset.uri, name: asset.fileName, mimeType: asset.mimeType }));
}

/**
 * The composer's images: attach from Photos, the camera or the clipboard; add photos shared from another app; remove
 * one; and why one was left out. One attach at a time; an image joins what the composer holds when it is ready, not
 * what it held when it was picked.
 */
export function useComposerImages() {
  const [images, setImages] = useState<ComposerImage[]>([]);
  const [problem, setProblem] = useState<string>();
  const [preparing, setPreparing] = useState(false);
  const latest = useRef(images);
  latest.current = images;
  const attaching = useRef(false);
  const store = (next: ComposerImage[]) => {
    latest.current = next;
    setImages(next);
  };

  /** Prepares `files` one at a time, adds them, and says what was left out, after `note` when there is one. */
  async function prepareAll(files: Picked[], note?: string) {
    setPreparing(true);
    // One at a time: each decodes a photo, and several at once could take the app past its memory.
    const prepared: (ComposerImage | Error)[] = [];
    for (const file of files) prepared.push(await prepare(file).catch((e: unknown) => (e instanceof Error ? e : new Error(String(e)))));
    const added = fit(latest.current, prepared);
    if (added.images.length > latest.current.length) haptic("success");
    store(added.images);
    setProblem([note, added.problem].filter(Boolean).join(" ") || undefined);
  }

  async function attach(from: AttachFrom) {
    if (attaching.current) return;
    setProblem(undefined);
    if (!room(latest.current)) return setProblem(TOO_MANY);
    attaching.current = true;
    let files: Picked[] = [];
    try {
      files = await pick(from, room(latest.current));
      if (!files.length) return;
      await prepareAll(files);
    } catch (e) {
      setProblem(e instanceof Error ? e.message : String(e));
    } finally {
      attaching.current = false;
      setPreparing(false);
      for (const file of files) discard(file.uri);
    }
  }
  /**
   * Photos shared from another app, already copied into the cache: prepared as a picked photo is, so they
   * are drawn again without their metadata and kept within Gateway's limits. `note` says what the share left out.
   */
  async function add(files: Picked[], note?: string) {
    // They left the inbox already, so they wait for an attach in progress rather than being dropped.
    while (attaching.current) await new Promise((resolve) => setTimeout(resolve, 100));
    attaching.current = true;
    try {
      if (files.length) await prepareAll(files, note);
      else setProblem(note);
    } finally {
      attaching.current = false;
      setPreparing(false);
      for (const file of files) discard(file.uri);
    }
  }
  return {
    images,
    problem,
    preparing,
    attach,
    add,
    remove: (id: string) => {
      const index = latest.current.findIndex((image) => image.id === id);
      const image = latest.current[index];
      if (!image) return;
      store(latest.current.filter((item) => item.id !== id));
      return { image, index };
    },
    restore: ({ image, index }: { image: ComposerImage; index: number }) => {
      const current = latest.current;
      if (current.some((item) => item.id === image.id)) return false;
      const added = fit(current, [image]);
      setProblem(added.problem);
      if (added.images.length === current.length) return false;
      store([...current.slice(0, index), image, ...current.slice(index)]);
      return true;
    },
    clear: () => {
      store([]);
      setProblem(undefined);
    },
  };
}

/** One image as the viewer shows it: its data URL once it is read, its name and its size. */
type Viewed = { key: string; uri?: string; name: string; detail: string; width?: number; height?: number };

/** The composer's images, waiting to be sent: open one, or remove it; one being prepared; and why one was left out. */
export function ImageTray({ images, problem, preparing, onRemove, theme }: { images: ComposerImage[]; problem?: string; preparing: boolean; onRemove: (id: string) => void; theme: Theme }) {
  const [open, setOpen] = useState<number | null>(null);
  if (!images.length && !problem && !preparing) return null;
  const viewed = images.map((image) => ({ key: image.id, uri: image.uri, name: image.name, detail: detailOf(image), width: image.width, height: image.height }));
  return (
    <View style={styles.tray}>
      {images.length || preparing ? (
        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.trayList} keyboardShouldPersistTaps="handled" accessibilityLabel="Images to send">
          {images.map((image, index) => (
            <View key={image.id} style={styles.trayTile}>
              <Pressable onPress={() => { haptic("tap"); setOpen(index); }} accessibilityRole="imagebutton" accessibilityLabel={`Open ${image.name}`}>
                <Image source={{ uri: image.uri }} style={[styles.trayImage, { backgroundColor: theme.userBubble }]} />
              </Pressable>
              <Pressable
                style={styles.remove}
                onPress={() => onRemove(image.id)}
                accessibilityRole="button"
                accessibilityLabel={`Remove ${image.name}`}
              >
                <View style={[styles.removeBadge, { backgroundColor: theme.foreground, borderColor: theme.card }]}>
                  <Icon name="close" size={9} color={theme.screen} />
                </View>
              </Pressable>
            </View>
          ))}
          {preparing ? (
            <View style={[styles.trayImage, styles.missing, { backgroundColor: theme.userBubble }]} accessibilityLabel="Preparing the image">
              <ActivityIndicator color={theme.muted} />
            </View>
          ) : null}
        </ScrollView>
      ) : null}
      {problem ? (
        <Text style={[styles.problem, { color: theme.danger }]} accessibilityRole="alert" accessibilityLiveRegion="polite">
          {problem}
        </Text>
      ) : null}
      <ImageViewer images={viewed} index={open} onIndex={setOpen} onClose={() => setOpen(null)} />
    </View>
  );
}

/** Where a question's stored images are read: its author's own chat, or a shared artifact (whoever may read it). */
export type ImageSource = { key: string; load: (attachmentId: string) => Promise<ImageData> };

/** One stored image's data URL once it is read, or that it could not be. */
function useStoredImage(source: ImageSource | undefined, attachment: ChatAttachmentRef): { uri?: string; missing: boolean } {
  const [shown, setShown] = useState<{ uri?: string; missing: boolean }>({ missing: !source });
  useEffect(() => {
    if (!source) return;
    let gone = false;
    readImage(`${source.key}:${attachment.id}`, () => source.load(attachment.id)).then(
      (image) => !gone && setShown({ uri: dataUrl(image), missing: false }),
      () => !gone && setShown({ missing: true }),
    );
    return () => {
      gone = true;
    };
  }, [source?.key, attachment.id]);
  return shown;
}

/** One image alone is drawn this large at most; several are square tiles, three to a row. */
const SINGLE = { width: 260, height: 300 };
const TILE = 92;

/** A question's images, above its text: what was attached while it is sent, then Gateway's copies, read from `source`. */
export function QuestionImages({ attachments, previews, source, theme }: { attachments?: ChatAttachmentRef[]; previews?: ComposerImage[]; source?: ImageSource; theme: Theme }) {
  const [open, setOpen] = useState<number | null>(null);
  const [uris, setUris] = useState<Record<string, string>>({});
  const items = previews?.length
    ? previews.map((image) => ({ key: image.id, name: image.name, bytes: image.bytes, width: image.width, height: image.height, uri: image.uri as string | undefined }))
    : (attachments ?? []).map((a) => ({ key: a.id, name: a.name ?? "Image", bytes: a.bytes, width: a.width, height: a.height, uri: uris[a.id] }));
  if (!items.length) return null;
  const single = items.length === 1;
  const viewed: Viewed[] = items.map((item) => ({ ...item, detail: detailOf(item) }));
  return (
    <View style={styles.questionImages} accessibilityLabel={single ? "Image" : `${items.length} images`}>
      {items.map((item, index) => {
        const size = single ? within(item.width ?? 0, item.height ?? 0, SINGLE.width, SINGLE.height) : { width: TILE, height: TILE };
        const attachment = previews?.length ? undefined : attachments?.[index];
        return attachment ? (
          <StoredTile
            key={item.key}
            source={source}
            attachment={attachment}
            size={size}
            theme={theme}
            onOpen={() => setOpen(index)}
            onRead={(uri) => setUris((current) => (current[attachment.id] === uri ? current : { ...current, [attachment.id]: uri }))}
          />
        ) : (
          <Pressable key={item.key} onPress={() => { haptic("tap"); setOpen(index); }} accessibilityRole="imagebutton" accessibilityLabel={`Open ${item.name}`}>
            <Image source={{ uri: item.uri }} style={[styles.questionImage, size, { backgroundColor: theme.userBubble }]} />
          </Pressable>
        );
      })}
      <ImageViewer images={viewed} index={open} onIndex={setOpen} onClose={() => setOpen(null)} />
    </View>
  );
}

function StoredTile({
  source,
  attachment,
  size,
  theme,
  onOpen,
  onRead,
}: {
  source?: ImageSource;
  attachment: ChatAttachmentRef;
  size: { width: number; height: number };
  theme: Theme;
  onOpen: () => void;
  /** Tells the viewer what this tile shows, once it is read. */
  onRead: (uri: string) => void;
}) {
  const { uri, missing } = useStoredImage(source, attachment);
  useEffect(() => {
    if (uri) onRead(uri);
  }, [uri]);
  const name = attachment.name ?? "Image";
  if (missing) {
    return (
      <View style={[styles.questionImage, styles.missing, size, { backgroundColor: theme.userBubble }]} accessibilityLabel="This image isn't available">
        <Icon name="photo" size={18} color={theme.muted} />
      </View>
    );
  }
  return (
    <Pressable onPress={onOpen} accessibilityRole="imagebutton" accessibilityLabel={`Open ${name}`}>
      {uri ? (
        <Image source={{ uri }} style={[styles.questionImage, size, { backgroundColor: theme.userBubble }]} />
      ) : (
        <View style={[styles.questionImage, size, { backgroundColor: theme.userBubble }]} accessibilityLabel="Loading the image" />
      )}
    </Pressable>
  );
}

/**
 * Full size, one of `images` at a time, on black: swipe between them, pinch to zoom on iOS, and Close (or Android's
 * Back) to leave. Its name, place and size are under it, as the web's viewer has them.
 */
function ImageViewer({ images, index, onIndex, onClose }: { images: Viewed[]; index: number | null; onIndex: (index: number) => void; onClose: () => void }) {
  const { width, height } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const shown = index === null ? undefined : images[index];
  return (
    <Modal visible={!!shown} transparent animationType="fade" onRequestClose={onClose} statusBarTranslucent supportedOrientations={["portrait", "landscape"]}>
      <View style={styles.viewer}>
        <FlatList
          data={images}
          horizontal
          pagingEnabled
          showsHorizontalScrollIndicator={false}
          initialScrollIndex={index ?? 0}
          getItemLayout={(_, i) => ({ length: width, offset: width * i, index: i })}
          keyExtractor={(image) => image.key}
          // The list still reports a scroll end while the viewer fades out after Close; that one must not open it again.
          onMomentumScrollEnd={(e) => index !== null && onIndex(Math.round(e.nativeEvent.contentOffset.x / width))}
          renderItem={({ item }) => (
            <ScrollView
              style={{ width, height }}
              contentContainerStyle={styles.stage}
              maximumZoomScale={4}
              minimumZoomScale={1}
              centerContent
              showsHorizontalScrollIndicator={false}
              showsVerticalScrollIndicator={false}
            >
              {item.uri ? (
                <Image source={{ uri: item.uri }} style={{ width, height }} resizeMode="contain" accessibilityLabel={item.name} />
              ) : (
                <Text style={styles.viewerWait}>Loading the image…</Text>
              )}
            </ScrollView>
          )}
        />
        <View style={[styles.viewerTop, { paddingTop: insets.top + 8 }]}>
          <Pressable style={styles.viewerClose} onPress={onClose} accessibilityRole="button" accessibilityLabel="Close" hitSlop={8}>
            <Icon name="close" size={18} color="#ffffff" />
          </Pressable>
        </View>
        {shown ? (
          <View style={[styles.viewerBar, { paddingBottom: Math.max(insets.bottom, 16) }]} pointerEvents="none">
            <Text style={styles.viewerName} numberOfLines={1}>
              {shown.name}
            </Text>
            <Text style={styles.viewerDetail}>
              {images.length > 1 ? `${(index ?? 0) + 1} of ${images.length} · ` : ""}
              {shown.detail}
            </Text>
          </View>
        ) : null}
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create({
  tray: { paddingHorizontal: 6, paddingTop: 8, gap: 6 },
  trayList: { gap: 8, paddingTop: 6, paddingRight: 6 },
  trayTile: { width: 64, height: 64 },
  trayImage: { width: 64, height: 64, borderRadius: 14, borderCurve: "continuous" },
  remove: {
    position: "absolute",
    top: -6,
    right: -6,
    width: 44,
    height: 44,
    alignItems: "flex-end",
  },
  removeBadge: {
    width: 22,
    height: 22,
    borderRadius: 11,
    borderWidth: 2,
    alignItems: "center",
    justifyContent: "center",
  },
  problem: { ...text.xs, fontFamily: font.regular, paddingHorizontal: 4 },
  questionImages: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", alignSelf: "flex-end", gap: 6, marginBottom: 6, maxWidth: 3 * TILE + 2 * 6 },
  questionImage: { borderRadius: 16, borderCurve: "continuous" },
  missing: { alignItems: "center", justifyContent: "center" },
  viewer: { flex: 1, backgroundColor: "#000000" },
  stage: { flexGrow: 1, alignItems: "center", justifyContent: "center" },
  viewerWait: { ...text.sm, fontFamily: font.regular, color: "#b4b4b4" },
  viewerTop: { position: "absolute", top: 0, left: 0, right: 0, flexDirection: "row", justifyContent: Platform.OS === "ios" ? "flex-end" : "flex-start", paddingHorizontal: 16 },
  viewerClose: { width: 40, height: 40, borderRadius: 20, alignItems: "center", justifyContent: "center", backgroundColor: "rgba(255,255,255,0.16)" },
  viewerBar: { position: "absolute", left: 0, right: 0, bottom: 0, paddingHorizontal: 20, paddingTop: 12, gap: 2, alignItems: "center" },
  viewerName: { ...text.sm, fontFamily: font.medium, color: "#ffffff" },
  viewerDetail: { ...text.xs, fontFamily: font.regular, color: "#b4b4b4" },
});
