import { requireOptionalNativeModule } from "expo";
import { useEffect, useState } from "react";
import { AppState, Platform } from "react-native";
import { webBillingAllowed } from "./storefront";

// The app's own module (`modules/store-country`), iOS only. Expo Go and Android lack it: no storefront, so no link.
const native = requireOptionalNativeModule<{ read(): Promise<string | null> }>("NexusStoreCountry");

async function read(): Promise<string | null> {
  try {
    return (await native?.read()) ?? null;
  } catch {
    return null;
  }
}

/**
 * Whether Billing may link to Nexus UI's Billing (`storefront.ts`). False until the storefront is read, and read again
 * each time the app comes to the front: a person can change their App Store country while the app runs.
 */
export function useWebBilling(): boolean {
  const [allowed, setAllowed] = useState(false);
  useEffect(() => {
    if (!native) return;
    let live = true;
    const check = () => void read().then((storefront) => live && setAllowed(webBillingAllowed(Platform.OS, storefront)));
    check();
    const subscription = AppState.addEventListener("change", (state) => state === "active" && check());
    return () => {
      live = false;
      subscription.remove();
    };
  }, []);
  return allowed;
}
