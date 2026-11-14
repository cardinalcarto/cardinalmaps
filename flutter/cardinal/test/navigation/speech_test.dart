import 'dart:async';

import 'package:cardinal/navigation/speech.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_tts/flutter_tts.dart';

/// `flutter_tts`'s channel and the app's `cardinal/navigation`, both faked: what's asked
/// of each, in one log, and engine events sent back as the engine would.
class Harness {
  Harness() {
    messenger.setMockMethodCallHandler(_tts, (call) async {
      log.add('tts ${call.method}');
      if (call.method == 'stop' && holdStops) {
        final stopped = Completer<Object?>();
        pendingStops.add(stopped);
        return stopped.future;
      }
      return 1;
    });
    messenger.setMockMethodCallHandler(AndroidSpeechFocus.channel, (
      call,
    ) async {
      log.add(call.method);
      return call.method == 'requestSpeechFocus' ? true : null;
    });
  }

  static const _tts = MethodChannel('flutter_tts');

  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  final log = <String>[];
  final tts = FlutterTts();
  late final speaker = PlatformSpeaker(
    tts: tts,
    focus: const AndroidSpeechFocus(),
  );

  /// Whether `stop` waits for [pendingStops] to be completed, as one still on its way to
  /// the engine.
  bool holdStops = false;
  final pendingStops = <Completer<Object?>>[];

  void engine(String event) =>
      unawaited(tts.platformCallHandler(MethodCall('speak.$event')));

  /// Has the engine say [count] announcements, one after another: each starts, then
  /// completes.
  void sayAll(FakeAsync async, int count) {
    for (var i = 0; i < count; i++) {
      engine('onStart');
      async.flushMicrotasks();
      engine('onComplete');
      async.flushMicrotasks();
    }
  }

  int count(String method) => log.where((e) => e == method).length;

  List<String> get focus => [
    for (final e in log)
      if (e.endsWith('SpeechFocus')) e,
  ];

