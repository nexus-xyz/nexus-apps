import ExpoModulesCore
import MetricKit

/// Native crashes. The OS writes a crash down when it happens and MetricKit delivers it at the app's next
/// start. `CrashStore` keeps each one in a file until the app's JavaScript takes it (`take`), turns it into a report
/// (`@nexus/kit`'s `diagnostics.fromCrash`) and sends it to the Gateway. Nothing here leaves the device by itself.
public class CrashReportsModule: Module {
  public func definition() -> ModuleDefinition {
    Name("NexusCrashReports")

    /// The crashes kept since the last call, oldest first, as `NativeCrash`; the store is empty after it.
    AsyncFunction("take") { () -> [[String: Any]] in
      CrashStore.shared.take()
    }

    /// Stops the app at once, for the forced test crash (`nexus://diagnostics/test-crash?kind=native`, after the person
    /// agrees). MetricKit reports it at the next start like any other crash.
    Function("crash") { () -> Void in
      fatalError("Forced test crash")
    }
  }
}

/// Subscribes to MetricKit as the app finishes launching, so the crash it delivers at this start is not missed.
public class CrashReportsAppDelegateSubscriber: ExpoAppDelegateSubscriber {
  public func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    CrashStore.shared.subscribe()
    return true
  }
}

final class CrashStore: NSObject, MXMetricManagerSubscriber {
  static let shared = CrashStore()

  /// The newest crashes kept: the API's batch (`MAX_PENDING` in `eng/factory/kit/src/diagnostics.ts`).
  private static let limit = 20
  /// The frames kept from the top of the crashed thread: the API's bound.
  private static let frames = 64

  private let queue = DispatchQueue(label: "xyz.nexus.crash-reports")
  private let file = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    .appendingPathComponent("nexus-crash-reports.json")

  func subscribe() {
    MXMetricManager.shared.add(self)
  }

  func didReceive(_ payloads: [MXDiagnosticPayload]) {
    let crashes = payloads.flatMap { payload in
      (payload.crashDiagnostics ?? []).map { Self.crash($0, at: payload.timeStampEnd) }
    }
    guard !crashes.isEmpty else { return }
    queue.sync { write(Array((read() + crashes).suffix(Self.limit))) }
  }

  func take() -> [[String: Any]] {
    queue.sync {
      let crashes = read()
      try? FileManager.default.removeItem(at: file)
      return crashes
    }
  }

  private func read() -> [[String: Any]] {
    guard let data = try? Data(contentsOf: file),
      let crashes = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
    else { return [] }
    return crashes
  }

  private func write(_ crashes: [[String: Any]]) {
    guard let data = try? JSONSerialization.data(withJSONObject: crashes) else { return }
    try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
    try? data.write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
  }

  /// One crash as `NativeCrash`: its name, its reason, the crashed thread's frames, and the build and OS it ran on.
  /// An Objective-C exception (iOS 17 and later) names itself; otherwise the Mach exception and the signal do.
  private static func crash(_ diagnostic: MXCrashDiagnostic, at time: Date) -> [String: Any] {
    var name = [exception(diagnostic.exceptionType), signal(diagnostic.signal).map { "(\($0))" }]
      .compactMap { $0 }
      .joined(separator: " ")
    var message = diagnostic.terminationReason ?? ""
    if #available(iOS 17.0, *), let reason = diagnostic.exceptionReason {
      name = reason.exceptionName
      message = reason.composedMessage
    }
    return [
      "name": name,
      "message": message,
      "stack": stack(diagnostic.callStackTree),
      "occurredAt": ISO8601DateFormatter().string(from: time),
      "appBuild": diagnostic.metaData.applicationBuildVersion,
      "osVersion": diagnostic.metaData.osVersion,
    ]
  }

  /// The crashed thread's frames from the top, as `<binary> +0x<offset>`: what symbolication needs with the build's
  /// dSYM. MetricKit nests each caller in its frame's `subFrames`.
  private static func stack(_ tree: MXCallStackTree) -> [String] {
    guard let json = try? JSONSerialization.jsonObject(with: tree.jsonRepresentation()) as? [String: Any],
      let stacks = json["callStacks"] as? [[String: Any]],
      let crashed = stacks.first(where: { $0["threadAttributed"] as? Bool == true }) ?? stacks.first
    else { return [] }
    var out: [String] = []
    var frame = (crashed["callStackRootFrames"] as? [[String: Any]])?.first
    while let current = frame, out.count < frames {
      let binary = current["binaryName"] as? String ?? "?"
      let offset = (current["offsetIntoBinaryTextSegment"] as? NSNumber)?.uint64Value ?? 0
      out.append("\(binary) +0x\(String(offset, radix: 16))")
      frame = (current["subFrames"] as? [[String: Any]])?.first
    }
    return out
  }

  private static func exception(_ type: NSNumber?) -> String? {
    guard let type else { return nil }
    let names: [Int: String] = [
      1: "EXC_BAD_ACCESS", 2: "EXC_BAD_INSTRUCTION", 3: "EXC_ARITHMETIC", 6: "EXC_BREAKPOINT", 10: "EXC_CRASH",
      11: "EXC_RESOURCE", 12: "EXC_GUARD",
    ]
    return names[type.intValue] ?? "EXC_\(type.intValue)"
  }

  private static func signal(_ number: NSNumber?) -> String? {
    guard let number else { return nil }
    let names: [Int32: String] = [
      4: "SIGILL", 5: "SIGTRAP", 6: "SIGABRT", 8: "SIGFPE", 9: "SIGKILL", 10: "SIGBUS", 11: "SIGSEGV",
    ]
    return names[number.int32Value] ?? "SIG\(number.int32Value)"
  }
}
