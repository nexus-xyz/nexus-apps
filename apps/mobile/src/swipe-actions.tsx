import { useRef, type ComponentProps, type ReactNode } from "react";
import { Platform, Pressable, StyleSheet, Text, View } from "react-native";
import ReanimatedSwipeable, { type SwipeableMethods } from "react-native-gesture-handler/ReanimatedSwipeable";
import { Icon } from "@/icon";
import { font, text } from "@/theme";

/** One action under a row: what it says, its symbol, its fill, the ink on it, and what it does. */
export type SwipeAction = { key: string; label: string; icon: ComponentProps<typeof Icon>["name"]; color: string; ink?: string; onPress: () => void };

/**
 * A row's swipe to the left (brand v5's P43 on the phone): the actions under it, side by side, each 74 pt wide. An
 * action closes the row before it runs, so what it opens or changes does not jump under the finger. The actions are
 * hidden from VoiceOver: the row offers the same ones as its own accessibility actions. Android's lists have no swipe,
 * so there it is the row alone.
 */
export function SwipeActions({ actions, children }: { actions: SwipeAction[]; children: ReactNode }) {
  const swipe = useRef<SwipeableMethods>(null);
  if (Platform.OS === "android" || !actions.length) return children;
  return (
    <ReanimatedSwipeable
      ref={swipe}
      friction={1.5}
      rightThreshold={40}
      overshootRight={false}
      renderRightActions={() => (
        <View style={styles.actions} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
          {actions.map((a) => (
            <Pressable
              key={a.key}
              style={({ pressed }) => [styles.action, { backgroundColor: a.color }, pressed && styles.pressed]}
              onPress={() => {
                swipe.current?.close();
                a.onPress();
              }}
            >
              <Icon name={a.icon} size={18} color={a.ink ?? "#ffffff"} />
              <Text style={[styles.label, { color: a.ink ?? "#ffffff" }]}>{a.label}</Text>
            </Pressable>
          ))}
        </View>
      )}
    >
      {children}
    </ReanimatedSwipeable>
  );
}

const styles = StyleSheet.create({
  actions: { flexDirection: "row" },
  action: { width: 74, alignItems: "center", justifyContent: "center", gap: 4 },
  pressed: { opacity: 0.85 },
  label: { ...text.caption, fontFamily: font.medium },
});
