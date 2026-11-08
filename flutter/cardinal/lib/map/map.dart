import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:path_provider/path_provider.dart';
import 'package:sparkle/sparkle.dart';

import '../config.dart';
import '../routes.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import 'layers.dart';
import 'poi.dart';
import 'saved_layer.dart';
import 'viewport.dart';

/// Provides the map's controller to the widgets below it.
class MapScope extends InheritedWidget {
  const MapScope({super.key, required this.controller, required super.child});

  final SparkleMapController controller;

  static SparkleMapController of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<MapScope>()!.controller;

  @override
  bool updateShouldNotify(MapScope oldWidget) =>
      controller != oldWidget.controller;
}

/// The map: [tilesUrl]'s tiles in the style the layer picker chose (see `layers.dart`),
/// with the selected place on top (see `selection.dart`). The body of a [SheetScaffold]:
/// the camera keeps what it shows clear of the sheet.
class CardinalMap extends StatefulWidget {
  const CardinalMap({super.key, required this.controller});

  final SparkleMapController controller;

  @override
  State<CardinalMap> createState() => _CardinalMapState();
}

class _CardinalMapState extends State<CardinalMap> {
  late final Future<List<String>> _args = _start(
    StoreScope.of(context).settings,
  );
  late final Future<Map<int, WofArea>> _wofAreas = _loadWofAreas();
  Object? _error;
  SheetController? _sheet;

  /// What the style file was last written for.
  MapLayers? _layers;
  final _watching = <StreamSubscription<void>>[];

  /// The style file's writes, one after another.
  Future<void> _writing = Future.value();

  SettingsStore? _settings;
  SavedMapLayer? _savedLayer;
  late final AppLifecycleListener _lifecycle;
  Timer? _viewSaver;
  MapViewport? _savedView;

  /// How often where the map is looking is kept, if it's moved: leaving with the back
  /// button can end the app before a save on the way out finishes.
  static const _viewSaveInterval = Duration(seconds: 2);

  @override
  void initState() {
    super.initState();
    // As the app goes away too, whichever comes first: it may not come back from any.
    _lifecycle = AppLifecycleListener(
      onHide: _saveView,
      onPause: _saveView,
      onDetach: _saveView,
    );
    _viewSaver = Timer.periodic(_viewSaveInterval, (_) => _saveView());
  }

  /// Keeps where the map is looking, to open there next time, if it's moved.
  void _saveView() {
    final settings = _settings;
    final view = MapViewport.of(widget.controller);
    if (settings == null || view == null || view == _savedView) return;
    _savedView = view;
    settings.set(savedViewport, view);
  }

  /// Starts the map in the stored layers, then follows the layer picker's choices.
  Future<List<String>> _start(SettingsStore settings) async {
    _settings = settings;
    _savedLayer = SavedMapLayer(
      store: StoreScope.of(context).saved,
      map: widget.controller,
      painter: discMarkerPainter(
        WidgetsBinding.instance.platformDispatcher.views.first.devicePixelRatio,
      ),
    )..start();
    final layers = await MapLayers.read(settings);
    final args = await _mapArgs(
      layers,
      openingView(await settings.get(savedViewport)),
    );
    _layers = layers;
    _watching.addAll([
      settings
          .watch(mapStyleChoice)
          .listen((v) => _change((l) => l.copyWith(style: v))),
      settings
          .watch(contourUnits)
          .listen((v) => _change((l) => l.copyWith(units: v))),
      settings
          .watch(winterMode)
          .listen((v) => _change((l) => l.copyWith(winter: v))),
    ]);
    return args;
  }

  /// Writes the style for a changed choice; sparkle reloads the file when it changes,
  /// keeping what it can.
  void _change(MapLayers Function(MapLayers) update) {
    final current = _layers;
    if (current == null) return;
    final next = update(current);
    if (next == current) return;
    _layers = next;
    _writing = _writing.then((_) => _writeStyle(next)).catchError((Object e) {
      if (mounted) setState(() => _error = e);
    });
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final sheet = SheetScaffold.controllerOf(context);
    if (sheet != _sheet) {
      _sheet?.removeListener(_updatePadding);
      _sheet = sheet..addListener(_updatePadding);
    }
    // The sheet's layout or the system bars may have changed.
    _updatePadding();
  }

  @override
  void dispose() {
    _saveView();
    _savedLayer?.dispose();
    _viewSaver?.cancel();
    _lifecycle.dispose();
    _sheet?.removeListener(_updatePadding);
    for (final watching in _watching) {
      watching.cancel();
    }
    super.dispose();
  }

  /// The map's padding follows what the sheet and the status bar cover, so the camera
  /// centers things in what's left of the map, though it's drawn behind them too.
  void _updatePadding() {
    if (!mounted) return;
    final covered = SheetScaffold.viewOf(context).obscured;
    widget.controller.setPadding(
      EdgeInsets.only(top: MediaQuery.paddingOf(context).top) + covered,
    );
  }

