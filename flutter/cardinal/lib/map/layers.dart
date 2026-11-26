import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoIcons, CupertinoListTileChevron;
import 'package:material_ui/material_ui.dart';

import '../config.dart';
import '../directions/route_source.dart';
import '../offline/drawing/area_layer.dart';
import '../store/store.dart';
import '../saved/saved_layers_sheet.dart';
import '../ui/adaptive.dart';
import '../ui/symbols.dart';
import 'location.dart';
import 'map_control.dart';
import 'route_layer.dart';
import 'saved_layer.dart';
import 'selection.dart';

/// The map's style.
enum MapStyle {
  /// The everyday map of streets and places, [mapStyle].
  streets,

  /// Cardinal Topo: contours and backcountry features, for hiking.
  topo,
}

/// The units of Cardinal Topo's contour lines.
enum ContourUnits { imperial, metric }

/// The map's style.
final mapStyleChoice = Setting.choice(
  'map.style',
  MapStyle.values,
  MapStyle.streets,
);

/// Cardinal Topo's contour units, its `units` global state.
final contourUnits = Setting.choice(
  'map.contour_units',
  ContourUnits.values,
  ContourUnits.imperial,
);

/// What the layer picker chooses: the map's style, and Cardinal Topo's settings; and
/// whether the app is dark and the map is showing motor or bicycle navigation.
@immutable
class MapLayers {
  const MapLayers({
    this.style = MapStyle.streets,
    this.units = ContourUnits.imperial,
    this.dark = false,
    this.navigationStyle = false,
  });

  /// The stored choice, for an app that's [dark] or not.
  static Future<MapLayers> read(
    SettingsStore settings, {
    bool dark = false,
  }) async => MapLayers(
    style: await settings.get(mapStyleChoice),
    units: await settings.get(contourUnits),
    dark: dark,
  );

  final MapStyle style;
  final ContourUnits units;

  /// Whether the app is in dark mode, which each style has a version for.
  final bool dark;

  /// The temporary, simplified basemap for driving and cycling. Not a saved choice.
  final bool navigationStyle;

  /// Navigation overrides the basemap without losing the layer picker's choice.
  /// Walking (including wheelchair navigation) and idle keep that choice.
  MapLayers forNavigation(TravelMode? mode) => copyWith(
    navigationStyle: mode == TravelMode.driving || mode == TravelMode.cycling,
  );

  MapLayers copyWith({
    MapStyle? style,
    ContourUnits? units,
    bool? dark,
    bool? navigationStyle,
  }) => MapLayers(
    style: style ?? this.style,
    units: units ?? this.units,
    dark: dark ?? this.dark,
    navigationStyle: navigationStyle ?? this.navigationStyle,
  );

  @override
  bool operator ==(Object other) =>
      other is MapLayers &&
      other.style == style &&
      other.units == units &&
      other.dark == dark &&
      other.navigationStyle == navigationStyle;

  @override
  int get hashCode => Object.hash(style, units, dark, navigationStyle);
}

/// The style JSON for [layers], from the bundled styles, with where you are and the
/// selected place on top.
Future<Map<String, Object?>> styleFor(MapLayers layers) async {
  final file = layers.navigationStyle
      ? (layers.dark ? darkDrivingStyle : drivingStyle)
      : switch (layers.style) {
          MapStyle.streets => layers.dark ? darkMapStyle : mapStyle,
          MapStyle.topo => layers.dark ? darkTopoStyle : topoStyle,
        };
  final style = jsonDecode(
    await rootBundle.loadString('assets/map/$file'),
  ) as Map<String, Object?>;
  setGlobalStateDefaults(style, {'units': layers.units.name});
  addSavedLayers(style, dark: layers.dark);
  addAreaLayers(style);
  addRouteLayers(style);
  addSelectionLayers(style);
  addLocationLayers(style);
  return style;
}

/// Sets the defaults of the `["global-state", name]` values [style] declares, leaving
/// alone any it doesn't.
void setGlobalStateDefaults(
  Map<String, Object?> style,
  Map<String, Object?> values,
) {
  final state = style['state'];
  if (state is! Map<String, Object?>) return;
  for (final MapEntry(:key, :value) in values.entries) {
    final declared = state[key];
    if (declared is Map<String, Object?>) declared['default'] = value;
  }
}

