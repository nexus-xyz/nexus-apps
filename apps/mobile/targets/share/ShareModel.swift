import SwiftUI
import UIKit

/// The extension's state: reading the share, the preview, then Save's outcome. Save keeps the import on the
/// phone and ends there: no network, no model call. The person opens Nexus to choose where it goes and to send it.
@MainActor
final class ShareModel: ObservableObject {
  enum Phase: Equatable {
    case loading
    case ready
    case saving
    case done(SaveOutcome)
    /// The App Group is missing (an unsigned build): nothing can be kept.
    case unavailable
  }

  @Published private(set) var phase: Phase = .loading
  @Published private(set) var content = SharedContent()
  @Published private(set) var thumbnails: [UUID: UIImage] = [:]
  let signedIn: Bool

  private let inbox = ShareInbox()
  private let staging = FileManager.default.temporaryDirectory.appendingPathComponent("share-\(UUID().uuidString)", isDirectory: true)
  private let finish: () -> Void
  private let cancelRequest: () -> Void

  init(finish: @escaping () -> Void, cancel: @escaping () -> Void) {
    self.finish = finish
    cancelRequest = cancel
    signedIn = inbox?.signedInAccount() != nil
  }

  var canSave: Bool { phase == .ready && !content.isEmpty }

  func load(_ items: [NSExtensionItem], scale: CGFloat) async {
    guard inbox != nil else {
      phase = .unavailable
      return
    }
    try? FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
    content = await ShareItems.load(items, staging: staging)
    for image in content.images {
      thumbnails[image.id] = ShareItems.thumbnail(image.url, side: 76, scale: scale)
    }
    phase = .ready
  }

  func save() {
    guard canSave, let inbox else { return }
    phase = .saving
    let content = content
    Task.detached(priority: .userInitiated) {
      let outcome = inbox.save(content)
      await MainActor.run {
        self.phase = .done(outcome)
        if outcome == .saved {
          UINotificationFeedbackGenerator().notificationOccurred(.success)
        }
        UIAccessibility.post(notification: .announcement, argument: ShareWords.title(outcome))
      }
    }
  }

  func done() {
    cleanUp()
    finish()
  }

  func cancel() {
    cleanUp()
    cancelRequest()
  }

  private func cleanUp() {
    try? FileManager.default.removeItem(at: staging)
  }
}

/// The extension's words, kept in one place.
enum ShareWords {
  static func title(_ outcome: SaveOutcome) -> String {
    switch outcome {
    case .saved: "Saved for Nexus"
    case .alreadySaved: "Already saved"
    case .full: "Nexus is holding \(ShareInboxLimits.imports) shared items"
    case .failed: "Couldn't save"
    }
  }

  static func detail(_ outcome: SaveOutcome, signedIn: Bool) -> String {
    switch outcome {
    case .saved, .alreadySaved:
      signedIn
        ? "Open Nexus to choose where it goes. Nothing is sent until you tap Send. Nexus keeps it for \(ShareInboxLimits.days) days."
        : "Sign in to Nexus to use it. Nothing is sent until you tap Send. Nexus keeps it for \(ShareInboxLimits.days) days."
    case .full: "Open Nexus and use or discard one, then share this again."
    case .failed: "Nexus couldn't keep this on your iPhone. Try again."
    }
  }

  static func reason(_ reason: SkipReason) -> String {
    switch reason {
    case .unsupported: "Nexus takes text, links and photos only"
    case .tooLarge: "Larger than \(ShareInboxLimits.imageBytes / 1024 / 1024) MB"
    case .tooMany: "A message takes at most \(ShareInboxLimits.images) photos"
    case .unreadable: "Couldn't be read"
    }
  }
}
