import { useEffect, useLayoutEffect, useRef, useState, type ComponentProps, type ComponentRef, type ReactNode, type RefObject } from "react";
import { Platform, Pressable, ScrollView, StyleSheet, Text, useWindowDimensions, View } from "react-native";
import Reanimated, { Keyframe } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { Glass, useGlass } from "@/glass";
import { Icon } from "@/icon";
import { haptic } from "@/haptics";
import { DURATION, EASE_PRESS, SCALE } from "@/motion";
import { pressedFill, ripple } from "@/press";
import { font, text, useTheme } from "@/theme";
import { BAR_HEIGHT } from "@/top-bar";

/** `disabled`: shown dimmed, and a tap does nothing. */
export type DropdownOption<T extends string> = { value: T; label: string; detail?: string; leading?: ReactNode; disabled?: boolean };
export type DropdownSection<T extends string> = { title?: string; options: DropdownOption<T>[] };

// The menu grows out of its chip, from 0.95, and has no exit: picking an option snaps it shut. Glass may not fade (see glass.tsx),
// so the glass menu only scales, its rows fade inside it, and only the stand-in surface fades with its scale.
// Reduce Motion skips these: Reanimated's Keyframe follows the system setting.
const GROW = new Keyframe({
  0: { transform: [{ scale: SCALE.enter }] },
  100: { transform: [{ scale: 1 }], easing: EASE_PRESS },
}).duration(DURATION.fast);
const GROW_FADED = new Keyframe({
  0: { opacity: 0, transform: [{ scale: SCALE.enter }] },
  100: { opacity: 1, transform: [{ scale: 1 }], easing: EASE_PRESS },
}).duration(DURATION.fast);
const FADE = new Keyframe({ 0: { opacity: 0 }, 100: { opacity: 1, easing: EASE_PRESS } }).duration(DURATION.fast);

/** The menu's gap to its chip, to the bar above it and to the screen's sides. */
const GAP = 8;
const MARGIN = 12;
/** The tallest a menu gets, as a share of the screen: a long list (the models) scrolls inside it rather than filling it. */
const TALLEST = 0.55;

/**
 * A menu that opens from the control that owns it (above it, since the composer sits at the bottom; `placement:
 * "below"` for a control in the bar, as the Assistant's ⋯), listing every option with the current one checked, in
 * sections when there are several (the models, by provider). It stops below the chat's bar and at about half the
 * screen, and scrolls inside that, opened on the checked option.
 *
 * Not a Modal: opening one takes the focus from the composer, so the keyboard closed under the menu. It is drawn in
 * `container` instead (render it last there, so it sits on top), placed from the chip's position inside it. It is
 * Liquid Glass where iOS draws it, as iOS 26's own menus are, a card elsewhere on iOS, and Material 3's menu on Android.
 */
