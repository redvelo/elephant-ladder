package com.redvelo.elephant_ladder

import android.os.Build
import android.graphics.Rect
import android.view.RoundedCorner
import android.webkit.WebView

internal object ReaderDisplayGeometry {
  fun install(webView: WebView) {
    var applied: String? = null
    var pending = false
    webView.viewTreeObserver.addOnPreDrawListener {
      if (webView.progress < 100) applied = null
      val insets = webView.rootWindowInsets
      if (insets != null && webView.width > 0 && webView.height > 0 && !pending) {
        val location = IntArray(2)
        webView.getLocationInWindow(location)
        val left = location[0]
        val top = location[1]
        val right = left + webView.width
        val bottom = top + webView.height
        val corners = IntArray(12)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
          val positions = intArrayOf(
            RoundedCorner.POSITION_TOP_LEFT, RoundedCorner.POSITION_TOP_RIGHT,
            RoundedCorner.POSITION_BOTTOM_LEFT, RoundedCorner.POSITION_BOTTOM_RIGHT,
          )
          positions.forEachIndexed { index, position ->
            val corner = insets.getRoundedCorner(position)
            if (corner != null) {
              corners[index * 3] = corner.radius
              corners[index * 3 + 1] = corner.center.x - left
              corners[index * 3 + 2] = corner.center.y - top
            }
          }
        }
        val cutout = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) insets.displayCutout else null
        val bounds = Rect()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
          cutout?.boundingRects?.forEach { bounds.union(it) }
        }
        val values = corners.toList() + listOf(
          ((cutout?.safeInsetLeft ?: 0) - left).coerceAtLeast(0),
          (right - (webView.rootView.width - (cutout?.safeInsetRight ?: 0))).coerceAtLeast(0),
          ((cutout?.safeInsetTop ?: 0) - top).coerceAtLeast(0),
          bounds.left - left, bounds.top - top, bounds.right - left, bounds.bottom - top,
        )
        val key = "${webView.url}:${webView.width}:${webView.height}:$values"
        if (applied != key) {
          pending = true
          webView.evaluateJavascript(
            """(() => {
              const root = document.documentElement;
              if (!root) return false;
              const names = ['top-left', 'top-right', 'bottom-left', 'bottom-right']
                .flatMap(corner => ['radius', 'x', 'y'].map(field => 'corner-' + corner + '-' + field))
                .concat(['cutout-left', 'cutout-right', 'cutout-top',
                  'cutout-bounds-left', 'cutout-bounds-top', 'cutout-bounds-right', 'cutout-bounds-bottom']);
              const values = [${values.joinToString(",")}];
              const viewport = window.visualViewport;
              const scale = (viewport ? viewport.width * viewport.scale : window.innerWidth) / ${webView.width};
              names.forEach((name, index) => root.style.setProperty(
                '--reader-host-' + name, (values[index] * scale) + 'px'));
              return true;
            })()""".trimIndent()
          ) { result ->
            pending = false
            if (result == "true" && webView.progress == 100) applied = key
          }
        }
      }
      true
    }
  }

}
