import { Button, ContextMenu, Host, RNHostView } from "@expo/ui/swift-ui";
import { useState, type ComponentProps, type ReactElement } from "react";
import { StyleSheet, View, type StyleProp, type ViewStyle } from "react-native";

type SFSymbol = NonNullable<ComponentProps<typeof Button>["systemImage"]>;

/** One action of a message's menu: its label, its symbol on each platform, and what it does. */
export type MessageAction = { key: string; label: string; icon: { ios: SFSymbol; android: string }; onPress: () => void; destructive?: boolean };

/**
 * A message's long-press menu (brand v5's P47 and P52): iOS's own context menu, which lifts the message over the
 * blurred chat and lists the actions under it, in Liquid Glass on iOS 26. Android draws a sheet
 * (`message-menu.android.tsx`). With no actions, the message alone.
 *
 * SwiftUI lays a hosted view out unbounded, so a long message would run off the screen as one line. The message lays
 * out here first, unseen, in React Native's own layout (`style` bounds it), and the menu's host is laid over it at
 * that size.
 */
export function MessageMenu({ actions, style, children }: { actions: MessageAction[]; style?: StyleProp<ViewStyle>; children: ReactElement }) {
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  if (!actions.length) return children;
  return (
    <View style={style}>
      <View
        style={styles.measure}
        onLayout={(e) => setSize({ width: e.nativeEvent.layout.width, height: e.nativeEvent.layout.height })}
        pointerEvents="none"
        accessibilityElementsHidden
        importantForAccessibility="no-hide-descendants"
      >
        {children}
      </View>
      {size ? (
        <Host style={[StyleSheet.absoluteFill, size]}>
          <ContextMenu>
            <ContextMenu.Items>
              {actions.map((a) => (
                <Button key={a.key} label={a.label} systemImage={a.icon.ios} role={a.destructive ? "destructive" : "default"} onPress={a.onPress} />
              ))}
            </ContextMenu.Items>
            <ContextMenu.Trigger>
              <RNHostView>
                <View style={size}>{children}</View>
              </RNHostView>
            </ContextMenu.Trigger>
          </ContextMenu>
        </Host>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({ measure: { opacity: 0 } });
