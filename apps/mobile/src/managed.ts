import { requireOptionalNativeModule } from "expo";
import { useEffect, useState } from "react";
import { AppState } from "react-native";
import { readManaged, type ManagedConfig } from "./managed-config";

// The app's own module (`modules/managed-config`), linked by a development build. Expo Go lacks it: nothing is managed.
const native = requireOptionalNativeModule<{ read(): Record<string, string> }>("NexusManagedConfig");

const read = (): ManagedConfig => {
  try {
    return readManaged(native?.read());
  } catch {
    return {};
  }
};

/**
 * What device management pins (`managed-config.ts`). Read at start, and again each time the app comes to the
 * foreground, because an MDM may push a new configuration while the app runs.
 */
export function useManagedConfig(): ManagedConfig {
  const [config, setConfig] = useState(read);
  useEffect(() => {
    const subscription = AppState.addEventListener("change", (state) => {
      if (state !== "active") return;
      const next = read();
      setConfig((current) => (current.org === next.org ? current : next));
    });
    return () => subscription.remove();
  }, []);
  return config;
}
