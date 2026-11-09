import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show PlatformDispatcher;

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';
import 'package:sparkle/sparkle.dart';

import '../geocoder/address.dart';
import '../geocoder/geocoder.dart';
import '../map/location.dart';
import '../map/map.dart';
import '../map/route_layer.dart';
import '../map/selection.dart';
import '../saved/saved_feedback.dart';
import '../saved/saved_names.dart';
import '../search/place_search.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import 'callouts.dart';
import 'directions.dart';
import 'format.dart';
import 'route_source.dart';

/// Directions to the place with [to]: the trip's two ends, the chips for how you're
/// traveling, and the selected route's summary and steps, with the routes on the map.
///
/// It's showing the trip, or editing one of its ends:
///
/// | In | When | Then |
/// |---|---|---|
/// | showing the trip | a field is tapped | editing it: full, keyboard up, the field's text selected |
/// | editing | a place or My Location is chosen | that's the end; showing the trip, at the summary |
/// | editing | back | showing the trip, at the summary, the end as it was unless it was cleared |
/// | showing the trip | back | the place's card |
class DirectionsPage extends StatefulWidget {
  const DirectionsPage({super.key, required this.to});

  /// The ID of the place to go to.
  final String to;

  @override
  State<DirectionsPage> createState() => _DirectionsPageState();
}

class _DirectionsPageState extends State<DirectionsPage> {
  static const _modes = #directionsModes;
  static const _summary = #directionsSummary;

  /// Just below the summary. Flings always stop here, on the way up or down.
  static const _summaryAnchor = SheetAnchor.section(
    _summary,
    gravity: double.infinity,
  );

  /// Space kept around the routes, in logical pixels.
  static const _margin = 48.0;

  /// The closest the map moves in to show a short trip.
  static const _maxZoom = 17.0;

  /// How long the map has to be still for the callouts to move.
  static const _calloutDelay = Duration(milliseconds: 150);

  /// How far a callout's point stays from the edge of what's showing, in logical pixels:
  /// enough for the bubble, which sits above the point.
  static const _calloutMargin = EdgeInsets.fromLTRB(44, 40, 44, 4);

  Directions? _directions;
  late SparkleMapController _map;
  SheetController? _sheet;
  PlaceSearch? _search;
  SettingsStore? _settings;
  StreamSubscription<Place?>? _watchTo;
  StreamSubscription<SavedTree>? _watchSaved;
  StreamSubscription<int>? _taps;
  Timer? _calloutTimer;

  final _text = TextEditingController();
  final _focus = FocusNode(debugLabel: 'directions');

  /// The end being edited, if one is.
  RouteField? _editing;

  List<SavedPlace> _saved = const [];

  /// The routes the map shows, and what's worked out from them.
  List<DirectionsRoute> _shownRoutes = const [];
  CalloutPlacer? _placer;
  List<Callout> _callouts = const [];

