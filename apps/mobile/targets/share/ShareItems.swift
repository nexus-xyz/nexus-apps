import ImageIO
import UIKit
import UniformTypeIdentifiers

/// Reads what another app shared: text, web links and photos. Everything else is listed as left out, with
/// its name, so the person sees what Nexus did not keep. Photos are copied into `staging` as they are (the app removes
/// their metadata when it prepares them, as it does for a photo picked in the app); nothing is decoded whole here.
enum ShareItems {
  static func load(_ items: [NSExtensionItem], staging: URL) async -> SharedContent {
    var content = SharedContent()
    var texts: [String] = []
    for item in items {
      for provider in item.attachments ?? [] {
        let types = provider.registeredTypeIdentifiers.compactMap { UTType($0) }
        let name = provider.suggestedName ?? ""
        if let imageType = types.first(where: { $0.conforms(to: .image) }) {
          if content.images.count >= ShareInboxLimits.images {
            content.skipped.append(SkippedItem(name: name.isEmpty ? "A photo" : name, reason: .tooMany))
            continue
          }
          switch await stageImage(provider, type: imageType, index: content.images.count + 1, staging: staging) {
          case .success(let image): content.images.append(image)
          case .failure(let reason): content.skipped.append(SkippedItem(name: name.isEmpty ? "A photo" : name, reason: reason))
          }
        } else if types.contains(where: { $0.conforms(to: .url) }) {
          guard let url = await loadURL(provider) else {
            content.skipped.append(SkippedItem(name: name.isEmpty ? "A link" : name, reason: .unreadable))
            continue
          }
          // A shared link passes its address only. A file is not a link, and other schemes are not pages.
          if url.isFileURL || !["http", "https"].contains(url.scheme?.lowercased() ?? "") {
            content.skipped.append(SkippedItem(name: name.isEmpty ? (url.isFileURL ? url.lastPathComponent : url.absoluteString) : name, reason: .unsupported))
          } else if content.urls.count >= ShareInboxLimits.urls {
            content.linksCut = true
          } else if !content.urls.contains(url.absoluteString) {
            content.urls.append(url.absoluteString)
          }
        } else if types.contains(where: { $0.conforms(to: .plainText) }) {
          if let text = await loadText(provider) { texts.append(text) } else { content.skipped.append(SkippedItem(name: name.isEmpty ? "Text" : name, reason: .unreadable)) }
        } else {
          let kind = types.first?.localizedDescription ?? "file"
          content.skipped.append(SkippedItem(name: name.isEmpty ? "A \(kind.lowercased())" : name, reason: .unsupported))
        }
      }
      // Some apps put the page's title or the message beside the item rather than in it.
      if let words = item.attributedContentText?.string.trimmingCharacters(in: .whitespacesAndNewlines), !words.isEmpty,
         !texts.contains(words), !content.urls.contains(words) {
        texts.insert(words, at: 0)
      }
    }
    let text = texts.joined(separator: "\n\n")
    content.textCut = text.count > ShareInboxLimits.textCharacters
    content.text = String(text.prefix(ShareInboxLimits.textCharacters))
    return content
  }

  private static func loadURL(_ provider: NSItemProvider) async -> URL? {
    await withCheckedContinuation { continuation in
      provider.loadItem(forTypeIdentifier: UTType.url.identifier, options: nil) { item, _ in
        switch item {
        case let url as URL: continuation.resume(returning: url)
        case let data as Data: continuation.resume(returning: URL(dataRepresentation: data, relativeTo: nil))
        case let text as String: continuation.resume(returning: URL(string: text))
        default: continuation.resume(returning: nil)
        }
      }
    }
  }

  private static func loadText(_ provider: NSItemProvider) async -> String? {
    await withCheckedContinuation { continuation in
      provider.loadItem(forTypeIdentifier: UTType.plainText.identifier, options: nil) { item, _ in
        switch item {
        case let text as String: continuation.resume(returning: text)
        case let text as NSAttributedString: continuation.resume(returning: text.string)
        case let data as Data: continuation.resume(returning: String(data: data, encoding: .utf8))
        default: continuation.resume(returning: nil)
        }
      }
    }
  }

  /// The photo copied to `staging/image-<index>.<ext>`, or why it was left out.
  private static func stageImage(_ provider: NSItemProvider, type: UTType, index: Int, staging: URL) async -> Result<StagedImage, SkipReason> {
    let base = staging.appendingPathComponent("image-\(index)")
    // A file first: it is copied, never read into memory. An app that offers only an image object (a screenshot's
    // markup) is drawn once as a JPEG.
    var copied: URL? = await withCheckedContinuation { continuation in
      provider.loadFileRepresentation(forTypeIdentifier: type.identifier) { url, _ in
        guard let url else { return continuation.resume(returning: nil) }
        let ext = url.pathExtension.isEmpty ? (type.preferredFilenameExtension ?? "jpg") : url.pathExtension
        let destination = base.appendingPathExtension(ext.lowercased())
        do {
          try FileManager.default.copyItem(at: url, to: destination)
          continuation.resume(returning: destination)
        } catch {
          continuation.resume(returning: nil)
        }
      }
    }
    if copied == nil {
      copied = await withCheckedContinuation { continuation in
        provider.loadItem(forTypeIdentifier: type.identifier, options: nil) { item, _ in
          let destination = base.appendingPathExtension("jpg")
          let data: Data? = switch item {
          case let image as UIImage: image.jpegData(compressionQuality: 0.92)
          case let data as Data: UIImage(data: data)?.jpegData(compressionQuality: 0.92)
          default: nil
          }
          guard let data, (try? data.write(to: destination)) != nil else { return continuation.resume(returning: nil) }
          continuation.resume(returning: destination)
        }
      }
    }
    guard let url = copied else { return .failure(.unreadable) }
    let bytes = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
    if bytes > ShareInboxLimits.imageBytes {
      try? FileManager.default.removeItem(at: url)
      return .failure(.tooLarge)
    }
    guard bytes > 0, CGImageSourceCreateWithURL(url as CFURL, nil).map({ CGImageSourceGetCount($0) > 0 }) == true else {
      try? FileManager.default.removeItem(at: url)
      return .failure(.unreadable)
    }
    let fileType = UTType(filenameExtension: url.pathExtension) ?? type
    let shownName = provider.suggestedName.map { name in
      (name as NSString).pathExtension.isEmpty ? "\(name).\(url.pathExtension)" : name
    } ?? "Photo \(index).\(url.pathExtension)"
    return .success(StagedImage(url: url, name: shownName, type: fileType.preferredMIMEType, bytes: bytes))
  }

  /// A small copy to show, decoded no larger than `side` points, so a 48 MP photo costs a thumbnail's memory.
  static func thumbnail(_ url: URL, side: CGFloat, scale: CGFloat) -> UIImage? {
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
    let options: [CFString: Any] = [
      kCGImageSourceCreateThumbnailFromImageAlways: true,
      kCGImageSourceCreateThumbnailWithTransform: true,
      kCGImageSourceThumbnailMaxPixelSize: side * scale,
    ]
    return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary).map { UIImage(cgImage: $0) }
  }
}