export function Dropdown<T extends string>(props: {
  anchor: RefObject<ComponentRef<typeof View> | null>;
  /**
   * The view the menu is drawn in: it starts at the screen's top, under the chat's bar, and ends at the keyboard's top,
   * so the chip's distance to its bottom holds.
   */
  container: RefObject<ComponentRef<typeof View> | null>;
  visible: boolean;
  sections: DropdownSection<T>[];
  /** The option checked; a menu of actions (Attach) checks none. */
  selected?: T;
  onPick: (value: T) => void;
  onClose: () => void;
  width?: number;
  /** Options kept behind a row that expands in place ("Other models"), each section as above. */
  more?: { label: string; sections: DropdownSection<T>[] };
  /** Where the menu opens from its control: above it (the default, for the composer's chips) or below it (the bar). */
  placement?: "above" | "below";
}) {
  const below = props.placement === "below";
  const theme = useTheme();
  const glass = useGlass();
  const insets = useSafeAreaInsets();
  const { height: screen } = useWindowDimensions();
  /** The chip's top-left, width and height in the container, and the container's size. */
  const [at, setAt] = useState<{ x: number; y: number; chip: number; chipHeight: number; width: number; height: number } | null>(null);
  const scroller = useRef<ComponentRef<typeof ScrollView>>(null);
  const list = useRef<ComponentRef<typeof View>>(null);
  const checked = useRef<ComponentRef<typeof View>>(null);
  /** Where "Other models" was, to scroll the models it opens into view once they are laid out. */
  const opened = useRef<number | null>(null);
  const moreAt = useRef(0);
  const viewport = useRef(0);
  // Open already when the current choice lives behind it, so the check mark is in view.
  const behind = !!props.more?.sections.some((s) => s.options.some((o) => o.value === props.selected));
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    if (props.visible) {
      setExpanded(behind);
      const anchor = props.anchor.current;
      const container = props.container.current;
      if (anchor && container) container.measure((_x, _y, cw, ch) => anchor.measureLayout(container, (x, y, w, h) => setAt({ x, y, chip: w, chipHeight: h, width: cw, height: ch })));
    } else setAt(null);
  }, [props.visible, props.anchor]);
  const sections = expanded && props.more ? [...props.sections, ...props.more.sections] : props.sections;
  // Above its chip, the menu may reach the bar's foot; below it, the container's foot (the keyboard's top). Never more
  // than a share of the screen.
  const room = !at ? 0 : below ? at.height - (at.y + at.chipHeight + GAP) - MARGIN : at.y - GAP - (insets.top + BAR_HEIGHT + GAP);
  const maxHeight = at ? Math.min(room, Math.round(screen * TALLEST)) : 0;
  // Opened on the checked option, scrolled just enough to show it, and the scroll bar flashed when the list is longer
  // than the menu, as iOS's own lists do.
  useLayoutEffect(() => {
    if (!at) return;
    const row = checked.current;
    const content = list.current;
    if (row && content) {
      row.measureLayout(content, (_x, y, _w, h) => {
        const offset = y + h + GAP - maxHeight;
        if (offset > 0) scroller.current?.scrollTo({ y: offset, animated: false });
      });
    }
    scroller.current?.flashScrollIndicators();
  }, [at]);

  if (!props.visible || !at) return null;
  const width = Math.min(props.width ?? 280, at.width - MARGIN * 2);
  const left = Math.max(MARGIN, Math.min(at.x, at.width - width - MARGIN));
  return (
    <>
      <Pressable accessibilityRole="button" style={[StyleSheet.absoluteFill, styles.backdrop]} onPress={props.onClose} accessibilityLabel="Close" />
      {at ? (
        <Reanimated.View
          entering={glass ? GROW : GROW_FADED}
          // Grows from the chip's middle, so it reads as coming out of it.
          style={[
            styles.menu,
            below
              ? { width, left, top: at.y + at.chipHeight + GAP, transformOrigin: [at.x + at.chip / 2 - left, 0, 0] }
              : { width, left, bottom: at.height - at.y + GAP, transformOrigin: [at.x + at.chip / 2 - left, "100%", 0] },
          ]}
        >
          <Surface
            glass={glass}
            style={[styles.shape, !glass && [styles.card, { backgroundColor: theme.card, borderColor: theme.border }], { maxHeight }]}
          >
            <Reanimated.View entering={FADE} style={styles.fill}>
              <ScrollView
                ref={scroller}
                alwaysBounceVertical={false}
                // A row answers the first tap with the keyboard up, rather than that tap only closing the keyboard.
                keyboardShouldPersistTaps="handled"
                contentContainerStyle={styles.content}
                onLayout={(e) => (viewport.current = e.nativeEvent.layout.height)}
                onContentSizeChange={(_w, h) => {
                  if (opened.current === null) return;
                  // The first models "Other models" opened, at the top, with a little of what was above them.
                  const y = Math.max(0, Math.min(opened.current - GAP * 3, h - viewport.current));
                  opened.current = null;
                  scroller.current?.scrollTo({ y, animated: true });
                }}
              >
                <View ref={list}>
                {sections.map((section, s) => (
                  // By position: "Other models" repeats the providers' titles.
                  <View key={s} style={s > 0 && [styles.divided, { borderColor: theme.border }]}>
                    {section.title ? <Text style={[styles.title, { color: theme.tertiary }]}>{section.title}</Text> : null}
                    {section.options.map((option) => (
                      <Pressable
                        key={option.value}
                        ref={option.value === props.selected ? checked : undefined}
                        android_ripple={option.disabled ? undefined : ripple(theme)}
                        disabled={option.disabled}
                        style={({ pressed }) => [
                          styles.option,
                          ANDROID && option.value === props.selected && { backgroundColor: theme.fill },
                          option.disabled && styles.disabled,
                          pressed && pressedFill(theme),
                        ]}
                        onPress={() => {
                          if (option.value !== props.selected) haptic("selection");
                          props.onPick(option.value);
                          props.onClose();
                        }}
                        accessibilityRole="menuitem"
                        accessibilityState={{ selected: option.value === props.selected, disabled: !!option.disabled }}
                      >
                        {option.leading}
                        <View style={styles.body}>
                          <Text style={[styles.label, { color: theme.foreground }]} numberOfLines={2}>
                            {option.label}
                          </Text>
                          {option.detail ? <Text style={[styles.detail, { color: theme.muted }]}>{option.detail}</Text> : null}
                        </View>
                        {option.value === props.selected ? <Icon name="check" size={16} color={theme.primary} /> : null}
                      </Pressable>
                    ))}
                  </View>
                ))}
                {props.more && !expanded ? (
                  <>
                  {ANDROID ? <View style={[styles.rule, { backgroundColor: theme.border }]} /> : null}
                  <Pressable
                    android_ripple={ripple(theme)}
                    // A rounded row can't carry the hairline above it on Android: the divider is its own line there.
                    style={({ pressed }) => [styles.option, !ANDROID && [styles.divided, { borderColor: theme.border }], pressed && pressedFill(theme)]}
                    onLayout={(e) => (moreAt.current = e.nativeEvent.layout.y)}
                    onPress={() => {
                      opened.current = moreAt.current;
                      setExpanded(true);
                    }}
                    accessibilityRole="button"
                    accessibilityState={{ expanded: false }}
                  >
                    <Text style={[styles.label, styles.body, { color: theme.muted }]}>{props.more.label}</Text>
                    <Icon name="chevron" size={14} color={theme.muted} />
                  </Pressable>
                  </>
                ) : null}
                </View>
              </ScrollView>
            </Reanimated.View>
          </Surface>
        </Reanimated.View>
      ) : null}
    </>
  );
}

