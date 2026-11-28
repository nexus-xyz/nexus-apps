package expo.modules.crashreports

import android.os.Process
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

/**
 * Crashes on Android, as iOS's MetricKit module gives them (`ios/CrashReportsModule.swift`). `CrashStore`
 * keeps each one until the app's JavaScript takes it (`take`), turns it into a report (`@nexus/kit`'s
 * `diagnostics.fromCrash`) and sends it to the Gateway. Nothing here leaves the device by itself.
 */
class CrashReportsModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("NexusCrashReports")

    /** The crashes kept since the last call, oldest first, as `NativeCrash`; the store is empty after it. */
    AsyncFunction("take") {
      val context = appContext.reactContext ?: return@AsyncFunction emptyList<Map<String, Any>>()
      CrashStore.take(context)
    }

    /**
     * Stops the app at once with SIGABRT, for the forced test crash (`nexus://diagnostics/test-crash?kind=native`,
     * after the person agrees). Android records it as a native crash, which `take` reads at the next start.
     */
    Function("crash") {
      Process.sendSignal(Process.myPid(), SIGABRT)
    }
  }

  private companion object {
    const val SIGABRT = 6
  }
}
