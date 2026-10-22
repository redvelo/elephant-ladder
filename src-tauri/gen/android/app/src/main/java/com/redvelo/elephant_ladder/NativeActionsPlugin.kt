package com.redvelo.elephant_ladder

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.os.Build
import android.util.Log
import android.webkit.WebStorage
import java.io.File
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONObject

private const val TAG = "NativeActionsPlugin"
private const val EINK_HELPER_CLASS = "android.onyx.optimization.EInkHelper"
private const val ACTION_SHOW_FRONT_LIGHT_DIALOG = "action.show.brightness.dialog"

object AnnotationLinkInbox {
  private const val MAX_LINKS = 16
  private const val MAX_LINK_LENGTH = 16 * 1024
  private val links = ArrayList<String>()

  @Synchronized
  fun receive(intent: Intent?) {
    val data = intent?.takeIf { it.action == Intent.ACTION_VIEW }?.dataString ?: return
    if (data.startsWith("elephant-ladder:") && data.length <= MAX_LINK_LENGTH && links.size < MAX_LINKS) {
      links.add(data)
    }
  }

  @Synchronized
  fun take(): List<String> = ArrayList(links).also { links.clear() }
}

/// Publications another application asked this one to open, staged where this process can
/// read them. The platform grant on a content URI lasts only as long as the activity, so the
/// bytes are copied when they arrive rather than when the host gets to them.
object OpenedPublicationInbox {
  private const val MAX_PUBLICATIONS = 8
  private const val MAX_BYTES = 4L * 1024 * 1024 * 1024
  private const val DIRECTORY = "opened-publications"
  private val publications = ArrayList<Pair<String, String>>()

  @Synchronized
  private fun offer(fileName: String, path: String) {
    if (publications.size < MAX_PUBLICATIONS) publications.add(fileName to path) else File(path).delete()
  }

  @Synchronized
  fun take(): List<Pair<String, String>> = ArrayList(publications).also { publications.clear() }

  fun receive(activity: Activity, intent: Intent?) {
    val uri = when (intent?.action) {
      Intent.ACTION_VIEW -> intent.data
      Intent.ACTION_SEND ->
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
          intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
          @Suppress("DEPRECATION")
          intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
        }
      else -> null
    } ?: return
    if (uri.scheme != "content" && uri.scheme != "file") return
    val context = activity.applicationContext
    Thread { stage(context, uri) }.start()
  }

  private fun stage(context: android.content.Context, uri: Uri) {
    val staging = File(context.cacheDir, DIRECTORY).apply { mkdirs() }
    val target = File(staging, "${java.util.UUID.randomUUID()}.epub")
    try {
      val copied = context.contentResolver.openInputStream(uri)?.use { input ->
        target.outputStream().use { output -> input.copyTo(output) }
      }
      if (copied == null || target.length() == 0L || target.length() > MAX_BYTES) {
        target.delete()
        return
      }
      offer(displayName(context, uri), target.absolutePath)
    } catch (error: Exception) {
      Log.w(TAG, "could not stage an opened publication: ${error.message}")
      target.delete()
    }
  }

  private fun displayName(context: android.content.Context, uri: Uri): String {
    val queried = runCatching {
      context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
        ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
    }.getOrNull()
    return queried ?: uri.lastPathSegment?.substringAfterLast('/') ?: "publication.epub"
  }
}

/// Whether this application wants the device's page buttons, which arrive as volume keys.
/// The activity must answer a key press at once, so the answer is kept where it can be read
/// synchronously rather than asked of the page.
object SideButtons {
  @Volatile
  var turnPages: Boolean = false
}

@InvokeArg
class SideButtonsArgs {
  var enabled: Boolean = false
}

@InvokeArg
class OpenUrlArgs {
  lateinit var url: String
}

@InvokeArg
class SaveFileArgs {
  lateinit var fileName: String
  lateinit var mediaType: String
  lateinit var contents: String
}

@InvokeArg
class PurgeOriginArgs {
  lateinit var origin: String
}