  bool _dark = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _map = MapScope.of(context);
    final dark = Theme.of(context).brightness == Brightness.dark;
    final sheet = SheetScaffold.controllerOf(context);
    if (sheet != _sheet) {
      _sheet?.removeListener(_viewChanged);
      _sheet = sheet..addListener(_viewChanged);
    }
    if (_directions == null) {
      _start();
    } else if (dark != _dark) {
      _dark = dark;
      _drawRoutes();
    }
    _dark = dark;
  }

  void _start() {
    final scope = DirectionsScope.of(context);
    final store = StoreScope.of(context);
    _settings = store.settings;
    _search = PlaceSearch(
      GeocoderScope.of(context),
      lang: PlatformDispatcher.instance.locale.toLanguageTag(),
    );
    final directions = _directions = Directions(
      routes: scope.routes,
      location: scope.location,
    )..addListener(_changed);
    // The place's card under this page has it, or is fetching it.
    final cached = store.places.cached(widget.to);
    if (cached != null) {
      directions.setEndpoint(RouteField.to, RouteEndpoint.place(cached));
    } else {
      _watchTo = store.places.watch(widget.to).listen((place) {
        if (place == null) return;
        _watchTo?.cancel();
        _watchTo = null;
        directions.setEndpoint(RouteField.to, RouteEndpoint.place(place));
      });
    }
    _watchSaved = store.saved.watch().listen((tree) {
      setState(() => _saved = tree.allPlaces);
    });
    _taps = routeTaps.stream.listen(directions.select);
    _map.cameraMoved.addListener(_viewChanged);
    // This page's own pin marks the destination, which can change.
    showSelection(_map, null);
    unawaited(_resume(directions));
  }

  /// Starts with the mode last chosen, then from where you are.
  Future<void> _resume(Directions directions) async {
    directions.setMode(await _settings!.get(lastTravelMode));
    await directions.startFromMyLocation();
  }

  @override
  void dispose() {
    _watchTo?.cancel();
    _watchSaved?.cancel();
    _taps?.cancel();
    _calloutTimer?.cancel();
    _map.cameraMoved.removeListener(_viewChanged);
    _sheet?.removeListener(_viewChanged);
    _map
      ..setGeoJson(routeSource, empty)
      ..setGeoJson(routeCalloutSource, empty);
    _directions?.dispose();
    _search?.dispose();
    _focus.dispose();
    _text.dispose();
    super.dispose();
  }

  /// The trip changed: redraw it, and if the routes are new, show them all and place
  /// their callouts.
  void _changed() {
    if (!mounted) return;
    final directions = _directions!;
    if (!identical(directions.routes, _shownRoutes)) {
      _shownRoutes = directions.routes;
      _placer = _shownRoutes.isEmpty ? null : CalloutPlacer(_shownRoutes);
      _showAll();
    }
    _drawRoutes();
    _placeCallouts();
    setState(() {});
  }

  void _drawRoutes() {
    final directions = _directions!;
    _map.setGeoJson(
      routeSource,
      routesGeoJson(
        directions.routes,
        selected: directions.selected,
        destination: directions.to?.point,
        dark: _dark,
      ),
    );
  }

  /// Makes all of the routes the map's subject, kept in view as the sheet moves.
  void _showAll() {
    final points = _shownRoutes.expand((route) => route.geometry);
    if (points.isEmpty) return;
    var west = double.infinity, south = double.infinity;
    var east = -double.infinity, north = -double.infinity;
    for (final p in points) {
      west = math.min(west, p.lng);
      south = math.min(south, p.lat);
      east = math.max(east, p.lng);
      north = math.max(north, p.lat);
    }
    _map.setSubject(
      west: west,
      south: south,
      east: east,
      north: north,
      maxZoom: _maxZoom,
      margin: _margin,
    );
  }

  /// The map or the sheet moved: once they're still, the callouts follow.
  void _viewChanged() {
    _calloutTimer?.cancel();
    _calloutTimer = Timer(_calloutDelay, _placeCallouts);
  }

  /// Places each route's callout somewhere in what the sheet leaves uncovered.
  void _placeCallouts() {
    if (!mounted) return;
    final placer = _placer;
    final List<Callout> callouts;
    if (placer == null) {
      callouts = const [];
    } else {
      final outline = _visibleOutline();
      if (outline == null) return;
      callouts = placer.place((lng, lat) => insideOutline(outline, lng, lat));
    }
    if (_sameCallouts(callouts)) return;
    _callouts = callouts;
    _map.setGeoJson(
      routeCalloutSource,
      calloutsGeoJson(callouts, _shownRoutes),
    );
  }

  bool _sameCallouts(List<Callout> callouts) {
    if (callouts.length != _callouts.length) return false;
    for (var i = 0; i < callouts.length; i++) {
      if (callouts[i] != _callouts[i]) return false;
    }
    return true;
  }

  /// The ground showing outside the sheet and the status bar, or null if it can't be
  /// told: the sheet covers the map, or the view reaches past the horizon.
  List<({double lng, double lat})>? _visibleOutline() {
    final view = SheetScaffold.viewOf(context);
    final clear =
        EdgeInsets.only(top: MediaQuery.paddingOf(context).top) + view.obscured;
    final area = _calloutMargin.deflateRect(
      clear.deflateRect(Offset.zero & MediaQuery.sizeOf(context)),
    );
    if (area.isEmpty) return null;
    final corners = [
      area.topLeft,
      area.topRight,
      area.bottomRight,
      area.bottomLeft,
    ].map(_map.screenToLngLat).nonNulls.toList();
    return corners.length < 4 ? null : corners;
  }

  void _setMode(TravelMode mode) {
    _directions!.setMode(mode);
    unawaited(_settings!.set(lastTravelMode, mode));
  }

  /// A field's tapped: edit it, full, with its text ready to be typed over.
  void _edit(RouteField field) {
    final directions = _directions!;
    final current = directions.endpoint(field);
    final text = current == null || current.isMyLocation ? '' : current.label;
    _text.value = TextEditingValue(
      text: text,
      selection: TextSelection(baseOffset: 0, extentOffset: text.length),
    );
    _search!
      ..update('')
      ..focus = (directions.from ?? directions.to)?.point;
    setState(() => _editing = field);
    _focus.requestFocus();
    _sheet!.animateTo(const SheetAnchor.full());
  }

  /// Back to showing the trip. The summary's section isn't laid out while editing, so
  /// the sheet moves to it once it is.
  void _stopEditing() {
    if (_editing == null) return;
    setState(() => _editing = null);
    _focus.unfocus();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _sheet!.animateTo(_summaryAnchor);
    });
  }

  Future<void> _choose(Place place) async {
    final field = _editing;
    if (field == null) return;
    await StoreScope.of(context).places.put(place);
    if (!mounted) return;
    _directions!.setEndpoint(field, RouteEndpoint.place(place));
    _stopEditing();
  }

  Future<void> _useMyLocation() async {
    final field = _editing;
    if (field == null) return;
    final location = DirectionsScope.of(context).location;
    final result = await _directions!.useMyLocation(field);
    if (!mounted) return;
    switch (result) {
      case LocateResult.located:
        _stopEditing();
      case LocateResult.serviceOff:
        showSheetMessage(
          context,
          'Location is off',
          action: 'Settings',
          onAction: location.openLocationSettings,
        );
      case LocateResult.denied:
        showSheetMessage(context, 'Cardinal needs your location for this');
      case LocateResult.deniedForever:
        showSheetMessage(
          context,
          'Location is turned off for Cardinal',
          action: 'Settings',
          onAction: location.openAppSettings,
        );
      case LocateResult.unavailable:
        showSheetMessage(context, "Your location isn't available right now");
    }
  }

  @override
  Widget build(BuildContext context) {
    final editing = _editing;
    return PopScope(
      canPop: editing == null,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _stopEditing();
      },
      child: SheetContent(
        anchors: editing != null
            ? const [SheetAnchor.full()]
            : const [
                SheetAnchor.section(_modes),
                _summaryAnchor,
                SheetAnchor.full(),
              ],
        initialAnchor: 1,
        slivers: editing != null
            ? _editingSlivers(editing)
            : _tripSlivers(_directions!),
      ),
    );
  }

  List<Widget> _tripSlivers(Directions directions) {
    final theme = Theme.of(context);
    final miles = usesMiles(PlatformDispatcher.instance.locale);
    final route = directions.selectedRoute;
    final bothEnds = directions.from != null && directions.to != null;
    return [
      SliverToBoxAdapter(
        child: SheetSection(
          id: _modes,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 4, 8, 4),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Directions',
                        style: theme.textTheme.headlineSmall,
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
              _EndRow(
                label: 'From',
                endpoint: directions.from,
                onTap: () => _edit(RouteField.from),
                onClear: () => directions.setEndpoint(RouteField.from, null),
                action: IconButton(
                  tooltip: 'Recalculate route',
                  icon: const Icon(Icons.refresh),
                  onPressed: bothEnds && !directions.loading
                      ? directions.recalculate
                      : null,
                ),
              ),
              const SizedBox(height: 8),
              _EndRow(
                label: 'To',
                endpoint: directions.to,
                onTap: () => _edit(RouteField.to),
                onClear: () => directions.setEndpoint(RouteField.to, null),
                action: IconButton(
                  tooltip: 'Swap start and destination',
                  icon: const Icon(Icons.swap_vert),
                  onPressed: bothEnds ? directions.swap : null,
                ),
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
                child: Row(
                  spacing: 8,
                  children: [
                    for (final mode in TravelMode.values)
                      Expanded(
                        child: _ModeChip(
                          mode: mode,
                          selected: mode == directions.mode,
                          onTap: () => _setMode(mode),
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
      SliverToBoxAdapter(
        child: SheetSection(
          id: _summary,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(
                height: 2,
                child: directions.loading || directions.locating
                    ? const LinearProgressIndicator()
                    : null,
              ),
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 4, 12, 4),
                child: route != null
                    ? _Summary(route: route, miles: miles)
                    : _Message(_messageFor(directions)),
              ),
            ],
          ),
        ),
      ),
      if (route != null)
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
          sliver: SliverList.builder(
            itemCount: route.steps.length,
            itemBuilder: (context, i) =>
                _StepCard(number: i + 1, step: route.steps[i], miles: miles),
          ),
        ),
    ];
  }

  String _messageFor(Directions directions) => switch (directions) {
    Directions(problem: RouteProblem.noRoute) => 'No route found',
    Directions(problem: RouteProblem.failed) =>
      "Couldn't get directions. Check your connection and try again.",
    Directions(loading: true) => 'Finding routes…',
    _ => 'Enter a start and a destination to get directions',
  };

  List<Widget> _editingSlivers(RouteField field) {
    final search = _search!;
    return [
      SliverToBoxAdapter(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
          child: TextField(
            controller: _text,
            focusNode: _focus,
            onChanged: search.update,
            onSubmitted: (_) {
              if (search.results case [final first, ...]) _choose(first);
            },
            decoration: InputDecoration(
              labelText: field == RouteField.from ? 'From' : 'To',
              hintText: 'Enter a place',
              border: const OutlineInputBorder(),
              isDense: true,
              prefixIcon: const Icon(Icons.location_on_outlined),
              suffixIcon: IconButton(
                tooltip: 'Clear',
                icon: const Icon(Icons.close),
                onPressed: () {
                  _text.clear();
                  search.update('');
                  _directions!.setEndpoint(field, null);
                  _focus.requestFocus();
                },
              ),
            ),
          ),
        ),
      ),
      ListenableBuilder(
        listenable: Listenable.merge([search, _directions!]),
        builder: (context, _) => _EndChoices(
          search: search,
          saved: _saved,
          locating: _directions!.locating,
          onMyLocation: _useMyLocation,
          onPlace: _choose,
        ),
      ),
    ];
  }
}