  /// Opens the card of what [features] has first: a saved place, or else a POI, as a
  /// search result does, storing what the map knows about it and going to its ID.
  Future<void> _openPoi(List<ClickedFeature> features) async {
    final saved = features.where((f) => f.source == savedSource).firstOrNull;
    if (saved?.properties[savedPlaceProperty] case final id?) {
      openPlaceFromMap(GoRouter.of(context), id);
      return;
    }
    final poi = features.where((f) => f.sourceLayer == poiLayer).firstOrNull;
    if (poi == null) return;
    // Now, while the tile is sure to be loaded: it names the smaller areas.
    final inTile = <int, WofArea>{};
    for (final MapEntry(key: placetype, value: id) in poiAdmins(
      poi.properties,
    ).entries) {
      if (namedInApp.contains(placetype)) continue;
      final names = widget.controller.featureBeside(
        poi,
        adminsLayer,
        id,
      )?[adminNames];
      if (names != null) {
        inTile[id] = WofArea(jsonDecode(names) as Map<String, Object?>);
      }
    }
    final inApp = await _wofAreas;
    if (!mounted) return;
    final place = placeFromPoi(
      poi,
      lang: PlatformDispatcher.instance.locale.languageCode,
      area: (placetype, id) =>
          namedInApp.contains(placetype) ? inApp[id] : inTile[id],
    );
    await StoreScope.of(context).places.put(place);
    if (!mounted) return;
    openPlaceFromMap(GoRouter.of(context), place.id);
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return ColoredBox(
      color: scheme.surfaceDim,
      child: FutureBuilder(
        future: _args,
        builder: (context, snapshot) {
          final error = _error ?? snapshot.error;
          if (error != null) {
            return Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Text(
                  'The map could not be shown: $error',
                  textAlign: TextAlign.center,
                ),
              ),
            );
          }
          final args = snapshot.data;
          if (args == null) return const SizedBox.expand();
          return SparkleMap(
            args: args,
            controller: widget.controller,
            onError: (e) => setState(() => _error = e),
            onFeaturesClicked: _openPoi,
          );
        },
      ),
    );
  }
}

/// The names of the large areas that tiles leave out, from `tool/wof_areas.py`.
Future<Map<int, WofArea>> _loadWofAreas() async {
  // Gzipped, but not named `.gz`: Android's packaging would take that off its name.
  final data = await rootBundle.load('assets/map/wof_areas.jsongz');
  return compute(
    _decodeWofAreas,
    data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes),
  );
}

Map<int, WofArea> _decodeWofAreas(Uint8List compressed) {
  final areas =
      jsonDecode(utf8.decode(gzip.decode(compressed))) as Map<String, Object?>;
  return {
    for (final MapEntry(:key, :value) in areas.entries)
      int.parse(key): WofArea(value as Map<String, Object?>),
  };
}

/// The bundled map assets, which sparkle reads from files.
const _assets = ['sprites@2x.json', 'sprites@2x.png'];

/// Where sparkle reads the map's files from.
Future<Directory> _mapDir() async =>
    Directory('${(await getApplicationSupportDirectory()).path}/map');

/// Writes the style for [layers] where sparkle reads it. Written aside and renamed into
/// place, so sparkle never reads half a file.
Future<void> _writeStyle(MapLayers layers) async {
  final dir = await _mapDir();
  final partial = File('${dir.path}/style.json.partial');
  await partial.writeAsString(jsonEncode(await styleFor(layers)), flush: true);
  await partial.rename('${dir.path}/style.json');
}

/// Arguments for sparkle (as `sparkle_harness` takes them): the style for [layers] and
/// the bundled sprites, copied out to files, [tilesUrl]'s tiles, and the [view] to open.
Future<List<String>> _mapArgs(MapLayers layers, MapViewport view) async {
  if (tilesUrl.isEmpty) {
    throw StateError(
      'no tiles: build with --dart-define=CARDINAL_TILES_URL=<TileJSON URL>',
    );
  }
  final dir = await _mapDir();
  await dir.create(recursive: true);
  for (final name in _assets) {
    final data = await rootBundle.load('assets/map/$name');
    await File('${dir.path}/$name').writeAsBytes(
      data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes),
      flush: true,
    );
  }
  await _writeStyle(layers);
  final cache = await getApplicationCacheDirectory();
  final d = dir.path;
  return [
    '--style',
    '$d/style.json',
    '--sprite',
    '$d/sprites',
    '--cache',
    '${cache.path}/tiles',
    '--view',
    view.arg,
    '--tiles',
    'openmaptiles=$tilesUrl',
  ];
}
