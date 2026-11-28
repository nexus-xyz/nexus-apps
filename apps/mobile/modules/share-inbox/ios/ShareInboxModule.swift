import ExpoModulesCore
import Foundation

/// The App Group inbox the Share extension writes (`targets/share/ShareInbox.swift`), for `src/share-inbox-native.ts`.
/// It lists, copies and removes imports, and tells the extension which account is signed in. It reads only folders and
/// files with the names the extension writes, so a name never leads out of the inbox.
public class ShareInboxModule: Module {
  static let appGroup = "group.xyz.nexus.ios"

  private static let id = try! NSRegularExpression(pattern: "^[A-Za-z0-9-]{1,64}$")
  private static let imageFile = try! NSRegularExpression(pattern: "^image-[0-9]{1,2}\\.[A-Za-z0-9]{1,8}$")

  private static func matches(_ pattern: NSRegularExpression, _ value: String) -> Bool {
    pattern.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) != nil
  }

  private static var group: URL? {
    FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)
  }

  private static var inbox: URL? { group?.appendingPathComponent("ShareInbox", isDirectory: true) }

  private static func folder(_ id: String) throws -> URL {
    guard matches(Self.id, id), let inbox else { throw Exception(name: "ShareInboxId", description: "Not an import.") }
    return inbox.appendingPathComponent(id, isDirectory: true)
  }

  public func definition() -> ModuleDefinition {
    Name("NexusShareInbox")

    /// Each import's folder name and `import.json`, unparsed: `src/share-inbox.ts` checks them.
    AsyncFunction("list") { () -> [[String: String]] in
      guard let inbox = Self.inbox else { return [] }
      let entries = (try? FileManager.default.contentsOfDirectory(at: inbox, includingPropertiesForKeys: nil)) ?? []
      return entries.compactMap { entry in
        let id = entry.lastPathComponent
        guard Self.matches(Self.id, id) else { return nil }
        let json = (try? String(contentsOf: entry.appendingPathComponent("import.json"), encoding: .utf8)) ?? ""
        return ["id": id, "json": json, "folder": entry.absoluteString]
      }
    }

    /// The import's photos copied into the app's caches, where the composer prepares and then deletes them.
    AsyncFunction("copyImages") { (id: String, files: [String]) throws -> [String] in
      let folder = try Self.folder(id)
      let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
      return try files.map { file in
        guard Self.matches(Self.imageFile, file) else { throw Exception(name: "ShareInboxFile", description: "Not a shared photo.") }
        let copy = caches.appendingPathComponent("shared-\(UUID().uuidString).\((file as NSString).pathExtension)")
        try FileManager.default.copyItem(at: folder.appendingPathComponent(file), to: copy)
        return copy.absoluteString
      }
    }

    AsyncFunction("remove") { (id: String) throws in
      let folder = try Self.folder(id)
      if FileManager.default.fileExists(atPath: folder.path) { try FileManager.default.removeItem(at: folder) }
    }

    /// Every import, on sign-out.
    AsyncFunction("clear") { () throws in
      guard let inbox = Self.inbox, FileManager.default.fileExists(atPath: inbox.path) else { return }
      try FileManager.default.removeItem(at: inbox)
    }

    /// The signed-in account's key (`accountKey`), which Save records with an import; nil when no one is signed in.
    AsyncFunction("setAccount") { (key: String?) throws in
      guard let file = Self.group?.appendingPathComponent("share-account.json") else { return }
      guard let key, !key.isEmpty else {
        if FileManager.default.fileExists(atPath: file.path) { try FileManager.default.removeItem(at: file) }
        return
      }
      let data = try JSONSerialization.data(withJSONObject: ["key": key])
      try data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
  }
}
