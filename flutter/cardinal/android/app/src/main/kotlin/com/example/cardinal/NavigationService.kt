package com.cardinalcarto.cardinal

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import io.flutter.FlutterInjector

/**
 * The foreground service that keeps navigation going with the screen off, and its
 * notification: the next maneuver, or what the trip's status card says, with End.
 *
 * It's of type location, which makes the app's location access foreground access
 * while it runs, so [NavigationFixes] keeps getting fixes with the screen off; its
 * partial wake lock keeps the CPU up to take them, and Dart to follow the route with
 * them.
 *
 * Dart shows it from when a trip starts until it ends ([show], [hide]). It's started
 * as an ordinary service and promoted to the foreground as it starts, so that a start
 * the system refuses, such as one without location permission on Android 14 and later,
 * leaves navigation running without it rather than ending the app. Without the
 * notification permission it still runs, and the system lists it in its task manager
 * rather than the notification shade.
 */
class NavigationService : Service() {
    /** What the notification says, as Dart words it. */
    data class Notice(
        val title: String,
        val text: String?,
        /** The maneuver's code point in the app's CardinalIcons font, if it has one. */
        val glyph: Int?,
        /** The notification channel's name. */
        val channel: String,
        /** The End action's label. */
        val end: String,
    )

    companion object {
        private const val TAG = "CardinalNavigation"
        private const val NOTIFICATION_ID = 7300
        private const val CHANNEL_ID = "navigation"
        private const val ACTION_END = "com.cardinalcarto.cardinal.navigation.END"
        private const val FONT = "assets/fonts/CardinalIcons.ttf"

        /** Called on End; null while no engine is there to hear it. */
        var onEnd: (() -> Unit)? = null

        private var notice: Notice? = null
        private var running: NavigationService? = null
        private var starting = false

        /** Shows [notice], starting the service if it isn't running. */
        fun show(context: Context, notice: Notice) {
            this.notice = notice
            val service = running
            if (service != null) {
                service.post()
                return
            }
            if (starting) return
            try {
                context.startService(Intent(context, NavigationService::class.java))
                starting = true
            } catch (e: RuntimeException) {
                // In the background, where services can't be started; the next notice
                // shown in the foreground tries again.
                Log.w(TAG, "Couldn't start navigation's service", e)
            }
        }

        /** Stops the service, and takes the notification away. */
        fun hide(context: Context) {
            notice = null
            if (running != null || starting) {
                context.stopService(Intent(context, NavigationService::class.java))
            }
            // Let go of it now, rather than when it's destroyed, so a trip started right
            // after starts a service of its own.
            running = null
            starting = false
        }

        /** Posts the notification again, as after the permission to was given. */
        fun repost() {
            running?.post()
        }
    }

    private var wakeLock: PowerManager.WakeLock? = null
    private var glyphs: Pair<Typeface?, MutableMap<Pair<Int, Boolean>, Bitmap>>? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_END) {
            val end = onEnd
            if (end != null) end() else stopSelf()
            return START_NOT_STICKY
        }
        starting = false
        val notice = notice
        if (notice == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        try {
            ServiceCompat.startForeground(
                this,
                NOTIFICATION_ID,
                build(notice),
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_LOCATION
                } else {
                    0
                },
            )
        } catch (e: RuntimeException) {
            // Not allowed now: no location permission, or the app went to the
            // background before it started. Navigation goes on while the screen is on.
            Log.w(TAG, "Couldn't start navigation in the foreground", e)
            stopSelf()
            return START_NOT_STICKY
        }
        running = this
        if (wakeLock == null) {
            wakeLock = (getSystemService(Context.POWER_SERVICE) as PowerManager)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "cardinal:navigation")
                .apply {
                    setReferenceCounted(false)
                    acquire()
                }
        }
        // Not restarted if the system ends it: the trip lives in the app, and went with it.
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        if (running == this) running = null
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        wakeLock?.release()
        wakeLock = null
        super.onDestroy()
    }

    private fun post() {
        val notice = notice ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION_ID, build(notice))
    }

    private fun build(notice: Notice): Notification {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            // Quiet: speech says what's coming, through its own audio. Created each time,
            // which only renames it, to follow the device's language.
            val channel = NotificationChannel(
                CHANNEL_ID,
                notice.channel,
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                setSound(null, null)
                enableVibration(false)
                setShowBadge(false)
                lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            }
            getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        }
        // Back to the app as it is, on the navigation screen while a trip is under way.
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java)
                .setAction(Intent.ACTION_MAIN)
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val end = PendingIntent.getService(
            this,
            1,
            Intent(this, NavigationService::class.java).setAction(ACTION_END),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(notice.title)
            .setContentText(notice.text)
            .setLargeIcon(notice.glyph?.let(::glyph))
            .setCategory(NotificationCompat.CATEGORY_NAVIGATION)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setShowWhen(false)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setContentIntent(open)
            .addAction(0, notice.end, end)
            .build()
    }

    /**
     * [codePoint] of the app's icon font drawn at the notification's large-icon size, in
     * the shade's text color: white when the device is in its dark theme, near black
     * otherwise. Null if the font can't be loaded.
     */
    private fun glyph(codePoint: Int): Bitmap? {
        val cache = glyphs ?: run {
            val typeface = try {
                val key = FlutterInjector.instance().flutterLoader().getLookupKeyForAsset(FONT)
                Typeface.createFromAsset(assets, key)
            } catch (e: RuntimeException) {
                Log.w(TAG, "Couldn't load the icon font", e)
                null
            }
            (typeface to mutableMapOf<Pair<Int, Boolean>, Bitmap>()).also { glyphs = it }
        }
        val typeface = cache.first ?: return null
        val night = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK ==
            Configuration.UI_MODE_NIGHT_YES
        return cache.second.getOrPut(codePoint to night) {
            val size = resources.getDimensionPixelSize(android.R.dimen.notification_large_icon_width)
            val bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
            val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
                this.typeface = typeface
                textSize = size * 0.9f
                textAlign = Paint.Align.CENTER
                color = if (night) Color.WHITE else Color.argb(0xDE, 0, 0, 0)
            }
            val metrics = paint.fontMetrics
            Canvas(bitmap).drawText(
                String(Character.toChars(codePoint)),
                size / 2f,
                size / 2f - (metrics.ascent + metrics.descent) / 2f,
                paint,
            )
            bitmap
        }
    }
}
