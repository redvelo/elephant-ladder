package com.redvelo.elephant_ladder

import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.view.KeyEvent
import android.util.Log
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  private external fun installFixtureSelection(arguments: Array<String>): Boolean
  private var readerWebView: WebView? = null
  override val handleBackNavigation: Boolean = false

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    readerWebView = webView
    ReaderDisplayGeometry.install(webView)
  }

  // The reader page has no Tauri bridge, so Wry's default Back handling would finish the
  // activity from anywhere in the app. Ask the page first; it closes an open book or dialog and
  // reports whether it did. Only an unhandled Back (at the library) leaves the app.
  @Deprecated("Deprecated in Java")
  override fun onBackPressed() {
    val webView = readerWebView
    if (webView == null) {
      finish()
      return
    }
    webView.evaluateJavascript(
      "(function(){try{return window.__elephantLadderBack?window.__elephantLadderBack()===true:false}catch(e){return false}})()"
    ) { result -> if (result != "true") finish() }
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    if (BuildConfig.APPLICATION_ID == FIXTURE_APPLICATION_ID) {
      withdrawPublicationOpening()
      val arguments = ArrayList<String>()
      intent.getStringExtra(FIXTURE_ID_EXTRA)?.let { arguments += listOf("--fixture", it) }
      intent.getStringExtra(FIXTURE_EPUB_EXTRA)?.let { arguments += listOf("--fixture-epub", it) }
      intent.getStringExtra(FIXTURE_OBSERVATION_EXTRA)?.let {
        arguments += listOf("--observation", it)
      }
      if (arguments.isEmpty()) {
        throw IllegalArgumentException("Missing fixture-runner launch selection")
      }
      System.loadLibrary("elephant_ladder_lib")
      if (!installFixtureSelection(arguments.toTypedArray())) {
        throw IllegalArgumentException("Invalid fixture-runner launch selection")
      }
    }
    super.onCreate(savedInstanceState)
    if (savedInstanceState == null) {
      AnnotationLinkInbox.receive(intent)
      OpenedPublicationInbox.receive(this, intent)
    }
    enterFullscreen()
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    AnnotationLinkInbox.receive(intent)
    OpenedPublicationInbox.receive(this, intent)
  }

  // An e-reader's page buttons are its volume keys, and the system consumes those before any
  // web content sees them. When the application asked for the buttons, the press is taken here
  // and handed to the page; otherwise it stays a volume key.
  override fun dispatchKeyEvent(event: KeyEvent): Boolean {
    val direction = when (event.keyCode) {
      KeyEvent.KEYCODE_VOLUME_UP -> -1
      KeyEvent.KEYCODE_VOLUME_DOWN -> 1
      else -> null
    }
    if (direction == null || !SideButtons.turnPages) return super.dispatchKeyEvent(event)
    if (event.action == KeyEvent.ACTION_DOWN) turnPage(direction)
    return true
  }

  private fun turnPage(direction: Int) {
    readerWebView?.evaluateJavascript(
      "(function(){try{return window.__elephantLadderTurnPage?window.__elephantLadderTurnPage($direction)===true:false}catch(e){return false}})()",
      null,
    )
  }

  override fun onWindowFocusChanged(hasFocus: Boolean) {
    super.onWindowFocusChanged(hasFocus)
    if (hasFocus) enterFullscreen()
  }

  // The fixture runner shares the product manifest, so it declares the same publication
  // filters. It is a test harness, never a way to read a book, so it takes itself out of the
  // chooser on every launch: a reinstall restores whatever the manifest itself declared.
  // This runs before the fixture selection is read, because a stray chooser tap arrives
  // without one and must still leave the alias disabled.
  private fun withdrawPublicationOpening() {
    runCatching {
      packageManager.setComponentEnabledSetting(
        ComponentName(this, "$PRODUCT_NAMESPACE.OpenPublicationActivity"),
        PackageManager.COMPONENT_ENABLED_STATE_DISABLED,
        PackageManager.DONT_KILL_APP,
      )
    }.onFailure { error ->
      Log.w("MainActivity", "could not withdraw publication opening: ${error.message}")
    }
  }

  private fun enterFullscreen() {
    WindowCompat.getInsetsController(window, window.decorView).apply {
      hide(WindowInsetsCompat.Type.systemBars())
      systemBarsBehavior =
        WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
    }
  }

  companion object {
    private const val PRODUCT_NAMESPACE = "com.redvelo.elephant_ladder"
    private const val FIXTURE_APPLICATION_ID =
      "com.redvelo.elephant_ladder.fixture_runner"
    const val FIXTURE_ID_EXTRA =
      "com.redvelo.elephant_ladder.fixture_runner.FIXTURE_ID"
    const val FIXTURE_EPUB_EXTRA =
      "com.redvelo.elephant_ladder.fixture_runner.FIXTURE_EPUB"
    const val FIXTURE_OBSERVATION_EXTRA =
      "com.redvelo.elephant_ladder.fixture_runner.FIXTURE_OBSERVATION"
  }
}
