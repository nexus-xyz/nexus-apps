import { NavigationContext } from "expo-router/react-navigation";
import { useContext, useEffect, useRef, useState } from "react";

/**
 * Closes an open sheet when its screen loses focus. A sheet is a modal, above every screen, so one left open would stay
 * over whatever screen opens next (a notification's link, say). Outside a navigator (no screen) it does nothing.
 */
export function useCloseOnBlur(visible: boolean, onClose: () => void) {
  const navigation = useContext(NavigationContext);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    if (!navigation || !visible) return;
    return navigation.addListener("blur", () => close.current());
  }, [navigation, visible]);
}

/**
 * Whether the screen this is on is the one showing; true outside a navigator. A page follows its live updates only
 * while it shows: every stream left open under other screens holds a connection, and iOS allows a few per host.
 */
export function useScreenFocused() {
  const navigation = useContext(NavigationContext);
  const [focused, setFocused] = useState(() => navigation?.isFocused() ?? true);
  useEffect(() => {
    if (!navigation) return;
    const shown = navigation.addListener("focus", () => setFocused(true));
    const hidden = navigation.addListener("blur", () => setFocused(false));
    return () => {
      shown();
      hidden();
    };
  }, [navigation]);
  return focused;
}