/// The button that opens the layer picker; see `MapButtons`.
class LayerButton extends StatelessWidget {
  const LayerButton({super.key});

  @override
  Widget build(BuildContext context) => MapControl(
    tooltip: 'Map layers',
    icon: AppSymbols.of(context).layers,
    onPressed: () => showLayerPicker(context),
  );
}

/// Shows the layer picker, which stores each choice as it's made.
Future<void> showLayerPicker(BuildContext context) {
  final store = StoreScope.of(context);
  return showModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    // iOS's grouped sheet grows to its rows.
    isScrollControlled: isCupertinoOf(context),
    builder: (context) =>
        LayerPicker(settings: store.settings, saved: store.saved),
  );
}

/// The map's styles, one row each, with each style's settings on its row. The rows and
/// their settings stay put whichever is chosen, so nothing moves under a tap; choosing a
/// style's setting chooses the style too.
class LayerPicker extends StatefulWidget {
  const LayerPicker({super.key, required this.settings, required this.saved});

  final SettingsStore settings;
  final SavedStore saved;

  @override
  State<LayerPicker> createState() => _LayerPickerState();
}

class _LayerPickerState extends State<LayerPicker> {
  MapLayers? _layers;
  late final Stream<SavedTree> _savedTree = widget.saved.watch();

  @override
  void initState() {
    super.initState();
    MapLayers.read(widget.settings).then((layers) {
      if (mounted) setState(() => _layers = layers);
    });
  }

  /// Shows [layers] at once and stores what changed.
  Future<void> _choose(MapLayers layers) async {
    final old = _layers;
    setState(() => _layers = layers);
    final settings = widget.settings;
    await Future.wait([
      if (old?.style != layers.style)
        settings.set(mapStyleChoice, layers.style),
      if (old?.units != layers.units) settings.set(contourUnits, layers.units),
    ]);
  }

