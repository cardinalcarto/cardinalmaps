// Renders widget-test screens to PNGs with real fonts, for reviewing how they look
// without a device. Used by the `*_screens.dart` files here, which aren't tests (they
// don't end in `_test.dart`, so the suite skips them) and are run by path:
//
//   timeout 45 flutter test --timeout 10s test/screenshots/offline_geocoder_screens.dart
//
// They write into `CARDINAL_SCREENSHOTS_DIR`, or the system's temporary directory.
import 'dart:io';

import 'package:material_ui/material_ui.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// Where screenshots go.
final screenshotsDir =
    Platform.environment['CARDINAL_SCREENSHOTS_DIR'] ??
    '${Directory.systemTemp.path}/cardinal-screenshots';

String get _flutterRoot =>
    Platform.environment['FLUTTER_ROOT'] ??
    '${Platform.environment['HOME']}/src/flutter';

/// Loads Roboto, Material's icons, Cupertino's icons and the app's own icon font, so
/// text renders as on Android rather than as the test font's boxes.
Future<void> loadRenderFonts() async {
  Future<void> load(String family, List<String> paths) async {
    final loader = FontLoader(family);
    for (final path in paths) {
      loader.addFont(
        Future.value(ByteData.sublistView(File(path).readAsBytesSync())),
      );
    }
    await loader.load();
  }

  final fonts = '$_flutterRoot/bin/cache/artifacts/material_fonts';
  final roboto = [
    for (final weight in ['Light', 'Regular', 'Medium', 'Bold'])
      '$fonts/Roboto-$weight.ttf',
  ];
  await load('Roboto', roboto);
  await load('MaterialIcons', ['$fonts/MaterialIcons-Regular.otf']);
  final pubCache =
      Platform.environment['PUB_CACHE'] ??
      '${Platform.environment['HOME']}/.pub-cache';
  final cupertino = Directory('$pubCache/hosted/pub.dev')
      .listSync()
      .whereType<Directory>()
      .where((d) => d.path.contains('cupertino_icons-'))
      .map((d) => '${d.path}/assets/CupertinoIcons.ttf')
      .where((p) => File(p).existsSync())
      .firstOrNull;
  if (cupertino != null) {
    await load('packages/cupertino_icons/CupertinoIcons', [cupertino]);
  }
  await load('CardinalIcons', ['assets/fonts/CardinalIcons.ttf']);
}

/// Writes every "golden" it's given instead of comparing it: how [screenshot] saves.
class _Writer extends GoldenFileComparator {
  @override
  Future<bool> compare(Uint8List imageBytes, Uri golden) async {
    await update(golden, imageBytes);
    return true;
  }

  @override
  Future<void> update(Uri golden, Uint8List imageBytes) async {
    final file = File.fromUri(golden);
    await file.parent.create(recursive: true);
    await file.writeAsBytes(imageBytes);
    // ignore: avoid_print
    print('wrote ${file.path}');
  }
}

/// Writes what the test's app shows to `<screenshotsDir>/<name>.png`.
Future<void> screenshot(WidgetTester tester, String name) async {
  await tester.pumpAndSettle();
  goldenFileComparator = _Writer();
  await expectLater(
    find.byType(MaterialApp).first,
    matchesGoldenFile(Uri.file('$screenshotsDir/$name.png')),
  );
}