  void dispose() {
    messenger.setMockMethodCallHandler(_tts, null);
    messenger.setMockMethodCallHandler(AndroidSpeechFocus.channel, null);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late Harness h;
  setUp(() => h = Harness());
  tearDown(() => h.dispose());

  test(
    'a batch of announcements asks for focus once, and gives it back after the '
    'last',
    () {
      fakeAsync((async) {
        unawaited(h.speaker.speak('In 500 m, turn left'));
        unawaited(h.speaker.speak('Then turn right'));
        unawaited(h.speaker.speak('Then keep left'));
        async.flushMicrotasks();
        expect(h.focus, ['requestSpeechFocus']);
        // Asked for before the engine's told to speak.
        expect(h.log.first, 'requestSpeechFocus');
        expect(h.count('tts speak'), 3);

        h.sayAll(async, 2);
        expect(h.focus, ['requestSpeechFocus']);
        h.sayAll(async, 1);
        expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
      });
    },
  );

  test('an announcement queued while one is being said joins its batch', () {
    fakeAsync((async) {
      unawaited(h.speaker.speak('Turn left'));
      async.flushMicrotasks();
      h.engine('onStart');
      async.flushMicrotasks();
      unawaited(h.speaker.speak('Then turn right'));
      async.flushMicrotasks();
      h.engine('onComplete');
      async.flushMicrotasks();
      expect(h.focus, ['requestSpeechFocus']);
      h.sayAll(async, 1);
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
    });
  });

  test('stop gives focus back, and what it cut off says nothing more', () {
    fakeAsync((async) {
      unawaited(h.speaker.speak('Turn left'));
      unawaited(h.speaker.speak('Then turn right'));
      async.flushMicrotasks();
      h.engine('onStart');
      async.flushMicrotasks();
      unawaited(h.speaker.stop());
      async.flushMicrotasks();
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
      h.engine('onCancel');
      h.engine('onComplete');
      async.flushMicrotasks();
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
      // The next batch asks again.
      unawaited(h.speaker.speak('Turn left'));
      async.flushMicrotasks();
      expect(h.count('requestSpeechFocus'), 2);
    });
  });

  test('stop with nothing said asks for nothing and gives nothing back', () {
    fakeAsync((async) {
      unawaited(h.speaker.stop());
      async.flushMicrotasks();
      expect(h.focus, isEmpty);
    });
  });

  test(
    'a reroute holds focus through "Rerouting", and gives it back after',
    () {
      fakeAsync((async) {
        h.holdStops = true;
        unawaited(h.speaker.speak('In 2 km, take exit 12'));
        async.flushMicrotasks();
        h.engine('onStart');
        async.flushMicrotasks();

        // As the controller does on leaving the route: stop, then "Rerouting", unawaited.
        unawaited(h.speaker.stop());
        unawaited(h.speaker.speak('Rerouting'));
        async.flushMicrotasks();
        // What the engine says before the stop reaches it is about the cut-off one.
        h.engine('onComplete');
        async.flushMicrotasks();
        h.pendingStops.single.complete(1);
        async.flushMicrotasks();
        expect(h.focus, ['requestSpeechFocus']);

        // The cut-off one's cancel comes after, and changes nothing.
        h.engine('onCancel');
        async.flushMicrotasks();
        expect(h.focus, ['requestSpeechFocus']);

        h.sayAll(async, 1);
        expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
        expect(h.log.where((e) => !e.endsWith('SpeechFocus')), [
          'tts speak',
          'tts stop',
          'tts speak',
        ]);
      });
    },
  );

  test('a reroute with nothing being said asks for focus for "Rerouting"', () {
    fakeAsync((async) {
      unawaited(h.speaker.stop());
      unawaited(h.speaker.speak('Rerouting'));
      async.flushMicrotasks();
      expect(h.focus, ['requestSpeechFocus']);
      h.sayAll(async, 1);
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
    });
  });

  test('muting while speaking gives focus back', () {
    // Muting stops the speaker, as the controller's tests check.
    fakeAsync((async) {
      unawaited(h.speaker.speak('Turn left'));
      unawaited(h.speaker.speak('Then turn right'));
      async.flushMicrotasks();
      h.engine('onStart');
      async.flushMicrotasks();
      unawaited(h.speaker.stop());
      async.flushMicrotasks();
      h.engine('onCancel');
      async.flushMicrotasks();
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
      async.elapse(speechFocusTimeout * 2);
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
    });
  });

  test('an error counts as finishing', () {
    fakeAsync((async) {
      unawaited(h.speaker.speak('Turn left'));
      unawaited(h.speaker.speak('Then turn right'));
      async.flushMicrotasks();
      h.engine('onStart');
      async.flushMicrotasks();
      unawaited(
        h.tts.platformCallHandler(const MethodCall('speak.onError', 'boom')),
      );
      async.flushMicrotasks();
      expect(h.focus, ['requestSpeechFocus']);
      h.sayAll(async, 1);
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
    });
  });

  test('focus is given back with no word from the engine for '
      '$speechFocusTimeout', () {
    fakeAsync((async) {
      unawaited(h.speaker.speak('Turn left'));
      async.flushMicrotasks();
      h.engine('onStart');
      async.flushMicrotasks();
      async.elapse(speechFocusTimeout - const Duration(seconds: 1));
      expect(h.focus, ['requestSpeechFocus']);
      async.elapse(const Duration(seconds: 1));
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
      // Its completion, come late, gives back nothing more.
      h.engine('onComplete');
      async.flushMicrotasks();
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
    });
  });

  test('each event from the engine starts the timeout again', () {
    fakeAsync((async) {
      unawaited(h.speaker.speak('Turn left'));
      unawaited(h.speaker.speak('Then turn right'));
      async.flushMicrotasks();
      async.elapse(speechFocusTimeout - const Duration(seconds: 1));
      h.engine('onStart');
      async.flushMicrotasks();
      async.elapse(speechFocusTimeout - const Duration(seconds: 1));
      h.engine('onComplete');
      async.flushMicrotasks();
      async.elapse(speechFocusTimeout - const Duration(seconds: 1));
      expect(h.focus, ['requestSpeechFocus']);
      h.sayAll(async, 1);
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
    });
  });

  test('many queued announcements, over many batches, leave nothing held', () {
    fakeAsync((async) {
      for (var batch = 0; batch < 20; batch++) {
        for (var i = 0; i < 5; i++) {
          unawaited(h.speaker.speak('Announcement $batch.$i'));
        }
        async.flushMicrotasks();
        // Every other batch is cut off part way, as by a reroute.
        if (batch.isOdd) {
          h.sayAll(async, 2);
          h.engine('onStart');
          async.flushMicrotasks();
          unawaited(h.speaker.stop());
          async.flushMicrotasks();
          h.engine('onCancel');
          async.flushMicrotasks();
        } else {
          h.sayAll(async, 5);
        }
      }
      expect(h.count('requestSpeechFocus'), 20);
      expect(h.count('abandonSpeechFocus'), 20);
      // Strictly alternating: never asked for twice without giving back between.
      for (var i = 0; i < h.focus.length; i++) {
        expect(
          h.focus[i],
          i.isEven ? 'requestSpeechFocus' : 'abandonSpeechFocus',
        );
      }
      // And no timeout left to fire.
      async.elapse(speechFocusTimeout * 2);
      expect(h.count('abandonSpeechFocus'), 20);
    });
  });

  test('a completion with nothing started is from before a stop, and not '
      'counted', () {
    fakeAsync((async) {
      unawaited(h.speaker.speak('Turn left'));
      unawaited(h.speaker.speak('Then turn right'));
      async.flushMicrotasks();
      h.engine('onComplete');
      async.flushMicrotasks();
      h.sayAll(async, 1);
      expect(h.focus, ['requestSpeechFocus']);
      h.sayAll(async, 1);
      expect(h.focus, ['requestSpeechFocus', 'abandonSpeechFocus']);
    });
  });

  test('on iOS, nothing asks for focus', () {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    try {
      fakeAsync((async) {
        final speaker = PlatformSpeaker(tts: h.tts);
        unawaited(speaker.speak('Turn left'));
        unawaited(speaker.speak('Then turn right'));
        async.flushMicrotasks();
        h.sayAll(async, 1);
        unawaited(speaker.stop());
        async.flushMicrotasks();
        expect(h.focus, isEmpty);
        expect(h.count('tts speak'), 2);
      });
    } finally {
      debugDefaultTargetPlatformOverride = null;
    }
  });
}
