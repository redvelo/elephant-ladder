package com.redvelo.elephant_ladder

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import java.util.concurrent.ConcurrentLinkedQueue

private const val NARRATION_TAG = "NarrationService"
private const val NARRATION_CHANNEL = "narration"
private const val NARRATION_NOTIFICATION = 7301
private const val ACTION_PLAY = "com.redvelo.elephant_ladder.narration.PLAY"
private const val ACTION_PAUSE = "com.redvelo.elephant_ladder.narration.PAUSE"
private const val ACTION_NEXT = "com.redvelo.elephant_ladder.narration.NEXT"
private const val ACTION_PREVIOUS = "com.redvelo.elephant_ladder.narration.PREVIOUS"
private const val MAX_PENDING_COMMANDS = 16

data class NarrationDisplay(
  val playing: Boolean,
  val title: String,
  val section: String?,
  val positionMs: Long?,
  val durationMs: Long?,
  val rate: Float,
  val updatedAt: Long = SystemClock.elapsedRealtime(),
)

// Android mutes background media playback unless a mediaPlayback foreground service owns it.
class NarrationService : Service() {
  companion object {
    private val commands = ConcurrentLinkedQueue<String>()
    private val mainThread = Handler(Looper.getMainLooper())

    @Volatile private var narrationDisplay = NarrationDisplay(false, "", null, null, null, 1f)
    @Volatile private var instance: NarrationService? = null

    fun update(context: Context, next: NarrationDisplay) {
      narrationDisplay = next
      mainThread.post {
        val running = instance
        if (running != null) {
          running.render()
          return@post
        }
        // Only playback started from the visible reader may start the service.
        if (!next.playing) return@post
        val intent = Intent(context, NarrationService::class.java)
        try {
          if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.startForegroundService(intent)
          } else {
            context.startService(intent)
          }
        } catch (error: RuntimeException) {
          Log.w(NARRATION_TAG, "Could not start narration playback service", error)
        }
      }
    }

    fun stop(context: Context) {
      narrationDisplay = narrationDisplay.copy(playing = false)
      commands.clear()
      mainThread.post { context.stopService(Intent(context, NarrationService::class.java)) }
    }

    fun takeCommands(): List<String> = generateSequence { commands.poll() }.toList()

    private fun enqueue(command: String) {
      if (commands.size < MAX_PENDING_COMMANDS) commands.add(command)
    }
  }

  private lateinit var session: MediaSession

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onCreate() {
    super.onCreate()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      getSystemService(NotificationManager::class.java).createNotificationChannel(
        NotificationChannel(NARRATION_CHANNEL, "Narration", NotificationManager.IMPORTANCE_LOW),
      )
    }
    session = MediaSession(this, NARRATION_TAG).apply {
      setCallback(object : MediaSession.Callback() {
        override fun onPlay() = enqueue("play")
        override fun onPause() = enqueue("pause")
        override fun onStop() = enqueue("pause")
        override fun onSkipToNext() = enqueue("next")
        override fun onSkipToPrevious() = enqueue("previous")
      })
      isActive = true
    }
    instance = this
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_PLAY -> enqueue("play")
      ACTION_PAUSE -> enqueue("pause")
      ACTION_NEXT -> enqueue("next")
      ACTION_PREVIOUS -> enqueue("previous")
    }
    render()
    return START_NOT_STICKY
  }

  override fun onTaskRemoved(rootIntent: Intent?) {
    stopSelf()
  }

  override fun onDestroy() {
    instance = null
    session.release()
    super.onDestroy()
  }

  private fun segmentAction(action: String, icon: Int, label: String): Notification.Action {
    val intent = PendingIntent.getService(
      this,
      action.hashCode(),
      Intent(this, NarrationService::class.java).setAction(action),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    return Notification.Action.Builder(Icon.createWithResource(this, icon), label, intent).build()
  }

  private fun render() {
    val current = narrationDisplay
    val heading = current.section ?: current.title
    val subtitle = if (current.section == null) "Narration" else current.title
    session.setPlaybackState(
      PlaybackState.Builder()
        .setActions(
          PlaybackState.ACTION_PLAY or
            PlaybackState.ACTION_PAUSE or
            PlaybackState.ACTION_PLAY_PAUSE or
            PlaybackState.ACTION_SKIP_TO_NEXT or
            PlaybackState.ACTION_SKIP_TO_PREVIOUS,
        )
        .setState(
          if (current.playing) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED,
          current.positionMs ?: PlaybackState.PLAYBACK_POSITION_UNKNOWN,
          if (current.playing) current.rate else 0f,
          current.updatedAt,
        )
        .build(),
    )
    val metadata = MediaMetadata.Builder()
      .putString(MediaMetadata.METADATA_KEY_TITLE, heading)
      .putString(MediaMetadata.METADATA_KEY_ARTIST, subtitle)
      .putString(MediaMetadata.METADATA_KEY_ALBUM, current.title)
    current.durationMs?.let { metadata.putLong(MediaMetadata.METADATA_KEY_DURATION, it) }
    session.setMetadata(metadata.build())
    val toggle = PendingIntent.getService(
      this,
      0,
      Intent(this, NarrationService::class.java).setAction(if (current.playing) ACTION_PAUSE else ACTION_PLAY),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val action = Notification.Action.Builder(
      Icon.createWithResource(
        this,
        if (current.playing) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play,
      ),
      if (current.playing) "Pause" else "Play",
      toggle,
    ).build()
    val previous = segmentAction(ACTION_PREVIOUS, android.R.drawable.ic_media_previous, "Previous")
    val next = segmentAction(ACTION_NEXT, android.R.drawable.ic_media_next, "Next")
    val content = packageManager.getLaunchIntentForPackage(packageName)?.let {
      PendingIntent.getActivity(this, 0, it, PendingIntent.FLAG_IMMUTABLE)
    }
    val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      Notification.Builder(this, NARRATION_CHANNEL)
    } else {
      @Suppress("DEPRECATION")
      Notification.Builder(this)
    }
    val notification = builder
      .setSmallIcon(android.R.drawable.ic_media_play)
      .setContentTitle(heading)
      .setContentText(subtitle)
      .setContentIntent(content)
      .setOngoing(current.playing)
      .addAction(previous)
      .addAction(action)
      .addAction(next)
      .setStyle(
        Notification.MediaStyle()
          .setMediaSession(session.sessionToken)
          .setShowActionsInCompactView(0, 1, 2),
      )
      .build()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      startForeground(NARRATION_NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
    } else {
      startForeground(NARRATION_NOTIFICATION, notification)
    }
  }
}