@InvokeArg
class NarrationPlaybackArgs {
  var state: String? = null
  var title: String = ""
  var section: String? = null
  var positionMs: Long? = null
  var durationMs: Long? = null
  var rate: Float = 1f
}

@TauriPlugin
class NativeActionsPlugin(private val activity: Activity) : Plugin(activity) {
  init {
    if (isBoox()) {
      val mode = booxRefreshMode()
      Log.i(TAG, "BOOX app refresh mode is ${mode?.second ?: "unknown"} (${mode?.first ?: "?"})")
    }
  }

  /// The refresh mode is the device's to choose: the per-application profile pins it, and a
  /// `setAppScopeRefreshMode` call changes nothing however it is asked. The application reads
  /// what the device decided, as the number the scope reports and the alias the profile names
  /// it by, so the choice can be shown rather than guessed at.
  private fun booxRefreshMode(): Pair<Int, String>? =
    try {
      val helper = Class.forName(EINK_HELPER_CLASS)
      val value = helper.getMethod("getAppScopeRefreshMode").invoke(null) as Int
      val config =
        helper.getMethod("getRefreshConfig", android.content.Context::class.java).invoke(null, activity)
      val alias =
        config?.let { it.javaClass.getMethod("getRefreshModeAlias").invoke(it) as? String } ?: "mode-$value"
      value to alias
    } catch (error: Throwable) {
      Log.w(TAG, "Could not read the BOOX refresh mode: ${error.message}")
      null
    }

  private fun isBoox(): Boolean {
    val identities = listOf(Build.MANUFACTURER, Build.BRAND)
    return identities.any {
      it.equals("onyx", ignoreCase = true) || it.equals("boox", ignoreCase = true)
    }
  }

  @Command
  fun displayTechnology(invoke: Invoke) {
    val response = JSObject().put("technology", if (isBoox()) "eink" else "unknown")
    if (isBoox()) {
      booxRefreshMode()?.let { response.put("refreshMode", it.second) }
      booxSpeedModeWhileDragging()?.let { response.put("speedModeWhileDragging", it) }
    }
    invoke.resolve(response)
  }

  /// Whether the device drags this application in its fast waveform. The profile field is
  /// `eacScrollStyle`: 1 takes the fast waveform and pays for it with a flashing refresh when the
  /// finger lifts, 2 declines. Read only; the device's own settings are the place to change it.
  private fun booxSpeedModeWhileDragging(): Boolean? =
    try {
      val config =
        Class.forName(EINK_HELPER_CLASS)
          .getMethod("getAppConfigFromService", String::class.java)
          .invoke(null, activity.packageName)
      val activityConfig = config?.let { it.javaClass.getMethod("getGlobalActivityConfig").invoke(it) }
      when (activityConfig?.let { it.javaClass.getMethod("getEacScrollStyle").invoke(it) as? Int }) {
        1 -> true
        2 -> false
        else -> null
      }
    } catch (error: Throwable) {
      Log.w(TAG, "Could not read the BOOX drag refresh style: ${error.message}")
      null
    }

  @Command
  fun frontLightDialogAvailable(invoke: Invoke) {
    invoke.resolve(JSObject().put("available", isBoox()))
  }

  @Command
  fun showFrontLightDialog(invoke: Invoke) {
    if (!isBoox()) {
      invoke.reject("front-light dialog is unavailable")
      return
    }
    try {
      activity.sendBroadcast(Intent(ACTION_SHOW_FRONT_LIGHT_DIALOG))
      invoke.resolve()
    } catch (error: Throwable) {
      invoke.reject("could not show the front-light dialog: ${error.message}")
    }
  }

  @Command
  fun openUrl(invoke: Invoke) {
    val args = invoke.parseArgs(OpenUrlArgs::class.java)
    try {
      val uri = Uri.parse(args.url)
      if ((uri.scheme != "http" && uri.scheme != "https") || uri.host.isNullOrEmpty() || uri.userInfo != null) {
        invoke.reject("invalid external URL")
        return
      }
      activity.startActivity(
        Intent(Intent.ACTION_VIEW, uri).addCategory(Intent.CATEGORY_BROWSABLE),
      )
      invoke.resolve()
    } catch (error: Throwable) {
      invoke.reject("could not open external URL: ${error.message}")
    }
  }

