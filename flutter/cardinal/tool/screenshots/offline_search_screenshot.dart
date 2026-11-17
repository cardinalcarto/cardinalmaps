// A screenshot of offline search's results in the real app on the Linux desktop, for
// review. A development tool, not a test: run it with `tool/offline_search_screenshot.sh`,
// which builds the app in profile mode for the desktop and drives it with `flutter drive`.
// It refuses to run anywhere else.
//
// It runs the app's real `main()` in a 412 × 915 view at a device pixel ratio of 2, over
// an offline folder (`CARDINAL_OFFLINE_ROOT`) holding a search index built on the
// desktop (`search/current.json`, see `geocoder/search_index.dart`), types
// `CARDINAL_SCREENSHOTS_QUERY` into the search bar, and saves the app's whole view as
// `<CARDINAL_SCREENSHOTS_DIR>/<CARDINAL_SCREENSHOTS_NAME>.png`. Typing is the test
// binding's, into the app; nothing is typed into the desktop. It changes nothing in the
// app's store, and fails if the store's setting sends search online.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/main.dart' as app;
import 'package:cardinal/map/map.dart';
import 'package:cardinal/store/store.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:material_ui/material_ui.dart';

const outDir = String.fromEnvironment('CARDINAL_SCREENSHOTS_DIR');
const query = String.fromEnvironment(
  'CARDINAL_SCREENSHOTS_QUERY',
  defaultValue: 'coffee',
);
const name = String.fromEnvironment(
  'CARDINAL_SCREENSHOTS_NAME',
  defaultValue: 'offline-search',
);

void main() {
  if (!Platform.isLinux || !kProfileMode) {
    stderr.writeln(
      'offline_search_screenshot: only for the Linux desktop in profile mode; '
      'run tool/offline_search_screenshot.sh',
    );
    exit(64);
  }
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'offline search screenshot',
    timeout: const Timeout(Duration(minutes: 5)),
    (tester) async {
      if (outDir.isEmpty) fail('needs CARDINAL_SCREENSHOTS_DIR');
      tester.view
        ..devicePixelRatio = 2
        ..physicalSize = const Size(412, 915) * 2
        ..padding = const FakeViewPadding(top: 24 * 2, bottom: 24 * 2)
        ..viewPadding = const FakeViewPadding(top: 24 * 2, bottom: 24 * 2);
      addTearDown(tester.view.reset);
      binding.platformDispatcher.platformBrightnessTestValue = Brightness.light;
      addTearDown(binding.platformDispatcher.clearPlatformBrightnessTestValue);

      app.main();
      await _settle(tester, const Duration(seconds: 3));
      final context = tester.element(find.byType(CardinalMap));
      final online = await StoreScope.of(context).settings.get(searchOnline);
      if (online) fail('the app\'s setting sends search online');

      // Typing goes through the test binding's text input, not the desktop's.
      tester.testTextInput.register();
      addTearDown(tester.testTextInput.unregister);
      await tester.tap(find.byType(SearchBar));
      await _settle(tester, const Duration(seconds: 1));
      await tester.enterText(find.byType(TextField), query);
      // The search, and the first time the index opens.
      await _settle(tester, const Duration(seconds: 4));

      final image = await _boundary(tester).toImage(pixelRatio: 2);
      final png = await image.toByteData(format: ui.ImageByteFormat.png);
      image.dispose();
      await Directory(outDir).create(recursive: true);
      await File('$outDir/$name.png').writeAsBytes(png!.buffer.asUint8List());
      debugPrint('offline-search-shot: $outDir/$name.png');
    },
  );
}

RenderRepaintBoundary _boundary(WidgetTester tester) {
  RenderRepaintBoundary? found;
  void visit(RenderObject object) {
    if (found != null) return;
    if (object is RenderRepaintBoundary) {
      found = object;
      return;
    }
    object.visitChildren(visit);
  }

  tester.binding.renderViews.first.visitChildren(visit);
  return found!;
}

/// Lets the app run for [time], drawing frames as it goes.
Future<void> _settle(WidgetTester tester, Duration time) async {
  final end = DateTime.now().add(time);
  while (DateTime.now().isBefore(end)) {
    await tester.pump(const Duration(milliseconds: 100));
  }
}