/** The menu's surface: glass, or the card it is drawn on where there is none. */
function Surface({ glass, style, children }: { glass: boolean; style: ComponentProps<typeof View>["style"]; children: ReactNode }) {
  return glass ? (
    <Glass appear style={style} accessibilityRole="menu">
      {children}
    </Glass>
  ) : (
    <View style={style} accessibilityRole="menu">
      {children}
    </View>
  );
}

// Android: a 16 dp card with no edge, raised to level 2 (3 dp). Its rows sit 4 dp in, with 12 dp corners that clip
// their ripple, and the chosen row is filled, as Material 3 Expressive's menus are. Text stays 16 dp from the sides.
const ANDROID = Platform.OS === "android";
const styles = StyleSheet.create({
  backdrop: { zIndex: 10 },
  menu: { position: "absolute", zIndex: 11 },
  shape: { borderRadius: ANDROID ? 16 : 24, borderCurve: "continuous", overflow: "hidden" },
  card: ANDROID
    ? { elevation: 3 }
    : {
        borderWidth: StyleSheet.hairlineWidth,
        shadowColor: "#000",
        shadowOpacity: 0.15,
        shadowRadius: 28,
        shadowOffset: { width: 0, height: 6 },
      },
  // Lets the list shrink to the surface's maxHeight, so it scrolls instead of overflowing.
  fill: { flexShrink: 1 },
  content: ANDROID ? { padding: 4 } : { paddingVertical: 6 },
  divided: { borderTopWidth: StyleSheet.hairlineWidth, marginTop: 4, paddingTop: 4 },
  rule: { height: StyleSheet.hairlineWidth, marginVertical: 4, marginHorizontal: 12 },
  title: { ...text.xs, fontFamily: font.medium, paddingHorizontal: ANDROID ? 12 : 16, paddingTop: 8, paddingBottom: 2 },
  option: {
    flexDirection: "row",
    alignItems: "center",
    gap: 12,
    paddingHorizontal: ANDROID ? 12 : 16,
    paddingVertical: 10,
    minHeight: 48,
    ...(ANDROID && { borderRadius: 12, overflow: "hidden" as const }),
  },
  disabled: { opacity: 0.4 },
  body: { flex: 1 },
  label: { ...text.base, fontFamily: font.medium },
  detail: { ...text.xs, fontFamily: font.regular, marginTop: 2 },
});
