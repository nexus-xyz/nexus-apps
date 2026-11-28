import ExpoModulesCore

/// Managed app configuration. An MDM pushes an AppConfig dictionary, and iOS keeps it in `UserDefaults`
/// under `com.apple.configuration.managed`. `read` returns its `org` key; the app's JavaScript checks them
/// (`src/managed-config.ts`). For a test on the simulator:
/// `xcrun simctl spawn booted defaults write xyz.nexus.ios com.apple.configuration.managed -dict org <slug>`
public class ManagedConfigModule: Module {
  static let managedKey = "com.apple.configuration.managed"
  static let keys = ["org"]

  public func definition() -> ModuleDefinition {
    Name("NexusManagedConfig")

    /// The pinned key, when it holds a string; `{}` when no MDM configures the app.
    Function("read") { () -> [String: String] in
      let managed = UserDefaults.standard.dictionary(forKey: Self.managedKey) ?? [:]
      return Self.keys.reduce(into: [:]) { values, key in
        if let value = managed[key] as? String { values[key] = value }
      }
    }
  }
}
