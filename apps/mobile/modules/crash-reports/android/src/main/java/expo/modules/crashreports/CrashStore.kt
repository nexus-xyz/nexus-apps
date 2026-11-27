package expo.modules.crashreports

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

/**
 * The crashes the app has not sent yet, as `NativeCrash` (`eng/factory/kit/src/diagnostics.ts`). Two sources:
 *
 * - Java crashes: an uncaught-exception handler writes the exception's class and message to a file, then lets the
 *   previous handler end the app. A fatal JavaScript error is skipped: React Native throws it as `JavascriptException`,
 *   whose message carries the JavaScript message and stack, and `src/reporting.tsx` already reports it with no content.
 * - Native crashes and ANRs: Android's own record of how the app's last processes ended (`ApplicationExitInfo`,
 *   Android 11 and later), read at `take`. Android does not say which build ran then, so the first `take` on a device
 *   only marks where to start, and reports none from before this module.
 *
 * Frames are not kept. The API takes only `<binary> +0x<offset>` frames, so Java frames would be dropped, and a native
 * crash's frames are in a tombstone, which needs a protobuf parser.
 */
internal object CrashStore {
  /** The newest crashes kept: the API's batch (`MAX_PENDING` in `eng/factory/kit/src/diagnostics.ts`). */
  private const val LIMIT = 20
  private const val FILE = "nexus-crash-reports.json"
  private const val PREFS = "nexus-crash-reports"
  /** The time of the newest process exit already read, in milliseconds. */
  private const val READ_UNTIL = "exits-read-until"
  private const val JAVASCRIPT_ERROR = "com.facebook.react.common.JavascriptException"

  private val SIGNALS = mapOf(4 to "SIGILL", 5 to "SIGTRAP", 6 to "SIGABRT", 7 to "SIGBUS", 8 to "SIGFPE", 9 to "SIGKILL", 11 to "SIGSEGV")

  fun install(context: Context) {
    val app = context.applicationContext
    val previous = Thread.getDefaultUncaughtExceptionHandler()
    Thread.setDefaultUncaughtExceptionHandler { thread, error ->
      try {
        if (!fromJavaScript(error)) keep(app, crash(app, error.javaClass.name, error.message.orEmpty(), System.currentTimeMillis()))
      } catch (_: Throwable) {
        // Nothing is kept; the app ends as it would without the store.
      }
      previous?.uncaughtException(thread, error)
    }
  }

  @Synchronized
  fun take(context: Context): List<Map<String, Any>> {
    val kept = read(context) + exits(context)
    file(context).delete()
    return kept.takeLast(LIMIT)
  }

  @Synchronized
  private fun keep(context: Context, crash: Map<String, Any>) = write(context, (read(context) + crash).takeLast(LIMIT))

  private fun fromJavaScript(error: Throwable): Boolean =
    generateSequence(error) { it.cause }.take(8).any { it.javaClass.name == JAVASCRIPT_ERROR }

  /** The native crashes and ANRs since the last read. */
  private fun exits(context: Context): List<Map<String, Any>> {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return emptyList()
    val manager = context.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return emptyList()
    val exits = try {
      manager.getHistoricalProcessExitReasons(context.packageName, 0, 0)
    } catch (_: Exception) {
      return emptyList()
    }
    val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    val readUntil = prefs.getLong(READ_UNTIL, -1)
    val newest = maxOf(readUntil, exits.maxOfOrNull { it.timestamp } ?: System.currentTimeMillis())
    prefs.edit().putLong(READ_UNTIL, newest).apply()
    if (readUntil < 0) return emptyList()
    return exits
      .filter { it.timestamp > readUntil }
      .sortedBy { it.timestamp }
      .mapNotNull { exit ->
        when (exit.reason) {
          ApplicationExitInfo.REASON_CRASH_NATIVE ->
            crash(context, SIGNALS[exit.status] ?: "Native crash", exit.description.orEmpty(), exit.timestamp)
          ApplicationExitInfo.REASON_ANR -> crash(context, "ANR", exit.description.orEmpty(), exit.timestamp)
          else -> null
        }
      }
  }

  private fun crash(context: Context, name: String, message: String, at: Long): Map<String, Any> =
    mapOf(
      "name" to name,
      "message" to message,
      "stack" to emptyList<String>(),
      "occurredAt" to iso(at),
      "appBuild" to build(context),
      "osVersion" to Build.VERSION.RELEASE,
    )

  private fun build(context: Context): String =
    try {
      val info = context.packageManager.getPackageInfo(context.packageName, 0)
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) info.longVersionCode.toString() else @Suppress("DEPRECATION") info.versionCode.toString()
    } catch (_: PackageManager.NameNotFoundException) {
      ""
    }

  private fun iso(millis: Long): String =
    SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'", Locale.US).apply { timeZone = TimeZone.getTimeZone("UTC") }.format(Date(millis))

  private fun file(context: Context) = File(context.filesDir, FILE)

  private fun read(context: Context): List<Map<String, Any>> =
    try {
      val array = JSONArray(file(context).readText())
      (0 until array.length()).map { index ->
        val crash = array.getJSONObject(index)
        val stack = crash.optJSONArray("stack") ?: JSONArray()
        mapOf(
          "name" to crash.optString("name"),
          "message" to crash.optString("message"),
          "stack" to (0 until stack.length()).map { stack.optString(it) },
          "occurredAt" to crash.optString("occurredAt"),
          "appBuild" to crash.optString("appBuild"),
          "osVersion" to crash.optString("osVersion"),
        )
      }
    } catch (_: Exception) {
      emptyList()
    }

  /** Written whole to a temporary file and renamed, so a crash while writing cannot leave half a list. */
  private fun write(context: Context, crashes: List<Map<String, Any>>) {
    val array = JSONArray(crashes.map { JSONObject(it) })
    val temporary = File(context.filesDir, "$FILE.tmp")
    temporary.writeText(array.toString())
    temporary.renameTo(file(context))
  }
}
