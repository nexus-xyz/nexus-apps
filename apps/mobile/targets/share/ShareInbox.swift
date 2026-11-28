import CryptoKit
import Foundation

/// What the inbox holds at most. `src/share-inbox.ts` has the same numbers, and the app's module the same
/// App Group; `src/share-inbox.test.ts` checks that they agree.
enum ShareInboxLimits {
  static let appGroup = "group.xyz.nexus.ios"
  static let imports = 5
  static let images = 8
  static let imageBytes = 25 * 1024 * 1024
  static let textCharacters = 20_000
  static let urls = 10
  static let days = 7
}

/// A photo copied out of the share, waiting for Save.
struct StagedImage: Identifiable {
  let id = UUID()
  let url: URL
  let name: String
  let type: String?
  let bytes: Int
}

/// Why an item was left out. The words are the app's (`skippedLine` in `src/share-inbox.ts`).
enum SkipReason: String, Encodable, Error {
  case unsupported
  case tooLarge = "too-large"
  case tooMany = "too-many"
  case unreadable
}

struct SkippedItem: Encodable, Identifiable {
  var id: String { "\(name)-\(reason.rawValue)" }
  let name: String
  let reason: SkipReason
}

/// What the share held, as the extension shows and saves it.
struct SharedContent {
  var text = ""
  var urls: [String] = []
  var images: [StagedImage] = []
  var skipped: [SkippedItem] = []
  /// The text was longer than the inbox keeps, or there were more links.
  var textCut = false
  var linksCut = false

  var isEmpty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && urls.isEmpty && images.isEmpty }

  /// The same text, links and photos give the same value: a second Save of one share keeps one import.
  func fingerprint() -> String {
    var hash = SHA256()
    func add(_ data: Data) {
      hash.update(data: data)
      hash.update(data: Data([0]))
    }
    add(Data(text.utf8))
    for url in urls { add(Data(url.utf8)) }
    for image in images {
      if let handle = try? FileHandle(forReadingFrom: image.url) {
        while let chunk = try? handle.read(upToCount: 1 << 20), !chunk.isEmpty { hash.update(data: chunk) }
        try? handle.close()
      }
      add(Data())
    }
    return hash.finalize().map { String(format: "%02x", $0) }.joined()
  }
}

enum SaveOutcome: Equatable {
  case saved
  case alreadySaved
  case full
  case failed
}

/// The App Group's inbox: `ShareInbox/<id>/import.json` and the import's photos, protected while the phone
/// is locked. Save writes a folder under a `.partial` name and renames it, so the app never reads half an import. The
/// app's module (`modules/share-inbox`) reads, copies and removes them; the app removes them all on sign-out.
struct ShareInbox {
  let root: URL
  private let account: URL
  private let files = FileManager.default

  init?() {
    guard let group = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: ShareInboxLimits.appGroup) else { return nil }
    root = group.appendingPathComponent("ShareInbox", isDirectory: true)
    account = group.appendingPathComponent("share-account.json")
  }

  /// The account signed in to the app, as the app wrote it (`setAccount`); nil when no one is.
  func signedInAccount() -> String? {
    guard let data = try? Data(contentsOf: account),
          let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let key = object["key"] as? String, !key.isEmpty
    else { return nil }
    return key
  }

  func save(_ content: SharedContent) -> SaveOutcome {
    do {
      try files.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.complete])
    } catch {
      return .failed
    }
    let waiting = prune()
    let fingerprint = content.fingerprint()
    if waiting.contains(fingerprint) { return .alreadySaved }
    if waiting.count >= ShareInboxLimits.imports { return .full }

    let id = UUID().uuidString
    let partial = root.appendingPathComponent("\(id).partial", isDirectory: true)
    do {
      try files.createDirectory(at: partial, withIntermediateDirectories: false, attributes: [.protectionKey: FileProtectionType.complete])
      var images: [[String: Any]] = []
      for (index, image) in content.images.enumerated() {
        let ext = image.url.pathExtension.isEmpty ? "jpg" : image.url.pathExtension.lowercased()
        let file = "image-\(index + 1).\(ext)"
        let destination = partial.appendingPathComponent(file)
        try files.copyItem(at: image.url, to: destination)
        try files.setAttributes([.protectionKey: FileProtectionType.complete], ofItemAtPath: destination.path)
        var entry: [String: Any] = ["file": file, "name": image.name, "bytes": image.bytes]
        if let type = image.type { entry["type"] = type }
        images.append(entry)
      }
      let manifest: [String: Any] = [
        "version": 1,
        "id": id,
        "createdAt": ISO8601DateFormatter().string(from: Date()),
        "account": signedInAccount() ?? NSNull(),
        "fingerprint": fingerprint,
        "text": content.text,
        "urls": content.urls,
        "images": images,
        "skipped": content.skipped.map { ["name": $0.name, "reason": $0.reason.rawValue] },
      ]
      let data = try JSONSerialization.data(withJSONObject: manifest)
      try data.write(to: partial.appendingPathComponent("import.json"), options: [.atomic, .completeFileProtection])
      try files.moveItem(at: partial, to: root.appendingPathComponent(id, isDirectory: true))
      return .saved
    } catch {
      try? files.removeItem(at: partial)
      return .failed
    }
  }

  /// Removes what no one will read (half-written folders, unreadable or expired imports) and returns the fingerprints
  /// of the imports still waiting.
  private func prune() -> [String] {
    let entries = (try? files.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)) ?? []
    let oldest = Date().addingTimeInterval(-Double(ShareInboxLimits.days) * 24 * 60 * 60)
    var waiting: [String] = []
    for entry in entries {
      guard let manifest = try? Data(contentsOf: entry.appendingPathComponent("import.json")),
            let object = try? JSONSerialization.jsonObject(with: manifest) as? [String: Any],
            let created = (object["createdAt"] as? String).flatMap({ ISO8601DateFormatter().date(from: $0) }),
            created >= oldest,
            !entry.lastPathComponent.hasSuffix(".partial")
      else {
        try? files.removeItem(at: entry)
        continue
      }
      waiting.append(object["fingerprint"] as? String ?? "")
    }
    return waiting
  }
}
