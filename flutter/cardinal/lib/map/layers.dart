import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:material_ui/material_ui.dart';

import '../config.dart';
import '../store/store.dart';
import 'location.dart';
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
final mapStyleChoice = Setting.choice('map.style', MapStyle.values, MapStyle.streets);

/// Cardinal Topo's contour units, its `units` global state.
final contourUnits = Setting.choice('map.contour_units', ContourUnits.values, ContourUnits.imperial);

/// Cardinal Topo's winter mode, its `winter` global state.
final winterMode = Setting.flag('map.winter', false);

/// What the layer picker chooses: the map's style, and Cardinal Topo's settings.
@immutable
class MapLayers {
  const MapLayers({this.style = MapStyle.streets, this.units = ContourUnits.imperial, this.winter = false});

  /// The stored choice.
  static Future<MapLayers> read(SettingsStore settings) async => MapLayers(
    style: await settings.get(mapStyleChoice),
    units: await settings.get(contourUnits),
    winter: await settings.get(winterMode),
  );

  final MapStyle style;
  final ContourUnits units;
  final bool winter;

  MapLayers copyWith({MapStyle? style, ContourUnits? units, bool? winter}) =>
      MapLayers(style: style ?? this.style, units: units ?? this.units, winter: winter ?? this.winter);

  @override
  bool operator ==(Object other) =>
      other is MapLayers && other.style == style && other.units == units && other.winter == winter;

  @override
  int get hashCode => Object.hash(style, units, winter);
}

/// The style JSON for [layers], from the bundled styles, with where you are and the
/// selected place on top.
Future<Map<String, Object?>> styleFor(MapLayers layers) async {
  final file = switch (layers.style) {
    MapStyle.streets => mapStyle,
    MapStyle.topo => topoStyle,
  };
  final style = jsonDecode(await rootBundle.loadString('assets/map/$file')) as Map<String, Object?>;
  setGlobalStateDefaults(style, {'units': layers.units.name, 'winter': layers.winter});
  addSelectionLayers(style);
  addLocationLayers(style);
  return style;
}

/// Sets the defaults of the `["global-state", name]` values [style] declares, leaving
/// alone any it doesn't.
void setGlobalStateDefaults(Map<String, Object?> style, Map<String, Object?> values) {
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
  Widget build(BuildContext context) => FloatingActionButton(
    heroTag: null,
    tooltip: 'Map layers',
    onPressed: () => showLayerPicker(context),
    child: const Icon(Icons.layers_outlined),
  );
}

/// Shows the layer picker, which stores each choice as it's made.
Future<void> showLayerPicker(BuildContext context) {
  final settings = StoreScope.of(context).settings;
  return showModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    builder: (context) => LayerPicker(settings: settings),
  );
}

/// The map's styles, one row each, with each style's settings on its row. The rows and
/// their settings stay put whichever is chosen, so nothing moves under a tap; choosing a
/// style's setting chooses the style too.
class LayerPicker extends StatefulWidget {
  const LayerPicker({super.key, required this.settings});

  final SettingsStore settings;

  @override
  State<LayerPicker> createState() => _LayerPickerState();
}

class _LayerPickerState extends State<LayerPicker> {
  MapLayers? _layers;

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
      if (old?.style != layers.style) settings.set(mapStyleChoice, layers.style),
      if (old?.units != layers.units) settings.set(contourUnits, layers.units),
      if (old?.winter != layers.winter) settings.set(winterMode, layers.winter),
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final layers = _layers;
    final scheme = Theme.of(context).colorScheme;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
        child: layers == null
            // As tall as the picker, so the sheet doesn't grow when the rows arrive.
            ? const SizedBox(height: _titleHeight + 2 * _StyleRow.height + 8)
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
                        child: Text('Map style', style: Theme.of(context).textTheme.titleMedium),
                      ),
                    ),
                  ),
                  _StyleRow(
                    icon: Icons.map_outlined,
                    label: 'Streets',
                    selected: layers.style == MapStyle.streets,
                    onTap: () => _choose(layers.copyWith(style: MapStyle.streets)),
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
                        style: const ButtonStyle(visualDensity: VisualDensity.compact),
                        segments: const [
                          ButtonSegment(value: ContourUnits.imperial, label: Text('ft'), tooltip: 'Contours in feet'),
                          ButtonSegment(value: ContourUnits.metric, label: Text('m'), tooltip: 'Contours in meters'),
                        ],
                        selected: {layers.units},
                        onSelectionChanged: (chosen) =>
                            _choose(layers.copyWith(style: MapStyle.topo, units: chosen.single)),
                      ),
                      const SizedBox(width: 4),
                      IconButton(
                        isSelected: layers.winter,
                        tooltip: 'Winter',
                        icon: const Icon(Icons.ac_unit),
                        style: IconButton.styleFrom(
                          backgroundColor: layers.winter ? scheme.secondaryContainer : null,
                        ),
                        onPressed: () => _choose(layers.copyWith(style: MapStyle.topo, winter: !layers.winter)),
                      ),
                    ],
                  ),
                ],
              ),
      ),
    );
  }
}

const _titleHeight = 36.0;

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
        color: selected ? scheme.primaryContainer.withValues(alpha: 0.4) : Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(16),
          side: BorderSide(color: selected ? scheme.primary : scheme.outlineVariant, width: selected ? 2 : 1),
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
                  Icon(icon, color: selected ? scheme.primary : scheme.onSurfaceVariant),
                  const SizedBox(width: 16),
                  Expanded(child: Text(label, style: Theme.of(context).textTheme.titleSmall)),
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
