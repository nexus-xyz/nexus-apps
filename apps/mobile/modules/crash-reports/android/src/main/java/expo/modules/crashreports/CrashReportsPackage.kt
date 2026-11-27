package expo.modules.crashreports

import android.app.Application
import android.content.Context
import expo.modules.core.interfaces.ApplicationLifecycleListener
import expo.modules.core.interfaces.Package

/** Keeps Java crashes from the application's start, before any activity or JavaScript runs (`CrashStore.install`). */
class CrashReportsPackage : Package {
  override fun createApplicationLifecycleListeners(context: Context): List<ApplicationLifecycleListener> =
    listOf(
      object : ApplicationLifecycleListener {
        override fun onCreate(application: Application) {
          CrashStore.install(application)
        }
      },
    )
}
