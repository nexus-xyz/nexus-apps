import SwiftUI
import UIKit

/// The Share extension's entry (`Info.plist`'s principal class). It hosts `ShareView`, and ends the request
/// with Done or Cancel. It never opens the app: Apple does not support that from a Share extension, so the person opens
/// Nexus, which finds the saved import.
final class ShareViewController: UIViewController {
  private var model: ShareModel?

  override func viewDidLoad() {
    super.viewDidLoad()
    let model = ShareModel(
      finish: { [weak self] in self?.extensionContext?.completeRequest(returningItems: nil) },
      cancel: { [weak self] in
        self?.extensionContext?.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
      }
    )
    self.model = model
    let host = UIHostingController(rootView: ShareView(model: model))
    addChild(host)
    host.view.frame = view.bounds
    host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    view.addSubview(host.view)
    host.didMove(toParent: self)
    let items = extensionContext?.inputItems.compactMap { $0 as? NSExtensionItem } ?? []
    let scale = view.window?.screen.scale ?? traitCollection.displayScale
    Task { await model.load(items, scale: scale) }
  }
}
