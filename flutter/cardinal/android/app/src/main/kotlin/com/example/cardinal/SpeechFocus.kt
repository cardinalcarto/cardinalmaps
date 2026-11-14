package com.cardinalcarto.cardinal

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log

/**
 * The audio focus navigation's speech holds while announcements are queued or being
 * said, so other audio ducks under them and comes back up once they're done.
 *
 * One request, held by the app: asking while it's held, or letting go while it isn't,
 * does nothing, so the system's focus stack never has more than one entry of ours, however
 * many announcements are queued. (`flutter_tts`'s own `focus` makes a new request for each
 * utterance and keeps only the last, leaking the others.) Dart decides when to ask and
 * let go (`PlatformSpeaker` in `speech.dart`).
 *
 * The request is tagged as navigation guidance, as the speech itself is, so the system
 * mixes and routes it as such.
 *
 * Losing focus, as to a phone call, is written to the log and otherwise left alone: speech
 * doesn't ask again until its next batch of announcements. A permanent loss gives our
 * request back at once; a transient one keeps it, to be given back as the batch ends.
 */
class SpeechFocus(context: Context) {
    private val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private val handler = Handler(Looper.getMainLooper())
    private var held = false

    private val listener = AudioManager.OnAudioFocusChangeListener { change ->
        when (change) {
            AudioManager.AUDIOFOCUS_LOSS -> {
                // Given back at once: whatever took it is playing for good, and the rest
                // of this batch is said without ducking anything.
                Log.i(TAG, "speech focus lost")
                abandon()
            }
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK,
            -> Log.i(TAG, "speech focus lost for now ($change)")
            AudioManager.AUDIOFOCUS_GAIN -> Log.i(TAG, "speech focus back")
        }
    }

    private val request: AudioFocusRequest? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_ASSISTANCE_NAVIGATION_GUIDANCE)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                        .build(),
                )
                .setOnAudioFocusChangeListener(listener, handler)
                .build()
        } else {
            null
        }

    /** Asks for focus, unless it's held; whether it's held afterwards. */
    fun request(): Boolean {
        if (held) return true
        val answer = if (request != null) {
            audio.requestAudioFocus(request)
        } else {
            @Suppress("DEPRECATION")
            audio.requestAudioFocus(
                listener,
                AudioManager.STREAM_MUSIC,
                AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK,
            )
        }
        held = answer == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        if (held) {
            Log.d(TAG, "speech focus taken")
        } else {
            // Refused during a call, say: the announcement is still said, just without
            // ducking anything, and nothing is held to give back.
            Log.i(TAG, "speech focus refused ($answer)")
        }
        return held
    }

    /** Gives focus back, if it's held. */
    fun abandon() {
        if (!held) return
        held = false
        Log.d(TAG, "speech focus given back")
        if (request != null) {
            audio.abandonAudioFocusRequest(request)
        } else {
            @Suppress("DEPRECATION")
            audio.abandonAudioFocus(listener)
        }
    }

    private companion object {
        const val TAG = "cardinal"
    }
}