/// A trip's end as a field to tap, and a button beside it.
class _EndRow extends StatelessWidget {
  const _EndRow({
    required this.label,
    required this.endpoint,
    required this.onTap,
    required this.onClear,
    required this.action,
  });

  final String label;
  final RouteEndpoint? endpoint;
  final VoidCallback onTap;
  final VoidCallback onClear;
  final Widget action;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.only(left: 16, right: 8),
    child: Row(
      children: [
        Expanded(
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(4),
            child: InputDecorator(
              isEmpty: endpoint == null,
              decoration: InputDecoration(
                labelText: label,
                border: const OutlineInputBorder(),
                isDense: true,
                prefixIcon: const Icon(Icons.location_on_outlined),
                suffixIcon: endpoint == null
                    ? null
                    : IconButton(
                        tooltip: 'Clear ${label.toLowerCase()}',
                        icon: const Icon(Icons.close),
                        onPressed: onClear,
                      ),
              ),
              child: Text(
                endpoint?.label ?? '',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ),
        ),
        action,
      ],
    ),
  );
}

/// A chip for a way of traveling.
class _ModeChip extends StatelessWidget {
  const _ModeChip({
    required this.mode,
    required this.selected,
    required this.onTap,
  });

  final TravelMode mode;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final enabled = mode.available;
    final radius = BorderRadius.circular(12);
    return Semantics(
      button: true,
      enabled: enabled,
      selected: selected,
      label: mode.label,
      excludeSemantics: true,
      onTap: enabled ? onTap : null,
      child: Tooltip(
        message: mode.label,
        child: Material(
          color: selected ? scheme.primary : scheme.surfaceContainerHighest,
          borderRadius: radius,
          child: InkWell(
            borderRadius: radius,
            onTap: enabled ? onTap : null,
            child: SizedBox(
              height: 40,
              child: Icon(
                mode.icon,
                size: 22,
                color: selected
                    ? scheme.onPrimary
                    : enabled
                    ? scheme.onSurfaceVariant
                    : scheme.onSurface.withValues(alpha: 0.38),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A route's distance and travel time, with the button that will start navigation.
class _Summary extends StatelessWidget {
  const _Summary({required this.route, required this.miles});

  final DirectionsRoute route;
  final bool miles;

  @override
  Widget build(BuildContext context) => Card.filled(
    margin: EdgeInsets.zero,
    child: Padding(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _SummaryRow('Distance', formatDistance(route.distance, miles: miles)),
          const SizedBox(height: 4),
          _SummaryRow('Duration', formatDuration(route.duration)),
          const SizedBox(height: 8),
          const FilledButton(onPressed: null, child: Text('Start Navigation')),
        ],
      ),
    ),
  );
}

class _SummaryRow extends StatelessWidget {
  const _SummaryRow(this.label, this.value);

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      mainAxisAlignment: MainAxisAlignment.spaceBetween,
      children: [
        Text(label, style: theme.textTheme.bodyLarge),
        Text(
          value,
          style: theme.textTheme.bodyLarge?.copyWith(
            color: theme.colorScheme.primary,
          ),
        ),
      ],
    );
  }
}

/// A numbered step of a route.
class _StepCard extends StatelessWidget {
  const _StepCard({
    required this.number,
    required this.step,
    required this.miles,
  });

  final int number;
  final DirectionsStep step;
  final bool miles;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card.filled(
      margin: const EdgeInsets.symmetric(vertical: 4),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        child: Row(
          children: [
            SizedBox(
              width: 28,
              child: Text(
                '$number',
                textAlign: TextAlign.center,
                style: theme.textTheme.labelLarge,
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Text(step.instruction, style: theme.textTheme.bodyMedium),
            ),
            const SizedBox(width: 12),
            Text(
              formatDistance(step.distance, miles: miles),
              style: theme.textTheme.bodySmall?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _Message extends StatelessWidget {
  const _Message(this.message);

  final String message;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Text(
        message,
        textAlign: TextAlign.center,
        style: theme.textTheme.bodyMedium?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}

/// What can fill the end being edited: My Location and saved places until there's text,
/// and then what the text finds.
class _EndChoices extends StatelessWidget {
  const _EndChoices({
    required this.search,
    required this.saved,
    required this.locating,
    required this.onMyLocation,
    required this.onPlace,
  });

  final PlaceSearch search;
  final List<SavedPlace> saved;
  final bool locating;
  final VoidCallback onMyLocation;
  final void Function(Place place) onPlace;

  @override
  Widget build(BuildContext context) {
    final searching = search.text.trim().isNotEmpty;
    final message = switch (search) {
      PlaceSearch(error: final error?) => 'Search failed: $error',
      PlaceSearch(loading: false, results: []) when searching => 'No results',
      _ => null,
    };
    return SliverMainAxisGroup(
      slivers: [
        SliverToBoxAdapter(
          child: SizedBox(
            height: 2,
            child: search.loading ? const LinearProgressIndicator() : null,
          ),
        ),
        if (message != null) SliverToBoxAdapter(child: _Message(message)),
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 4, 12, 12),
          sliver: searching
              ? SliverList.builder(
                  itemCount: search.results.length,
                  itemBuilder: (context, i) => _ChoiceCard(
                    title: placeTitle(search.results[i]),
                    subtitle: placeSubtitle(search.results[i]),
                    onTap: () => onPlace(search.results[i]),
                  ),
                )
              : SliverList.builder(
                  itemCount: saved.length + 1,
                  itemBuilder: (context, i) => i == 0
                      ? _MyLocationCard(locating: locating, onTap: onMyLocation)
                      : _ChoiceCard(
                          title: savedName(saved[i - 1]),
                          subtitle: placeSubtitle(saved[i - 1].place),
                          onTap: () => onPlace(saved[i - 1].place),
                        ),
                ),
        ),
      ],
    );
  }
}

/// My Location, which asks for permission when it's tapped if that's needed.
class _MyLocationCard extends StatelessWidget {
  const _MyLocationCard({required this.locating, required this.onTap});

  final bool locating;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final primary = Theme.of(context).colorScheme.primary;
    return Card.filled(
      margin: const EdgeInsets.symmetric(vertical: 4),
      clipBehavior: Clip.antiAlias,
      child: ListTile(
        leading: locating
            ? const SizedBox.square(
                dimension: 24,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            : Icon(Icons.my_location, color: primary),
        title: Text('My Location', style: TextStyle(color: primary)),
        subtitle: const Text('Use current location'),
        onTap: onTap,
      ),
    );
  }
}

class _ChoiceCard extends StatelessWidget {
  const _ChoiceCard({
    required this.title,
    required this.subtitle,
    required this.onTap,
  });

  final String title;
  final String? subtitle;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Card.filled(
    margin: const EdgeInsets.symmetric(vertical: 4),
    clipBehavior: Clip.antiAlias,
    child: ListTile(
      title: Text(title, maxLines: 1, overflow: TextOverflow.ellipsis),
      subtitle: subtitle == null
          ? null
          : Text(subtitle!, maxLines: 1, overflow: TextOverflow.ellipsis),
      onTap: onTap,
    ),
  );
}
