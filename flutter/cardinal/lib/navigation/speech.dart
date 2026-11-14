/// Speaking announcements and keeping the screen on while navigating, behind interfaces so
/// tests can count what's said without a speech engine.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
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

/// What holds audio focus while navigation speaks, so other audio is turned down under
/// the announcements and back up once they're done.
abstract class SpeechFocus {
  /// Takes focus; nothing if it's held.
  Future<void> request();

  /// Gives focus back; nothing if it isn't held.
  Future<void> abandon();
}

/// [SpeechFocus] on Android, through the app's native code on `cardinal/navigation`
/// (`SpeechFocus.kt`), which holds one focus request, tagged as navigation guidance.
class AndroidSpeechFocus implements SpeechFocus {
  const AndroidSpeechFocus();

  static const channel = MethodChannel('cardinal/navigation');

  @override
  Future<void> request() => _quietly('requestSpeechFocus');

  @override
  Future<void> abandon() => _quietly('abandonSpeechFocus');

  static Future<void> _quietly(String method) async {
    try {
      await channel.invokeMethod<Object?>(method);
    } on Object catch (e) {
      // Speech goes on, just without turning other audio down.
      debugPrint('navigation: speech focus: $e');
    }
  }
}

/// How long [PlatformSpeaker] holds focus with announcements queued and no word from the
/// speech engine, before letting go anyway: an announcement that's started or finished
/// (or one more queued) starts it again.
///
/// Announcements take a few seconds each, and an engine starting up cold a few more, so
/// half a minute of silence means an event was lost, not a long announcement; letting go
/// then brings other audio back up, rather than leaving it turned down for the rest of the
/// trip. Too short, and the rest of an announcement would be said without ducking.
const speechFocusTimeout = Duration(seconds: 30);

/// [Speaker] on the platform's speech engine, through `flutter_tts`, set up as navigation
/// apps' voices are: other audio is turned down while it speaks, and it keeps speaking
/// with the screen off.
///
/// On Android, audio focus is held through a [SpeechFocus] rather than `flutter_tts`'s own, which
/// makes a request per announcement and keeps only the last, so queued announcements leak
/// requests and leave other audio ducked after the trip; and whose cancelled
/// announcement gives back the request made for the one said after it. Here focus is taken
/// as the first of a batch of announcements is queued, held while any is queued or being
/// said, and given back as the last finishes, on [stop], or after [speechFocusTimeout]
/// without word from the engine.
class PlatformSpeaker implements Speaker {
  PlatformSpeaker({
    FlutterTts? tts,
    SpeechFocus? focus,
    this.focusTimeout = speechFocusTimeout,
  }) : _given = tts,
       _focus =
           focus ??
           (defaultTargetPlatform == TargetPlatform.android
               ? const AndroidSpeechFocus()
               : null);

  /// Told of each speech engine event a [PlatformSpeaker] hears, on Android, as `start`,
  /// `complete`, `cancel` or `error: <message>`: for development tools that watch the
  /// engine, which can't watch it through a `FlutterTts` of their own, as only the one
  /// made last hears its events.
  static void Function(String event)? debugEngineEvents;

  final FlutterTts? _given;

  /// Made as it's first needed, as a trip starts, rather than with the speaker: of the
  /// `FlutterTts` instances made, only the last hears the engine's events.
  late final FlutterTts _tts = _listen(_given ?? FlutterTts());

  FlutterTts _listen(FlutterTts tts) {
    if (_focus == null) return tts;
    void heard(String event) => debugEngineEvents?.call(event);
    return tts
      ..setStartHandler(() {
        heard('start');
        _started();
      })
      ..setCompletionHandler(() {
        heard('complete');
        _finished();
      })
      ..setErrorHandler((message) {
        heard('error: $message');
        debugPrint('navigation: speech: $message');
        _failed();
      })
      // A cancelled announcement is one [stop] dropped, and [stop] has already let go
      // of it: its cancel, which can come after the next announcement is queued, as on
      // a reroute, is left alone.
      ..setCancelHandler(() => heard('cancel'));
  }

  /// Audio focus while speaking, on Android; null where `flutter_tts` sees to it.
  final SpeechFocus? _focus;

  /// How long focus is held without word from the speech engine: [speechFocusTimeout].
  final Duration focusTimeout;

  bool _configured = false;

  /// Announcements queued and not yet finished, since the last [stop].
  int _queued = 0;

  /// Whether one of them has started and not yet finished: a finish without a start is
  /// one from before the last [stop], come late, and isn't counted.
  bool _playing = false;

  /// [stop]s waiting on the engine. What the engine says meanwhile is about what they
  /// stopped, never about announcements queued after them, which it only hears of later.
  int _stopping = 0;

  /// Whether focus has been asked for and not given back.
  bool _held = false;

  Timer? _timeout;

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
        // session is let go after each announcement that finishes (flutter_tts's
        // default), which brings the other audio back up. One cut off by [stop] doesn't
        // let it go: flutter_tts only does as an announcement finishes, so other audio
        // stays down until the next one does.
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
    if (_focus != null) {
      _queued++;
      _hold();
      _wait();
    }
    try {
      // Focus is [_focus]'s on Android, and `flutter_tts` has no say in it elsewhere.
      await _tts.speak(text, focus: false);
    } on Object catch (e) {
      debugPrint('navigation: speech: $e');
      // Not queued, so it won't finish.
      if (_focus != null) _done();
    }
  }

  @override
  Future<void> stop() async {
    if (_focus != null) {
      _queued = 0;
      _playing = false;
      _stopping++;
      _timeout?.cancel();
    }
    try {
      await _tts.stop();
    } on Object catch (e) {
      debugPrint('navigation: speech: $e');
    } finally {
      if (_focus != null) {
        _stopping--;
        // Unless something was queued since, as "Rerouting" is right after the stop on
        // a reroute: then focus stays held for it.
        if (_queued == 0) _letGo();
      }
    }
  }

  void _started() {
    if (_stopping > 0) return;
    _playing = true;
    _wait();
  }

  void _finished() {
    if (_stopping > 0 || !_playing) return;
    _playing = false;
    _done();
  }

  void _failed() {
    if (_stopping > 0) return;
    _playing = false;
    _done();
  }

  /// One announcement fewer to wait for; focus goes with the last.
  void _done() {
    if (_queued > 0) _queued--;
    if (_queued == 0) {
      _letGo();
    } else {
      _wait();
    }
  }

  void _hold() {
    if (_held) return;
    _held = true;
    unawaited(_focus!.request());
  }

  void _letGo() {
    _timeout?.cancel();
    _timeout = null;
    if (!_held) return;
    _held = false;
    unawaited(_focus!.abandon());
  }

  /// Starts [focusTimeout] again.
  void _wait() {
    _timeout?.cancel();
    _timeout = Timer(focusTimeout, () {
      debugPrint(
        'navigation: speech: no word from the engine in $focusTimeout, '
        'letting go of focus with $_queued queued',
      );
      _queued = 0;
      _playing = false;
      _letGo();
    });
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
