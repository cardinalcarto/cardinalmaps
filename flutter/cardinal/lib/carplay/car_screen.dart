/// The map on a car's screen, under CarPlay's templates.
///
/// It runs in a Flutter engine of its own, started by `CarPlaySceneDelegate.swift` at
/// `carPlayMain` when a car connects: Flutter on iOS shows one view an engine, and the
/// phone's has the phone's screen. Nothing of the app is here but the map: no store,
/// no plugins and no navigation. What it draws comes from the phone's engine
/// (`car_link.dart`), by way of the app's CarPlay code, on `cardinal/carplay/screen`.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:sparkle/sparkle.dart';

import '../directions/route_source.dart' show TravelMode;
import '../map/layers.dart';
import '../navigation/navigation_camera.dart';
import 'car_keys.dart';
import 'car_map.dart';
import 'car_trip.dart';

/// Shows the car's map. Called by `carPlayMain`, in the car's own engine.
void runCarScreen() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const CarScreen());
}

/// The car's map: the app's driving style, light or dark as the car is, with the trip
/// the phone is navigating drawn on it ([CarMap]).
///
/// CarPlay draws its templates over it and takes every touch, so nothing here is
/// tapped, and nothing but the map is drawn, as Apple asks of a navigation app's base
/// view. The camera keeps the trip clear of what CarPlay says it covers (the view's
/// safe area).
class CarScreen extends StatefulWidget {
  const CarScreen({super.key});

  @override
  State<CarScreen> createState() => _CarScreenState();
}

class _CarScreenState extends State<CarScreen> {
  static const _channel = MethodChannel('cardinal/carplay/screen');

  final _map = SparkleMapController();
  late final _trip = CarMap(_map)..start();

  /// Sparkle's arguments, once the phone has said where the tiles come from.
  Future<List<String>>? _args;

  /// Where the map's files are kept, once known.
  String? _dir;

  /// What the style file was last written for, and its writes, one after another.
  MapLayers? _layers;
  Future<void> _writing = Future.value();

  bool _dark = false;
  TravelMode? _mode;

  @override
  void initState() {
    super.initState();
    _channel.setMethodCallHandler(_called);
    // Now that there's someone to hear it, the car's code sends what it has.
    unawaited(
      _channel.invokeMethod<void>('ready').catchError((Object e) {
        debugPrint('carplay: ready: $e');
      }),
    );
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _dark = MediaQuery.platformBrightnessOf(context) == Brightness.dark;
    _restyle();
    final covered = MediaQuery.paddingOf(context);
    _map.setPadding(covered);
    _trip.padding = followPadding(
      size: MediaQuery.sizeOf(context),
      mapPadding: covered,
      systemPadding: covered,
      direction: TextDirection.ltr,
    );
  }

  @override
  void dispose() {
    _channel.setMethodCallHandler(null);
    super.dispose();
  }

  Future<Object?> _called(MethodCall call) async {
    final arguments = call.arguments;
    switch (call.method) {
      case 'map':
        if (arguments is Map && _args == null && mounted) {
          setState(() => _args = _start(arguments));
        }
      case 'route':
        if (arguments is Map) {
          _trip.route(carRouteLine(arguments[CarKeys.line]));
        }
      case 'trip':
        final trip = arguments is Map ? arguments : null;
        _mode = TravelMode.values.asNameMap()[trip?[CarKeys.mode]];
        _restyle();
        _trip.trip(trip);
      case 'camera':
        if (arguments == 'overview') {
          _trip.overview();
        } else {
          _trip.recenter();
        }
    }
    return null;
  }

  /// The style for how the car looks now and how the trip travels: the simplified
  /// driving style on a drive or a ride, as on the phone.
  MapLayers get _wanted => MapLayers(dark: _dark).forNavigation(_mode);

  /// Writes the style again if what it's for changed; sparkle reloads the file.
  void _restyle() {
    final dir = _dir;
    final layers = _wanted;
    if (dir == null || _layers == null || layers == _layers) return;
    _layers = layers;
    _writing = _writing.then((_) => _writeStyle(dir, layers)).catchError((
      Object e,
    ) {
      debugPrint('carplay: style: $e');
    });
  }

  /// Sparkle's arguments for what the phone's `map` message says: the style and the
  /// bundled sprites, copied out to files of the car's own, beside the phone's, and
  /// the tiles the phone's map reads. The tile cache is the car's own too, and small:
  /// two maps in one app shouldn't share one.
  Future<List<String>> _start(Map<Object?, Object?> config) async {
    final tileJson = config[CarKeys.tileJson];
    final support = config[CarKeys.support];
    final cache = config[CarKeys.cache];
    if (tileJson is! String || tileJson.isEmpty) {
      throw StateError('no tiles for the car');
    }
    if (support is! String || cache is! String) {
      throw StateError('no folders for the car');
    }
    final dir = '$support/map/car';
    await Directory(dir).create(recursive: true);
    for (final name in const ['sprites@2x.json', 'sprites@2x.png']) {
      final data = await rootBundle.load('assets/map/$name');
      await File('$dir/$name').writeAsBytes(
        data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes),
        flush: true,
      );
    }
    final layers = _wanted;
    await _writeStyle(dir, layers);
    _dir = dir;
    _layers = layers;
    // The car's look or the trip may have changed while the files were written.
    _restyle();
    return [
      '--style',
      '$dir/style.json',
      '--sprite',
      '$dir/sprites',
      '--cache',
      '$cache/car-tiles',
      '--cache-size',
      '100',
      if (config[CarKeys.view] case final String view) ...['--view', view],
      '--tiles',
      'openmaptiles=$tileJson',
      if (config[CarKeys.basemap] case final String basemap) ...[
        '--local-tiles',
        'openmaptiles=$basemap',
      ],
    ];
  }

  @override
  Widget build(BuildContext context) {
    final args = _args;
    return Directionality(
      textDirection: TextDirection.ltr,
      child: ColoredBox(
        color: _dark ? const Color(0xFF1B1D21) : const Color(0xFFE9EBEE),
        child: args == null
            ? const SizedBox.expand()
            : FutureBuilder(
                future: args,
                builder: (context, snapshot) {
                  if (snapshot.hasError) {
                    debugPrint('carplay: map: ${snapshot.error}');
                  }
                  final data = snapshot.data;
                  if (data == null) return const SizedBox.expand();
                  return SparkleMap(
                    args: data,
                    controller: _map,
                    // The first frame drawn says the map is set up.
                    onStatus: (_) => _trip.ready(),
                    onError: (e) => debugPrint('carplay: map: $e'),
                  );
                },
              ),
      ),
    );
  }
}

/// Writes the style for [layers] into [dir], aside and renamed into place, so sparkle
/// never reads half a file.
Future<void> _writeStyle(String dir, MapLayers layers) async {
  final partial = File('$dir/style.json.partial');
  await partial.writeAsString(jsonEncode(await styleFor(layers)), flush: true);
  await partial.rename('$dir/style.json');
}
