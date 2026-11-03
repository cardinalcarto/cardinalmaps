import 'dart:convert';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';
import 'package:path_provider/path_provider.dart';
import 'package:sparkle/sparkle.dart';

import '../config.dart';
import '../sheet/sheet.dart';
import 'selection.dart';

/// Provides the map's controller to the widgets below it.
class MapScope extends InheritedWidget {
  const MapScope({super.key, required this.controller, required super.child});

  final SparkleMapController controller;

  static SparkleMapController of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<MapScope>()!.controller;

  @override
  bool updateShouldNotify(MapScope oldWidget) => controller != oldWidget.controller;
}

/// The map: [tilesUrl]'s tiles in the Headway style, with the selected place on top
/// (see `selection.dart`). The body of a [SheetScaffold]: the camera keeps what it shows
/// clear of the sheet.
class CardinalMap extends StatefulWidget {
  const CardinalMap({super.key, required this.controller});

  final SparkleMapController controller;

  @override
  State<CardinalMap> createState() => _CardinalMapState();
}

class _CardinalMapState extends State<CardinalMap> {
  late final Future<List<String>> _args = _mapArgs();
  Object? _error;
  SheetController? _sheet;

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
    _sheet?.removeListener(_updatePadding);
    super.dispose();
  }

  /// The map's padding follows what the sheet and the status bar cover, so the camera
  /// centers things in what's left of the map, though it's drawn behind them too.
  void _updatePadding() {
    if (!mounted) return;
    final covered = SheetScaffold.viewOf(context).obscured;
    widget.controller.setPadding(EdgeInsets.only(top: MediaQuery.paddingOf(context).top) + covered);
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
                child: Text('The map could not be shown: $error', textAlign: TextAlign.center),
              ),
            );
          }
          final args = snapshot.data;
          if (args == null) return const SizedBox.expand();
          return SparkleMap(
            args: args,
            controller: widget.controller,
            onError: (e) => setState(() => _error = e),
          );
        },
      ),
    );
  }
}

/// The bundled map assets, which sparkle reads from files.
const _assets = ['sprites@2x.json', 'sprites@2x.png'];

/// Arguments for sparkle (as `sparkle_harness` takes them): the bundled style and
/// sprites, copied out to files, and [tilesUrl]'s tiles.
Future<List<String>> _mapArgs() async {
  if (tilesUrl.isEmpty) {
    throw StateError('no tiles: build with --dart-define=CARDINAL_TILES_URL=<TileJSON URL>');
  }
  final support = await getApplicationSupportDirectory();
  final dir = Directory('${support.path}/map');
  await dir.create(recursive: true);
  for (final name in _assets) {
    final data = await rootBundle.load('assets/map/$name');
    await File('${dir.path}/$name')
        .writeAsBytes(data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes), flush: true);
  }
  final style =
      jsonDecode(await rootBundle.loadString('assets/map/headway.json')) as Map<String, Object?>;
  addSelectionLayers(style);
  await File('${dir.path}/style.json').writeAsString(jsonEncode(style), flush: true);
  final cache = await getApplicationCacheDirectory();
  final d = dir.path;
  return [
    '--style',
    '$d/style.json',
    '--sprite',
    '$d/sprites',
    '--cache',
    '${cache.path}/tiles',
    if (initialView.isNotEmpty) ...['--view', initialView],
    '--tiles',
    'openmaptiles=$tilesUrl',
  ];
}
