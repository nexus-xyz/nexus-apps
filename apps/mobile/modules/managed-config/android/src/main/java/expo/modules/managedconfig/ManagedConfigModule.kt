package expo.modules.managedconfig

import android.content.Context
import android.content.RestrictionsManager
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

/**
 * Managed configurations: what the organization's device management (EMM) sets for the app, through
 * `RestrictionsManager`. The key is iOS's AppConfig key, declared in `res/xml/nexus_app_restrictions.xml`.
 * `read` returns it when it holds a string; the app's JavaScript checks them (`src/managed-config.ts`).
 */
class ManagedConfigModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("NexusManagedConfig")

    /** The pinned key, when it holds a string; empty when no EMM configures the app. */
    Function("read") {
      val context = appContext.reactContext ?: return@Function emptyMap<String, String>()
      val manager = context.getSystemService(Context.RESTRICTIONS_SERVICE) as? RestrictionsManager
      val restrictions = manager?.applicationRestrictions ?: return@Function emptyMap<String, String>()
      KEYS.mapNotNull { key -> restrictions.getString(key)?.let { key to it } }.toMap()
    }
  }

  companion object {
    private val KEYS = listOf("org")
  }
}
