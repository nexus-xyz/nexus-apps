import ExpoModulesCore
import StoreKit

/// The App Store storefront's country. Apple's rules on links to a purchase outside the app follow the
/// storefront, not the device's region or location, so Billing reads this (`src/storefront.ts`).
public class StoreCountryModule: Module {
  public func definition() -> ModuleDefinition {
    Name("NexusStoreCountry")

    /// ISO 3166-1 alpha-3, such as "USA"; `nil` when StoreKit does not know (no App Store account on the device).
    AsyncFunction("read") { () async -> String? in
      await Storefront.current?.countryCode
    }
  }
}
