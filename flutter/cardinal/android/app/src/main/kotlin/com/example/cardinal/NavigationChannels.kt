package com.cardinalcarto.cardinal

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

/**
 * Navigation's platform channels: `cardinal/navigation`, for the notification and its
 * permission and for speech's audio focus ([SpeechFocus]), and `cardinal/navigation/fixes`,
 * navigation's own location updates ([NavigationFixes]).
 *
 * Other native help navigation needs can go on `cardinal/navigation` beside these.
 *
 * Methods, from Dart:
 * - `show` `{title, text?, glyph?, channel, end}`: shows the notification, starting
 *   [NavigationService] if it isn't running.
 * - `hide`: stops [NavigationService], and with it the notification.
 * - `requestNotificationPermission`: asks for `POST_NOTIFICATIONS` on Android 13 and
 *   later, answering whether notifications may be posted.
 * - `requestSpeechFocus`: takes audio focus for speech, unless it's held, answering whether
 *   it's held.
 * - `abandonSpeechFocus`: gives it back, if it's held.
 *
 * And to Dart: `end`, when the notification's End is tapped.
 */
class NavigationChannels(
    private val activity: Activity,
    messenger: BinaryMessenger,
) : MethodChannel.MethodCallHandler {
    private val methods = MethodChannel(messenger, "cardinal/navigation")
    private val fixes = EventChannel(messenger, "cardinal/navigation/fixes")
    private val fixHandler = NavigationFixes(activity.applicationContext)
    private val speechFocus = SpeechFocus(activity.applicationContext)
    private var permissionResult: MethodChannel.Result? = null

    init {
        methods.setMethodCallHandler(this)
        fixes.setStreamHandler(fixHandler)
        NavigationService.onEnd = { methods.invokeMethod("end", null) }
    }

    fun dispose() {
        NavigationService.onEnd = null
        NavigationService.hide(activity.applicationContext)
        fixHandler.onCancel(null)
        speechFocus.abandon()
        methods.setMethodCallHandler(null)
        fixes.setStreamHandler(null)
        permissionResult?.success(false)
        permissionResult = null
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "show" -> {
                NavigationService.show(
                    activity.applicationContext,
                    NavigationService.Notice(
                        title = call.argument<String>("title") ?: "",
                        text = call.argument<String>("text"),
                        glyph = call.argument<Int>("glyph"),
                        channel = call.argument<String>("channel") ?: "",
                        end = call.argument<String>("end") ?: "",
                    ),
                )
                result.success(null)
            }
            "hide" -> {
                NavigationService.hide(activity.applicationContext)
                result.success(null)
            }
            "requestNotificationPermission" -> requestNotificationPermission(result)
            "requestSpeechFocus" -> result.success(speechFocus.request())
            "abandonSpeechFocus" -> {
                speechFocus.abandon()
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    private fun requestNotificationPermission(result: MethodChannel.Result) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            result.success(true)
            return
        }
        // One request at a time; a second waits on nothing and is told no.
        if (permissionResult != null) {
            result.success(false)
            return
        }
        permissionResult = result
        activity.requestPermissions(
            arrayOf(Manifest.permission.POST_NOTIFICATIONS),
            PERMISSION_REQUEST,
        )
    }

    fun onRequestPermissionsResult(requestCode: Int, grantResults: IntArray) {
        if (requestCode != PERMISSION_REQUEST) return
        val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
        // What was posted while it wasn't allowed wasn't shown; post it again now.
        if (granted) NavigationService.repost()
        permissionResult?.success(granted)
        permissionResult = null
    }

    private companion object {
        const val PERMISSION_REQUEST = 7301
    }
}