  /// iOS's version: grouped rows, a checkmark on the chosen style, the topo settings as
  /// rows of their own, and Done in the bar at the top.
  Widget _cupertino(BuildContext context, MapLayers layers) {
    final accent = Theme.of(context).colorScheme.primary;
    Widget? tick(bool on) =>
        on ? Icon(CupertinoIcons.checkmark, color: accent) : null;
    return SafeArea(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          AppSheetBar(title: 'Map', onDone: () => Navigator.of(context).pop()),
          AppListSection(
            header: 'Map style',
            children: [
              AppListTile(
                leading: Icon(CupertinoIcons.map, color: accent),
                title: const Text('Streets'),
                trailing: tick(layers.style == MapStyle.streets),
                onTap: () => _choose(layers.copyWith(style: MapStyle.streets)),
              ),
              AppListTile(
                leading: Icon(CupertinoIcons.tree, color: accent),
                title: const Text('Topo'),
                trailing: tick(layers.style == MapStyle.topo),
                onTap: () => _choose(layers.copyWith(style: MapStyle.topo)),
              ),
            ],
          ),
          AppListSection(
            header: 'Topo',
            footer: 'Changing these shows the topo map.',
            children: [
              AppListTile(
                title: const Text('Contours'),
                trailing: AppSegmentedControl<ContourUnits>(
                  segments: const {
                    ContourUnits.imperial: Text('ft'),
                    ContourUnits.metric: Text('m'),
                  },
                  selected: layers.units,
                  onChanged: (units) => _choose(
                    layers.copyWith(style: MapStyle.topo, units: units),
                  ),
                ),
              ),
            ],
          ),
          StreamBuilder(
            stream: _savedTree,
            builder: (context, snapshot) {
              final count = snapshot.data?.shownPlaces.length ?? 0;
              return AppListSection(
                header: 'Layers',
                children: [
                  AppListTile(
                    leading: Icon(CupertinoIcons.star, color: accent),
                    title: const Text('Saved places'),
                    trailing: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(
                          count == 0 ? 'Off' : '$count on',
                          style: TextStyle(
                            color: Theme.of(context)
                                .colorScheme
                                .onSurfaceVariant,
                          ),
                        ),
                        const SizedBox(width: 6),
                        const CupertinoListTileChevron(),
                      ],
                    ),
                    onTap: () => showSavedLayersSheet(context),
                  ),
                ],
              );
            },
          ),
          const SizedBox(height: 8),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final layers = _layers;
    if (layers != null && isCupertinoOf(context)) {
      return _cupertino(context, layers);
    }
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
        child: layers == null
            // As tall as the picker, so the sheet doesn't grow when the rows arrive.
            ? const SizedBox(
                height:
                    2 * _titleHeight + 3 * _StyleRow.height + 8 + _sectionGap,
              )
            : Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  SizedBox(
                    height: _titleHeight,
                    child: Padding(
                      padding: const EdgeInsetsDirectional.only(start: 8),
                      child: Align(
                        alignment: AlignmentDirectional.topStart,
                        child: Text(
                          'Map style',
                          style: Theme.of(context).textTheme.titleMedium,
                        ),
                      ),
                    ),
                  ),
                  _StyleRow(
                    icon: Icons.map_outlined,
                    label: 'Streets',
                    selected: layers.style == MapStyle.streets,
                    onTap: () =>
                        _choose(layers.copyWith(style: MapStyle.streets)),
                  ),
                  const SizedBox(height: 8),
                  _StyleRow(
                    icon: Icons.terrain_outlined,
                    label: 'Topo',
                    selected: layers.style == MapStyle.topo,
                    onTap: () => _choose(layers.copyWith(style: MapStyle.topo)),
                    settings: [
                      SegmentedButton<ContourUnits>(
                        showSelectedIcon: false,
                        style: const ButtonStyle(
                          visualDensity: VisualDensity.compact,
                        ),
                        segments: const [
                          ButtonSegment(
                            value: ContourUnits.imperial,
                            label: Text('ft'),
                            tooltip: 'Contours in feet',
                          ),
                          ButtonSegment(
                            value: ContourUnits.metric,
                            label: Text('m'),
                            tooltip: 'Contours in meters',
                          ),
                        ],
                        selected: {layers.units},
                        onSelectionChanged: (chosen) => _choose(
                          layers.copyWith(
                            style: MapStyle.topo,
                            units: chosen.single,
                          ),
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: _sectionGap),
                  SizedBox(
                    height: _titleHeight,
                    child: Padding(
                      padding: const EdgeInsetsDirectional.only(start: 8),
                      child: Align(
                        alignment: AlignmentDirectional.topStart,
                        child: Text(
                          'Layers',
                          style: Theme.of(context).textTheme.titleMedium,
                        ),
                      ),
                    ),
                  ),
                  StreamBuilder(
                    stream: _savedTree,
                    builder: (context, snapshot) {
                      final count = snapshot.data?.shownPlaces.length ?? 0;
                      return _StyleRow(
                        icon: Icons.star_border,
                        label: 'Saved places',
                        selected: false,
                        onTap: () => showSavedLayersSheet(context),
                        settings: [
                          Text(
                            count == 0 ? 'Off' : '$count on',
                            style: Theme.of(context).textTheme.bodyMedium,
                          ),
                          const Icon(Icons.chevron_right),
                        ],
                      );
                    },
                  ),
                ],
              ),
      ),
    );
  }
}

const _titleHeight = 36.0;
const _sectionGap = 12.0;

/// One of the picker's styles: outlined and tinted when it's chosen, with the style's
/// settings at its end.
class _StyleRow extends StatelessWidget {
  const _StyleRow({
    required this.icon,
    required this.label,
    required this.selected,
    required this.onTap,
    this.settings = const [],
  });

  /// Every row's height, settings or not, so the rows line up.
  static const height = 64.0;

  final IconData icon;
  final String label;
  final bool selected;
  final VoidCallback onTap;
  final List<Widget> settings;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Semantics(
      selected: selected,
      button: true,
      child: Material(
        color: selected
            ? scheme.primaryContainer.withValues(alpha: 0.4)
            : Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(16),
          side: BorderSide(
            color: selected ? scheme.primary : scheme.outlineVariant,
            width: selected ? 2 : 1,
          ),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: SizedBox(
            height: height,
            child: Padding(
              padding: const EdgeInsetsDirectional.only(start: 16, end: 8),
              child: Row(
                children: [
                  Icon(
                    icon,
                    color: selected ? scheme.primary : scheme.onSurfaceVariant,
                  ),
                  const SizedBox(width: 16),
                  Expanded(
                    child: Text(
                      label,
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                  ),
                  ...settings,
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
