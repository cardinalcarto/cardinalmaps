/// Speaking announcements and keeping the screen on while navigating, behind interfaces so
/// tests can count what's said without a speech engine.
library;

import 'package:flutter/foundation.dart';
import 'package:flutter_tts/flutter_tts.dart';
import 'package:wakelock_plus/wakelock_plus.dart';

/// What speaks navigation's announcements.
abstract class Speaker {
  /// Gets ready to speak in [language], a BCP-47 tag such as `pt-PT`. Speaking without a
  /// voice for it is left to the platform, which falls back to its own.
  Future<void> prepare(String language);

  /// Says [text], after anything still being said.
  Future<void> speak(String text);

  /// Stops speaking, and drops anything waiting to be said.
  Future<void> stop();
}

/// [Speaker] on the platform's speech engine, through `flutter_tts`, set up as navigation
/// apps' voices are: other audio is turned down while it speaks, and it keeps speaking
/// with the screen off.
class PlatformSpeaker implements Speaker {
  PlatformSpeaker([FlutterTts? tts]) : _tts = tts ?? FlutterTts();

  final FlutterTts _tts;
  bool _configured = false;

  /// `flutter_tts`'s Android queue mode that adds to what's being said, rather than cutting
  /// it off: two announcements due close together, at short steps, are both heard.
  static const _queueAdd = 1;

  @override
  Future<void> prepare(String language) async {
    try {
      if (!_configured) {
        _configured = true;
        await _configure();
      }
      // Each platform answers 1 if it has a voice for the language, and 0 if not.
      final set = await _tts.setLanguage(language);
      if (set != 1) {
        debugPrint('navigation: no voice for $language, using the default');
      }
    } on Object catch (e) {
      debugPrint('navigation: speech: $e');
    }
  }

  Future<void> _configure() async {
    switch (defaultTargetPlatform) {
      case TargetPlatform.android:
        // Marks the speech as navigation guidance, which Android routes and mixes as
        // such: over a car's speakers, and through Do Not Disturb.
        await _tts.setAudioAttributesForNavigation();
        await _tts.setQueueMode(_queueAdd);
      case TargetPlatform.iOS:
        // Playback keeps speaking with the screen locked (with the `audio` background
        // mode); ducking turns other audio down rather than stopping it, and spoken audio
        // such as a podcast pauses instead, so no words are lost under the voice. The
        // session is let go after each announcement (flutter_tts's default), which
        // brings the other audio back up.
        await _tts.setIosAudioCategory(
          IosTextToSpeechAudioCategory.playback,
          const [
            IosTextToSpeechAudioCategoryOptions.duckOthers,
            IosTextToSpeechAudioCategoryOptions
                .interruptSpokenAudioAndMixWithOthers,
            IosTextToSpeechAudioCategoryOptions.allowBluetoothA2DP,
          ],
          IosTextToSpeechAudioMode.voicePrompt,
        );
      default:
        break;
    }
  }

  @override
  Future<void> speak(String text) async {
    try {
      // On Android, `focus` asks for audio focus that lets other audio duck, and gives it
      // back when the announcement is done.
      await _tts.speak(text, focus: true);
    } on Object catch (e) {
      debugPrint('navigation: speech: $e');
    }
  }

  @override
  Future<void> stop() async {
    try {
      await _tts.stop();
    } on Object catch (e) {
      debugPrint('navigation: speech: $e');
    }
  }
}

/// What keeps the screen on.
abstract class ScreenWake {
  Future<void> keepOn();
  Future<void> release();
}

/// [ScreenWake] through `wakelock_plus`.
class PlatformScreenWake implements ScreenWake {
  const PlatformScreenWake();

  @override
  Future<void> keepOn() => _quietly(WakelockPlus.enable);

  @override
  Future<void> release() => _quietly(WakelockPlus.disable);

  static Future<void> _quietly(Future<void> Function() call) async {
    try {
      await call();
    } on Object catch (e) {
      // A platform without it; navigating with the screen going off still works.
      debugPrint('navigation: wakelock: $e');
    }
  }
}

/// [Speaker] that writes each announcement to the log as `cardinal-speech: <text>`, for
/// the Linux desktop, which `flutter_tts` has no speech engine for: a trip there is for
/// development, and the log shows what would have been said, and when.
class LogSpeaker implements Speaker {
  const LogSpeaker();

  @override
  Future<void> prepare(String language) async =>
      debugPrint('cardinal-speech: (voice for $language)');

  @override
  Future<void> speak(String text) async => debugPrint('cardinal-speech: $text');

  @override
  Future<void> stop() async {}
}
