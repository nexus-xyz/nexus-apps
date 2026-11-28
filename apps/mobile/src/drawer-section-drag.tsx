import { useCallback, useEffect, useRef, useState, type ComponentRef, type RefObject } from "react";
import { AppState, StyleSheet, Text, View, type ScrollView } from "react-native";
import { Gesture, GestureDetector } from "react-native-gesture-handler";
import Animated, { useAnimatedStyle, useSharedValue } from "react-native-reanimated";
import { dropSection, type DropRect, type Point } from "./drawer-section-drop";
import { haptic } from "./haptics";
import { Icon } from "./icon";
import { font, text, useTheme } from "./theme";

type Source = { key: string; title: string; current?: string };
type Drag = {
  source: Source;
  scope: object;
  root?: DropRect;
  visible?: DropRect;
  hover?: string;
};

export function useDrawerSectionDrag({
  scope, enabled, targetNames, scroll, head, foot, onLock, onDrop,
}: {
  scope: object;
  enabled: boolean;
  targetNames: ReadonlySet<string>;
  scroll: RefObject<ComponentRef<typeof ScrollView> | null>;
  head: number;
  foot: number;
  onLock: (locked: boolean) => void;
  onDrop: (key: string, name: string) => void;
}) {
  const root = useRef<ComponentRef<typeof View>>(null);
  const sections = useRef(new Map<string, DropRect>());
  const active = useRef<Drag | undefined>(undefined);
  const held = useRef<Source | undefined>(undefined);
  const position = useSharedValue(0);
  const [shown, setShown] = useState<Drag>();
  const latest = useRef({ scope, enabled, targetNames, onDrop });
  latest.current = { scope, enabled, targetNames, onDrop };

  const cancel = useCallback(() => {
    active.current = undefined;
    held.current = undefined;
    setShown(undefined);
    onLock(false);
  }, [onLock]);
  useEffect(() => {
    if (!enabled) cancel();
    const background = AppState.addEventListener("change", (state) => { if (state !== "active") cancel(); });
    return () => { background.remove(); cancel(); };
  }, [cancel, enabled, foot, head, scope]);

  const lock = (source: Source) => {
    if (held.current || !latest.current.enabled) return;
    held.current = source;
    onLock(true);
  };
  const release = (key: string) => { if (held.current?.key === key) cancel(); };
  const start = (source: Source, point: Point) => {
    if (held.current?.key !== source.key || !latest.current.enabled) return;
    const drag: Drag = { source, scope: latest.current.scope };
    active.current = drag;
    haptic("longPress");
    root.current?.measureInWindow((x, y, width, height) => {
      if (active.current !== drag || !width || !height) return;
      drag.root = { x, y, width, height };
      position.value = point.y - y;
      setShown({ ...drag });
    });
    scroll.current?.measureInWindow((x, y, width, height) => {
      if (active.current === drag) drag.visible = { x, y: y + head, width, height: Math.max(0, height - head - foot) };
    });
  };
  const target = (drag: Drag, point: Point) => {
    if (!drag.root || !drag.visible || drag.scope !== latest.current.scope || !latest.current.enabled) return undefined;
    return dropSection(point, drag.root, drag.visible, sections.current, drag.source.current, latest.current.targetNames);
  };
  const shift = (key: string, point: Point) => {
    const drag = active.current;
    if (!drag?.root || drag.source.key !== key) return;
    position.value = point.y - drag.root.y;
    const name = target(drag, point);
    if (name === drag.hover) return;
    drag.hover = name;
    setShown({ ...drag });
    if (name) haptic("selection");
  };
  const end = (key: string, point: Point) => {
    const drag = active.current;
    if (!drag || drag.source.key !== key) { release(key); return; }
    const name = target(drag, point);
    cancel();
    if (name) latest.current.onDrop(key, name);
  };

  const style = useAnimatedStyle(() => ({ transform: [{ translateY: position.value }] }));
  const visible = shown?.scope === scope && enabled ? shown : undefined;
  return {
    root,
    dragging: visible?.source,
    hover: visible?.hover,
    style,
    layout: (name: string, bounds: DropRect) => { sections.current.set(name, bounds); },
    lock, start, shift, end, cancel, release,
  };
}

type DragControls = Pick<ReturnType<typeof useDrawerSectionDrag>, "lock" | "start" | "shift" | "end" | "release">;

export function DrawerSectionGrip({ title, source, controls, onPicker }: {
  title: string;
  source: Source | undefined;
  controls: DragControls;
  onPicker: () => void;
}) {
  const theme = useTheme();
  const tap = Gesture.Tap().runOnJS(true).onEnd((_event, success) => {
    if (success) onPicker();
  });
  const pan = Gesture.Pan().runOnJS(true)
    .enabled(!!source)
    .maxPointers(1)
    .activateAfterLongPress(180)
    .minDistance(4)
    .shouldCancelWhenOutside(false)
    .onBegin(() => { if (source) controls.lock(source); })
    .onStart(({ absoluteX: x, absoluteY: y }) => {
      if (source) controls.start(source, { x, y });
    })
    .onUpdate(({ absoluteX: x, absoluteY: y }) => { if (source) controls.shift(source.key, { x, y }); })
    .onEnd(({ absoluteX: x, absoluteY: y }, success) => {
      if (success && source) controls.end(source.key, { x, y });
    })
    .onFinalize(() => { if (source) controls.release(source.key); });

  return <GestureDetector gesture={Gesture.Exclusive(pan, tap)}>
    <View style={styles.grip} accessible accessibilityRole="button" accessibilityLabel={`Move ${title} to section`}
      accessibilityHint={source ? "Opens the section picker. Touch and hold, then drag to a visible section." : "Opens the section picker."}
      accessibilityActions={[{ name: "activate" }]}
      onAccessibilityAction={({ nativeEvent }) => { if (nativeEvent.actionName === "activate") onPicker(); }}>
      <Icon name="reorder" color={theme.muted} size={18} />
    </View>
  </GestureDetector>;
}

export function DrawerSectionGhost({ title, motion }: {
  title: string;
  motion: ReturnType<typeof useDrawerSectionDrag>["style"];
}) {
  const theme = useTheme();
  return <Animated.View pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants"
    style={[styles.ghost, motion, { backgroundColor: theme.card, borderColor: theme.separator }]}>
    <Icon name="reorder" color={theme.muted} size={18} />
    <Text style={[styles.ghostText, { color: theme.foreground }]} numberOfLines={1}>{title}</Text>
  </Animated.View>;
}

const styles = StyleSheet.create({
  grip: { width: 44, minHeight: 44, alignItems: "center", justifyContent: "center", borderRadius: 12 },
  ghost: {
    position: "absolute", top: -22, left: 8, right: 8, minHeight: 44, zIndex: 4, elevation: 6,
    flexDirection: "row", alignItems: "center", gap: 10, paddingHorizontal: 12, borderWidth: StyleSheet.hairlineWidth, borderRadius: 12,
  },
  ghostText: { ...text.body, fontFamily: font.medium, flex: 1 },
});
