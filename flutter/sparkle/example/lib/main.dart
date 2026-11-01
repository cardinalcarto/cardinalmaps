// The sparkle harness in Flutter: a map of a tile server's tiles or local MBTiles and
// GeoJSON files, set up from the same command line as `sparkle_harness` (see
// `crates/sparkle_view/src/args.rs`), and drawn with Flutter GPU.
//
//   flutter run -d linux -a https://tiles.example.org/tileset -a --view -a -122.335,47.608,14
//
// or, once built, `build/linux/x64/release/bundle/sparkle_example seattle.mbtiles ...`.
// With `--screenshot out.png` it saves the first frame in which every visible tile has
// loaded, then exits.
//
// Without arguments (always the case on Android) it shows the tileset whose TileJSON URL
// it was built with, in the Headway style with the bundled terrain:
//
//   flutter run --dart-define=SPARKLE_TILES_URL=https://tiles.example.org/tileset \
//       [--dart-define=SPARKLE_VIEW=lng,lat,zoom]
//
// as if started with
//
//   sparkle_example --style headway.json --sprite sprites --cache <temp>/cache \
//       --tiles dem=dem-terrarium.mbtiles --tiles openmaptiles=$SPARKLE_TILES_URL \
//       [--view $SPARKLE_VIEW]
//
// Without SPARKLE_VIEW it starts at the tileset's center.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:sparkle/sparkle.dart';

Future<void> main(List<String> args) async {
  WidgetsFlutterBinding.ensureInitialized();
  await initSparkle();
  runApp(HarnessApp(args: args.isEmpty ? await _bundledArgs() : args));
}

/// The TileJSON URL of the tiles shown without arguments.
const _tilesUrl = String.fromEnvironment('SPARKLE_TILES_URL');

/// Where the view starts without arguments, as `--view` takes it.
const _view = String.fromEnvironment('SPARKLE_VIEW');

const _bundled = [
  'dem-terrarium.mbtiles',
  'headway.json',
  'sprites@2x.json',
  'sprites@2x.png',
];

/// Arguments for [_tilesUrl] and the bundled assets, copied out to files since sparkle
/// reads paths (and SQLite can't open an MBTiles file inside the APK).
Future<List<String>> _bundledArgs() async {
  if (_tilesUrl.isEmpty) {
    throw StateError(
      'no tiles: pass arguments, or build with '
      '--dart-define=SPARKLE_TILES_URL=<TileJSON URL>',
    );
  }
  final dir = Directory('${Directory.systemTemp.path}/sparkle_example');
  await dir.create(recursive: true);
  for (final name in _bundled) {
    final data = await rootBundle.load('assets/$name');
    await File('${dir.path}/$name').writeAsBytes(
      data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes),
      flush: true,
    );
  }
  final d = dir.path;
  return [
    '--style', '$d/headway.json',
    '--sprite', '$d/sprites',
    '--cache', '$d/cache',
    '--tiles', 'dem=$d/dem-terrarium.mbtiles',
    if (_view.isNotEmpty) ...['--view', _view],
    '--tiles', 'openmaptiles=$_tilesUrl',
  ];
}

class HarnessApp extends StatefulWidget {
  const HarnessApp({super.key, required this.args});

  final List<String> args;

  @override
  State<HarnessApp> createState() => _HarnessAppState();
}

class _HarnessAppState extends State<HarnessApp> {
  /// What the harness shows in its window title. Only the overlay showing it rebuilds
  /// when it changes, which isn't on every frame as the map moves.
  final ValueNotifier<String?> _status = ValueNotifier(null);

  @override
  void dispose() {
    _status.dispose();
    super.dispose();
  }

  bool get _screenshot => widget.args.contains('--screenshot');

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'sparkle',
      debugShowCheckedModeBanner: false,
      home: CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.escape): () => exit(0),
        },
        child: Stack(
          children: [
            Positioned.fill(
              child: SparkleMap(
                args: widget.args,
                multisample: const bool.fromEnvironment("SPARKLE_NO_MSAA") ? false : true, // PERF-TEMP
                onStatus: (s) => _status.value =
                    'zoom ${s.zoom.toStringAsFixed(2)}, '
                    'bearing ${s.bearing.toStringAsFixed(0)}°, '
                    'pitch ${s.pitch.toStringAsFixed(0)}° — '
                    '${s.tilesDrawn} tiles drawn, ${s.loading} loading',
                onScreenshot: (path) {
                  stdout.writeln('saved $path');
                  exit(0);
                },
                onError: (error) {
                  stderr.writeln('sparkle: $error');
                  if (_screenshot) {
                    exit(1);
                  }
                },
              ),
            ),
            // Not part of screenshots.
            if (!_screenshot)
              Positioned(
                left: 8,
                bottom: 8,
                child: RepaintBoundary(
                  child: ValueListenableBuilder(
                    valueListenable: _status,
                    builder: (context, status, _) => status == null
                        ? const SizedBox.shrink()
                        : DecoratedBox(
                            decoration: BoxDecoration(
                              color: Colors.black54,
                              borderRadius: BorderRadius.circular(4),
                            ),
                            child: Padding(
                              padding: const EdgeInsets.symmetric(
                                horizontal: 6,
                                vertical: 2,
                              ),
                              child: Text(
                                status,
                                style: const TextStyle(
                                  color: Colors.white,
                                  fontSize: 12,
                                ),
                              ),
                            ),
                          ),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}