  @Command
  fun saveFile(invoke: Invoke) {
    val args = invoke.parseArgs(SaveFileArgs::class.java)
    val intent = Intent(Intent.ACTION_CREATE_DOCUMENT)
      .addCategory(Intent.CATEGORY_OPENABLE)
      .setType(args.mediaType)
      .putExtra(Intent.EXTRA_TITLE, args.fileName)
    startActivityForResult(invoke, intent, "saveFileResult")
  }

  @ActivityCallback
  private fun saveFileResult(invoke: Invoke, result: ActivityResult) {
    val uri = result.data?.data
    if (result.resultCode != Activity.RESULT_OK || uri == null) {
      invoke.resolve(JSObject().put("location", JSONObject.NULL))
      return
    }
    val args = invoke.parseArgs(SaveFileArgs::class.java)
    try {
      val resolver = activity.contentResolver
      resolver.openOutputStream(uri, "wt")!!.use {
        it.write(args.contents.toByteArray(Charsets.UTF_8))
      }
      val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
        ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
        ?: args.fileName
      invoke.resolve(JSObject().put("location", name))
    } catch (error: Throwable) {
      invoke.reject("could not save file: ${error.message}")
    }
  }

  @Command
  fun takeAnnotationLinks(invoke: Invoke) {
    val links = JSArray()
    AnnotationLinkInbox.take().forEach { links.put(it) }
    invoke.resolve(JSObject().put("links", links))
  }

  @Command
  fun setSideButtonsTurnPages(invoke: Invoke) {
    SideButtons.turnPages = invoke.parseArgs(SideButtonsArgs::class.java).enabled
    invoke.resolve()
  }

  @Command
  fun takeOpenedPublications(invoke: Invoke) {
    val publications = JSArray()
    OpenedPublicationInbox.take().forEach { (fileName, path) ->
      publications.put(JSObject().put("fileName", fileName).put("path", path))
    }
    invoke.resolve(JSObject().put("publications", publications))
  }

  @Command
  fun purgeOriginWebsiteData(invoke: Invoke) {
    val args = invoke.parseArgs(PurgeOriginArgs::class.java)
    try {
      val uri = Uri.parse(args.origin)
      val host = uri.host
      if (
        uri.scheme != "http" ||
        host == null ||
        !host.matches(Regex("p-[0-9a-f]{32}\\.localhost")) ||
        uri.port !in 1..65535 ||
        uri.userInfo != null ||
        !uri.path.isNullOrEmpty() ||
        uri.query != null ||
        uri.fragment != null
      ) {
        invoke.reject("invalid publication origin")
        return
      }
      WebStorage.getInstance().deleteOrigin(args.origin)
      Log.i(TAG, "publication-origin WebStorage purge attempted")
      invoke.resolve()
    } catch (error: Throwable) {
      invoke.reject("could not attempt publication-origin WebStorage purge: ${error.message}")
    }
  }

  @Command
  fun setNarrationPlayback(invoke: Invoke) {
    val args = invoke.parseArgs(NarrationPlaybackArgs::class.java)
    val context = activity.applicationContext
    when (args.state) {
      null -> NarrationService.stop(context)
      "playing", "paused" -> NarrationService.update(
        context,
        NarrationDisplay(
          playing = args.state == "playing",
          title = args.title.take(200),
          section = args.section?.take(200),
          positionMs = args.positionMs,
          durationMs = args.durationMs,
          rate = args.rate,
        ),
      )
      else -> {
        invoke.reject("invalid narration playback state")
        return
      }
    }
    invoke.resolve()
  }

  @Command
  fun takeNarrationCommands(invoke: Invoke) {
    val commands = JSArray()
    NarrationService.takeCommands().forEach { commands.put(it) }
    invoke.resolve(JSObject().put("commands", commands))
  }
}
