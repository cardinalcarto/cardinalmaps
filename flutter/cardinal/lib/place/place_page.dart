import 'dart:async';

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../geocoder/address.dart';
import '../geocoder/geocoder.dart';
import '../map/map.dart';
import '../map/selection.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';

/// A place's card: its name and where it is, with its pin (and boundary) on the map.
///
/// Reads the place from the store by [id]; if it isn't there (a restored or linked
/// route), fetches it from the geocoder. The map flies to show it in what the sheet
/// leaves uncovered, following the sheet as it settles and moves.
class PlacePage extends StatefulWidget {
  const PlacePage({super.key, required this.id});

  final String id;

  @override
  State<PlacePage> createState() => _PlacePageState();
}

class _PlacePageState extends State<PlacePage> {
  static const _header = #placeHeader;

  Place? _place;
  bool _missing = false;
  bool _fetched = false;

  /// Whether the map has been sent to the place.
  bool _focused = false;
  StreamSubscription<Place?>? _watch;
  late SparkleMapController _map;

  /// This page's route animation: once it has come all the way in, the page is leaving
  /// whenever it's less than all the way in (closing, or a back gesture under way).
  Animation<double>? _route;
  bool _arrived = false;
  bool _leaving = false;

  /// Whether the map was keeping the place in view when the page started leaving, to
  /// keep it in view again if the page stays after all.
  bool _hadSubject = false;

  /// Whether the app is in dark mode, which has a pin of its own.
  bool _dark = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _map = MapScope.of(context);
    final dark = Theme.of(context).brightness == Brightness.dark;
    if (dark != _dark) {
      _dark = dark;
      if (_place != null) showSelection(_map, _place, dark: dark);
    }
    if (_watch == null) {
      final places = StoreScope.of(context).places;
      _place = places.cached(widget.id);
      _watch = places.watch(widget.id).listen(_changed);
      if (_place != null) _show();
    }
    final route = ModalRoute.of(context)?.animation;
    if (route != _route) {
      _route?.removeListener(_routeMoved);
      // Only from its changes: before its transition starts, it may say all the way in.
      _route = route?..addListener(_routeMoved);
    }
  }

  @override
  void dispose() {
    _watch?.cancel();
    _route?.removeListener(_routeMoved);
    clearSelection(_map, widget.id);
    super.dispose();
  }

  void _changed(Place? place) {
    if (place == null) {
      _fetch();
      return;
    }
    if (place == _place && !_missing) return;
    setState(() {
      _place = place;
      _missing = false;
    });
    _show();
  }

  /// Fetch the place from the geocoder, once, into the store.
  Future<void> _fetch() async {
    if (_fetched) return;
    _fetched = true;
    final places = StoreScope.of(context).places;
    try {
      final place = await GeocoderScope.of(context).place(widget.id);
      if (place != null) {
        await places.put(place);
        return;
      }
    } on Object catch (e) {
      debugPrint('fetching ${widget.id}: $e');
    }
    if (mounted && _place == null) setState(() => _missing = true);
  }

  /// Show the place on the map, and the first time, send the map to it. The map keeps
  /// it in what the sheet leaves uncovered from then on, as the sheet settles here and
  /// moves, until it's moved by hand.
  void _show() {
    final place = _place;
    showSelection(_map, place, dark: _dark);
    if (place == null || _focused) return;
    _focused = true;
    focusPlace(_map, place);
  }

  /// As the page leaves, let the camera go, so the sheet growing back to the page
  /// below's height doesn't zoom the map out to keep the place in view. If it comes
  /// back (a back gesture let go of), keep the place in view again, unless the map had
  /// been moved by hand.
  void _routeMoved() {
    final shown = (_route?.value ?? 1) >= 1;
    if (!_arrived) {
      _arrived = shown;
    } else if (!shown && !_leaving) {
      _leaving = true;
      _hadSubject = _map.hasSubject;
      _map.clearSubject();
    } else if (shown && _leaving) {
      _leaving = false;
      final place = _place;
      if (_hadSubject && place != null) focusPlace(_map, place);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final place = _place;
    final subtitle = place == null ? null : placeSubtitle(place);
    return SheetContent(
      anchors: const [SheetAnchor.section(_header), SheetAnchor.fraction(0.5)],
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _header,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 4, 8, 16),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          place != null
                              ? placeTitle(place)
                              : _missing
                              ? 'Place not found'
                              : '',
                          style: theme.textTheme.headlineSmall,
                        ),
                        if (subtitle != null)
                          Padding(
                            padding: const EdgeInsets.only(top: 4),
                            child: Text(
                              subtitle,
                              style: theme.textTheme.bodyMedium?.copyWith(
                                color: theme.colorScheme.onSurfaceVariant,
                              ),
                            ),
                          ),
                        if (place == null && !_missing)
                          const Padding(
                            padding: EdgeInsets.only(top: 8),
                            child: LinearProgressIndicator(),
                          ),
                      ],
                    ),
                  ),
                  IconButton(
                    tooltip: 'Close',
                    icon: const Icon(Icons.close),
                    onPressed: () =>
                        context.canPop() ? context.pop() : context.go('/'),
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}
