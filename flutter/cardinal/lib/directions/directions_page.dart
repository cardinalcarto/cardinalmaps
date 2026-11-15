import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show PlatformDispatcher;

import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoActivityIndicator,
        CupertinoButton,
        CupertinoButtonSize,
        CupertinoIcons,
        CupertinoSearchTextField,
        CupertinoSlidingSegmentedControl;
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
import '../navigation/navigation_scope.dart';
import '../routes.dart';
import '../saved/saved_feedback.dart';
import '../saved/saved_names.dart';
import '../search/place_search.dart';
import '../settings/accessibility.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import '../ui/haptics.dart';
import '../units.dart';
import 'callouts.dart';
import 'directions.dart';
import 'format.dart';
import 'route_source.dart';
import 'transit_details.dart';
import 'transit_plan.dart';
import 'transit_sheet.dart';
import 'transit_timeline.dart';

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
/// | showing the trip | transit is chosen | its journeys, the sheet full and held there |
/// | showing transit journeys | a journey is tapped | its legs in detail, the sheet at 40% so the map shows it |
/// | showing a journey | back, or a new plan | the journeys, the sheet full and held there |
/// | showing transit journeys | another mode is chosen | that mode's routes, the sheet free and down to the summary |
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
  static const _journeySummary = #directionsJourneySummary;

  /// Where a transit journey's details rest: low enough for the map to show its shape.
  static const _detailsAnchor = SheetAnchor.fraction(0.4);

  /// From the details anchor up, the sheet rests wherever it's let go, with no anchor
  /// at the top to spring to, and a fling up carries on into scrolling the journey, so
  /// reading through it is one long scroll.
  static const _detailsRange = SheetRange(
    _detailsAnchor,
    SheetAnchor.full(),
    carriesIntoScroll: true,
  );

  /// How often "Go in" times count down.
  static const _tick = Duration(seconds: 30);

  /// Just below the summary. Flings always stop here, on the way up or down.
  static const _summaryAnchor = SheetAnchor.section(
    _summary,
    gravity: double.infinity,
  );

  /// From just below the summary and its Go button up, as for a journey's details.
  static const _summaryRange = SheetRange(
    _summaryAnchor,
    SheetAnchor.full(),
    carriesIntoScroll: true,
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
  StreamSubscription<UnitSystem>? _watchUnits;

  /// The app's [unitSystem] setting, resolved for the device's region as each build shows
  /// distances, so a change to either shows at once. The default stands until the setting
  /// is read.
  UnitSystem _units = UnitSystem.locale;
  StreamSubscription<int>? _taps;
  StreamSubscription<bool>? _watchWheelchair;

  /// How transit last got to and from stops, or null if that's never been chosen.
  TransitAccess? _rememberedAccess;
  Timer? _calloutTimer;
  Timer? _clock;

  final _text = TextEditingController();
  final _focus = FocusNode(debugLabel: 'directions');

  /// The end being edited, if one is.
  RouteField? _editing;

  /// Whether the selected transit journey's legs are showing.
  bool _journeyOpen = false;

  /// The transit journey the map shows.
  Journey? _shownJourney;

  List<SavedPlace> _saved = const [];

  /// The routes the map shows, and what's worked out from them.
  List<DirectionsRoute> _shownRoutes = const [];
  CalloutPlacer? _placer;
  List<Callout> _callouts = const [];

  bool _dark = false;

  /// Whether the navigation screen shows over this page, and has the map: while it does,
  /// this page draws only the destination's pin on it, which the trip leads to, and
  /// draws the rest of the trip again when it closes.
  NavigationScreen? _navigation;
  bool get _navigating => _navigation?.shown ?? false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _map = MapScope.of(context);
    final navigation = NavigationScope.of(context).screen;
    if (navigation != _navigation) {
      _navigation?.removeListener(_navigationChanged);
      _navigation = navigation..addListener(_navigationChanged);
    }
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
      transit: scope.transit,
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
    _watchUnits = store.settings.watch(unitSystem).listen((units) {
      if (units != _units) setState(() => _units = units);
    });
    _taps = routeTaps.stream.listen(directions.select);
    _map.cameraMoved.addListener(_viewChanged);
    // This page's own pin marks the destination, which can change.
    showSelection(_map, null);
    unawaited(_resume(directions));
  }

  /// Starts with the mode last chosen, by wheelchair if that's the default, then from
  /// where you are. The default is followed if it changes while this page is open.
  Future<void> _resume(Directions directions) async {
    final settings = _settings!;
    final wheelchair = await settings.get(wheelchairByDefault);
    _rememberedAccess = await settings.get(lastTransitAccess);
    directions
      ..setWheelchair(wheelchair)
      ..setTransitAccess(
        transitAccess(_rememberedAccess, wheelchair: wheelchair),
      )
      ..setMode(await settings.get(lastTravelMode));
    if (!mounted) return;
    _watchWheelchair = settings
        .watch(wheelchairByDefault)
        .listen(_wheelchairChanged);
    await directions.startFromMyLocation();
  }

  /// Walking, and getting to stops if no way has been chosen, follow the default.
  void _wheelchairChanged(bool wheelchair) {
    _directions!
      ..setWheelchair(wheelchair)
      ..setTransitAccess(
        transitAccess(_rememberedAccess, wheelchair: wheelchair),
      );
  }

  @override
  void dispose() {
    _watchTo?.cancel();
    _watchSaved?.cancel();
    _watchUnits?.cancel();
    _taps?.cancel();
    _watchWheelchair?.cancel();
    _calloutTimer?.cancel();
    _clock?.cancel();
    _map.cameraMoved.removeListener(_viewChanged);
    _sheet?.removeListener(_viewChanged);
    _navigation?.removeListener(_navigationChanged);
    _map
      ..setGeoJson(routeSource, empty)
      ..setGeoJson(routeCalloutSource, empty)
      ..setGeoJson(transitSource, empty);
    _directions?.dispose();
    _search?.dispose();
    _focus.dispose();
    _text.dispose();
    super.dispose();
  }

  /// The navigation screen opened over this page, or closed: take the trip off the map
  /// for it but for the destination's pin, or draw it again and show all of it.
  /// Navigation leaves the map tilted and turned to the road ahead, so the routes come
  /// back flat and north up, as they were first shown, in the same flight that fits
  /// them.
  void _navigationChanged() {
    if (!mounted) return;
    if (_navigating) {
      _calloutTimer?.cancel();
      _callouts = const [];
      _drawRoutes();
      _map.setGeoJson(routeCalloutSource, empty);
    } else {
      _drawRoutes();
      _showAll(flat: true);
      _placeCallouts();
    }
  }

  /// Starts navigating the selected route, and opens the navigation screen over this
  /// page.
  void _startNavigation() {
    final directions = _directions!;
    final route = directions.selectedRoute;
    if (route == null) return;
    NavigationScope.of(context).controller.start(
      route: route,
      mode: directions.mode,
      wheelchair: directions.wheelchair,
      destinationName: switch (directions.to) {
        RouteEndpoint(:final place?) => placeTitle(place),
        _ => null,
      },
    );
    unawaited(context.push(navigationLocation(widget.to)));
  }

  /// The trip changed: redraw it, and if the routes are new, show them all and place
  /// their callouts.
  void _changed() {
    if (!mounted) return;
    final directions = _directions!;
    if (_navigating) {
      // Drawn as it is when navigation closes.
      setState(() {});
      return;
    }
    if (!identical(directions.routes, _shownRoutes)) {
      _shownRoutes = directions.routes;
      _placer = _shownRoutes.isEmpty ? null : CalloutPlacer(_shownRoutes);
      _showAll();
    }
    final journey = directions.journey;
    if (!identical(journey, _shownJourney)) {
      if (_journeyOpen &&
          (journey == null ||
              directions.plan?.journeys.contains(_shownJourney) != true)) {
        // A new plan: back to its list, which holds the sheet full.
        _journeyOpen = false;
        _animateAfterFrame(const SheetAnchor.full());
      }
      _shownJourney = journey;
      _showJourney();
    }
    _drawRoutes();
    _placeCallouts();
    _keepClock(directions.mode == TravelMode.transit);
    setState(() {});
  }

  /// While transit shows, "Go in" counts down.
  void _keepClock(bool running) {
    if (!running) {
      _clock?.cancel();
      _clock = null;
    } else {
      _clock ??= Timer.periodic(_tick, (_) {
        if (mounted) setState(() {});
      });
    }
  }

  /// Springs the sheet to [anchor] once this frame has laid out the content that has it.
  void _animateAfterFrame(SheetAnchor anchor) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _sheet!.animateTo(anchor);
    });
  }

  /// Where the sheet rests for the trip, outside a journey's details: full for transit,
  /// whose journeys need the room, and below the summary otherwise.
  SheetAnchor get _tripAnchor => _directions!.mode == TravelMode.transit
      ? const SheetAnchor.full()
      : _summaryAnchor;

  /// Draws the routes, their start and the destination's pin, and the transit journey;
  /// while navigating, only the pin, the trip's own drawing taking the place of the
  /// rest.
  void _drawRoutes() {
    final directions = _directions!;
    final navigating = _navigating;
    _map.setGeoJson(
      routeSource,
      routesGeoJson(
        navigating ? const [] : directions.routes,
        selected: directions.selected,
        destination: directions.to?.point,
        start: navigating || (directions.from?.isMyLocation ?? true)
            ? null
            : directions.from!.point,
        dark: _dark,
      ),
    );
    _map.setGeoJson(
      transitSource,
      journeyGeoJson(navigating ? null : _shownJourney),
    );
  }

  /// Makes the transit journey the map's subject.
  void _showJourney() {
    final points = _shownJourney?.legs.expand((leg) => leg.shape) ?? const [];
    _fit(points);
  }

  /// Makes all of the routes the map's subject, kept in view as the sheet moves. With
  /// [flat], the map also turns north up and untilts on the way; otherwise it keeps
  /// whatever tilt and turn the user gave it.
  void _showAll({bool flat = false}) =>
      _fit(_shownRoutes.expand((route) => route.geometry), flat: flat);

  /// Makes [points] the map's subject: see [_showAll] for [flat].
  void _fit(Iterable<DirectionsPoint> points, {bool flat = false}) {
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
      bearing: flat ? 0 : null,
      pitch: flat ? 0 : null,
    );
  }

  /// The map or the sheet moved: once they're still, the callouts follow.
  void _viewChanged() {
    _calloutTimer?.cancel();
    if (_navigating) return;
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
    final directions = _directions!;
    final was = directions.mode;
    directions.setMode(mode);
    if (directions.mode == was) return;
    unawaited(_settings!.set(lastTravelMode, mode));
    // Into transit the sheet goes full and stays; out of it, it's free again and comes
    // down to the anchor nearest full.
    if (mode == TravelMode.transit || was == TravelMode.transit) {
      _animateAfterFrame(_tripAnchor);
    }
  }

  void _setTransitAccess(TransitAccess access) {
    _rememberedAccess = access;
    _directions!.setTransitAccess(access);
    unawaited(_settings!.set(lastTransitAccess, access));
  }

  /// A journey's tapped: show its legs, and it alone on the map, with the sheet low
  /// enough for the map to show it.
  void _openJourney(int index) {
    _directions!.selectJourney(index);
    setState(() => _journeyOpen = true);
    _animateAfterFrame(_detailsAnchor);
  }

  void _closeJourney() {
    setState(() => _journeyOpen = false);
    _animateAfterFrame(_tripAnchor);
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
    _animateAfterFrame(_tripAnchor);
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
    final journey = _journeyOpen ? _directions!.journey : null;
    final transit = _directions!.mode == TravelMode.transit;
    return PopScope(
      canPop: editing == null && journey == null,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) return;
        if (editing != null) {
          _stopEditing();
        } else {
          _closeJourney();
        }
      },
      child: SheetContent(
        anchors: editing != null
            ? const [SheetAnchor.full()]
            : journey != null
            ? const [_detailsAnchor]
            : transit
            ? const [SheetAnchor.full()]
            : const [SheetAnchor.section(_modes), _summaryAnchor],
        freeRanges: editing != null
            ? const []
            : journey != null
            ? const [_detailsRange]
            : transit
            ? const []
            : const [_summaryRange],
        initialAnchor: 1,
        // Transit's journeys hold the sheet full until one is picked.
        draggable: editing != null || journey != null || !transit,
        slivers: editing != null
            ? _editingSlivers(editing)
            : journey != null
            ? _journeySlivers(_directions!, journey)
            : _tripSlivers(_directions!),
      ),
    );
  }

  List<Widget> _tripSlivers(Directions directions) {
    final theme = Theme.of(context);
    final cupertino = isCupertinoOf(context);
    final units = _units.onDevice;
    final transit = directions.mode == TravelMode.transit;
    final route = transit ? null : directions.selectedRoute;
    final plan = transit && !directions.loading ? directions.plan : null;
    final journeys = plan?.journeys ?? const <Journey>[];
    final axis = TimelineAxis.of(journeys);
    final now = DateTime.now();
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
                    AppCloseButton(
                      onPressed: () =>
                          context.canPop() ? context.pop() : context.go('/'),
                    ),
                  ],
                ),
              ),
              if (cupertino)
                _CupertinoEnds(
                  directions: directions,
                  onEdit: _edit,
                  bothEnds: bothEnds,
                )
              else ...[
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
              ],
              Padding(
                padding: EdgeInsets.fromLTRB(16, 12, 16, transit ? 6 : 12),
                child: cupertino
                    ? _CupertinoModes(
                        selected: directions.mode,
                        wheelchair: directions.wheelchair,
                        onChanged: _setMode,
                      )
                    : Row(
                        spacing: 8,
                        children: [
                          for (final mode in TravelMode.values)
                            Expanded(
                              child: _ModeChip(
                                mode: mode,
                                wheelchair: directions.wheelchair,
                                selected: mode == directions.mode,
                                onTap: () => _setMode(mode),
                              ),
                            ),
                        ],
                      ),
              ),
              if (transit)
                TransitAccessRow(
                  access: directions.transitAccess,
                  onChanged: _setTransitAccess,
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
              if (!cupertino)
                SizedBox(
                  height: 2,
                  child: directions.loading || directions.locating
                      ? const LinearProgressIndicator()
                      : null,
                )
              else if (directions.loading || directions.locating)
                const SizedBox(height: 28, child: CupertinoActivityIndicator()),
              if (transit) ...[
                if (plan != null && journeys.isNotEmpty) ...[
                  RealtimeLine(plan: plan),
                  Padding(
                    padding: const EdgeInsets.fromLTRB(16, 4, 16, 0),
                    child: TimelineTicks(axis: axis),
                  ),
                ] else
                  _Message(
                    plan != null && directions.problem == null
                        ? noJourneysMessage(plan, directions.transitAccess)
                        : _messageFor(directions),
                  ),
              ] else
                Padding(
                  padding: const EdgeInsets.fromLTRB(12, 4, 12, 4),
                  child: route == null
                      ? _Message(_messageFor(directions))
                      : cupertino
                      ? _CupertinoSummary(
                          route: route,
                          units: units,
                          onStart: _startNavigation,
                        )
                      : _Summary(
                          route: route,
                          units: units,
                          onStart: _startNavigation,
                        ),
                ),
            ],
          ),
        ),
      ),
      if (journeys.isNotEmpty)
        SliverPadding(
          padding: const EdgeInsets.only(bottom: 12),
          sliver: SliverList.separated(
            itemCount: journeys.length,
            itemBuilder: (context, i) => JourneyRow(
              journey: journeys[i],
              axis: axis,
              now: now,
              onTap: () => _openJourney(i),
            ),
            separatorBuilder: (context, i) =>
                const Divider(height: 1, indent: 16, endIndent: 16),
          ),
        ),
      if (route != null && cupertino && route.steps.isNotEmpty)
        SliverPadding(
          padding: const EdgeInsets.only(bottom: 12),
          sliver: SliverToBoxAdapter(
            child: AppListSection(
              header: 'Steps',
              children: [
                for (final (i, step) in route.steps.indexed)
                  _CupertinoStep(number: i + 1, step: step, units: units),
              ],
            ),
          ),
        )
      else if (route != null && !cupertino)
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
          sliver: SliverList.builder(
            itemCount: route.steps.length,
            itemBuilder: (context, i) =>
                _StepCard(number: i + 1, step: route.steps[i], units: units),
          ),
        ),
    ];
  }

  /// A transit journey's legs in detail, under a header to go back to the journeys.
  List<Widget> _journeySlivers(Directions directions, Journey journey) {
    final units = _units.onDevice;
    final now = DateTime.now();
    return [
      SliverToBoxAdapter(
        child: SheetSection(
          id: _journeySummary,
          child: JourneyHeader(
            journey: journey,
            now: now,
            units: units,
            onBack: _closeJourney,
          ),
        ),
      ),
      JourneyItinerary(
        journey: journey,
        start: directions.from?.label ?? 'Start',
        destination: directions.to?.label ?? 'Destination',
        units: units,
        access: directions.transitAccess,
        now: now,
      ),
    ];
  }

  String _messageFor(Directions directions) => switch (directions) {
    Directions(problem: RouteProblem.outsideArea) =>
      "This trip is outside the offline area, so there are no directions for it "
          "on this device",
    Directions(problem: RouteProblem.noOfflineArea) =>
      'Directions need an offline area, and this device has none',
    Directions(problem: RouteProblem.noRoute) => 'No route found',
    Directions(problem: RouteProblem.failed) =>
      "Couldn't get directions. Try again.",
    Directions(loading: true) => 'Finding routes…',
    _ => 'Enter a start and a destination to get directions',
  };

  List<Widget> _editingSlivers(RouteField field) {
    final search = _search!;
    return [
      SliverToBoxAdapter(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
          child: isCupertinoOf(context)
              // With Cancel beside it, as iOS has for a search, doing what back does.
              ? Row(
                  children: [
                    Expanded(
                      child: CupertinoSearchTextField(
                        controller: _text,
                        focusNode: _focus,
                        placeholder: field == RouteField.from
                            ? 'Search for a start'
                            : 'Search for a destination',
                        onChanged: search.update,
                        onSubmitted: (_) {
                          if (search.results case [final first, ...]) {
                            _choose(first);
                          }
                        },
                        onSuffixTap: () {
                          _text.clear();
                          search.update('');
                          _directions!.setEndpoint(field, null);
                          _focus.requestFocus();
                        },
                      ),
                    ),
                    CupertinoButton(
                      padding: const EdgeInsetsDirectional.only(start: 12),
                      onPressed: _stopEditing,
                      child: const Text('Cancel'),
                    ),
                  ],
                )
              : TextField(
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
    required this.wheelchair,
    required this.selected,
    required this.onTap,
  });

  final TravelMode mode;

  /// Whether walking is by wheelchair, which the walking chip then says.
  final bool wheelchair;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final enabled = mode.available;
    final radius = BorderRadius.circular(12);
    final label = mode.labelFor(wheelchair: wheelchair);
    return Semantics(
      button: true,
      enabled: enabled,
      selected: selected,
      label: label,
      excludeSemantics: true,
      onTap: enabled ? onTap : null,
      child: Tooltip(
        message: label,
        child: Material(
          color: selected ? scheme.primary : scheme.surfaceContainerHighest,
          borderRadius: radius,
          child: InkWell(
            borderRadius: radius,
            onTap: enabled ? onTap : null,
            child: SizedBox(
              height: 40,
              child: Icon(
                mode.iconFor(wheelchair: wheelchair),
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

/// A route's distance and travel time, with the button that starts navigation.
class _Summary extends StatelessWidget {
  const _Summary({
    required this.route,
    required this.units,
    required this.onStart,
  });

  final DirectionsRoute route;
  final UnitSystem units;
  final VoidCallback onStart;

  @override
  Widget build(BuildContext context) => Card.filled(
    margin: EdgeInsets.zero,
    child: Padding(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _SummaryRow('Distance', formatDistance(route.distance, units: units)),
          const SizedBox(height: 4),
          _SummaryRow('Duration', formatDuration(route.duration)),
          const SizedBox(height: 8),
          FilledButton(
            onPressed: onStart,
            child: const Text('Start Navigation'),
          ),
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
    required this.units,
  });

  final int number;
  final DirectionsStep step;
  final UnitSystem units;

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
              formatDistance(step.distance, units: units),
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
    if (isCupertinoOf(context)) {
      final accent = Theme.of(context).colorScheme.primary;
      Widget row(String title, String? subtitle, VoidCallback onTap) =>
          AppListTile(
            title: Text(title, maxLines: 1, overflow: TextOverflow.ellipsis),
            subtitle: subtitle == null
                ? null
                : Text(subtitle, maxLines: 1, overflow: TextOverflow.ellipsis),
            onTap: onTap,
          );
      return SliverMainAxisGroup(
        slivers: [
          if (search.loading)
            const SliverToBoxAdapter(
              child: SizedBox(height: 28, child: CupertinoActivityIndicator()),
            ),
          if (message != null) SliverToBoxAdapter(child: _Message(message)),
          SliverToBoxAdapter(
            child: AppListSection(
              children: searching
                  ? [
                      for (final place in search.results)
                        row(
                          placeTitle(place),
                          placeSubtitle(place),
                          () => onPlace(place),
                        ),
                    ]
                  : [
                      AppListTile(
                        leading: locating
                            ? const CupertinoActivityIndicator()
                            : Icon(CupertinoIcons.location_fill, color: accent),
                        title: Text(
                          'My Location',
                          style: TextStyle(color: accent),
                        ),
                        onTap: onMyLocation,
                      ),
                      for (final place in saved)
                        row(
                          savedName(place),
                          placeSubtitle(place.place),
                          () => onPlace(place.place),
                        ),
                    ],
            ),
          ),
        ],
      );
    }
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

/// iOS's trip ends: From and To together in one grouped card, each with its marker and
/// its action beside the card: Recalculate by From, Swap by To.
class _CupertinoEnds extends StatelessWidget {
  const _CupertinoEnds({
    required this.directions,
    required this.onEdit,
    required this.bothEnds,
  });

  final Directions directions;
  final void Function(RouteField field) onEdit;
  final bool bothEnds;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    Widget end(RouteField field) {
      final from = field == RouteField.from;
      final endpoint = from ? directions.from : directions.to;
      final label = from ? 'From' : 'To';
      return AppListTile(
        leading: Icon(
          from ? CupertinoIcons.circle_fill : CupertinoIcons.location_solid,
          size: from ? 14 : 20,
          color: from ? const Color(0xFF34C759) : const Color(0xFFFF3B30),
        ),
        title: Text(
          endpoint?.label ?? label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: endpoint == null
              ? TextStyle(color: theme.colorScheme.onSurfaceVariant)
              : null,
        ),
        trailing: endpoint == null
            ? null
            : Semantics(
                button: true,
                label: 'Clear ${label.toLowerCase()}',
                excludeSemantics: true,
                child: CupertinoButton(
                  padding: EdgeInsets.zero,
                  minimumSize: const Size.square(32),
                  onPressed: () => directions.setEndpoint(field, null),
                  child: Icon(
                    CupertinoIcons.xmark_circle_fill,
                    size: 18,
                    color: theme.colorScheme.onSurfaceVariant,
                  ),
                ),
              ),
        onTap: () => onEdit(field),
      );
    }

    Widget action(String label, IconData icon, VoidCallback? onPressed) =>
        Semantics(
          button: true,
          label: label,
          excludeSemantics: true,
          child: CupertinoButton(
            padding: EdgeInsets.zero,
            minimumSize: const Size.square(44),
            onPressed: onPressed,
            child: Icon(icon, size: 20),
          ),
        );

    return Row(
      children: [
        Expanded(
          child: AppListSection(
            children: [end(RouteField.from), end(RouteField.to)],
          ),
        ),
        Padding(
          padding: const EdgeInsets.only(right: 8, top: 8),
          child: Column(
            children: [
              action(
                'Recalculate route',
                CupertinoIcons.arrow_clockwise,
                bothEnds && !directions.loading ? directions.recalculate : null,
              ),
              action(
                'Swap start and destination',
                CupertinoIcons.arrow_up_arrow_down,
                bothEnds ? directions.swap : null,
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// iOS's ways of traveling: a segmented control, with those that can't be routed yet
/// disabled.
class _CupertinoModes extends StatelessWidget {
  const _CupertinoModes({
    required this.selected,
    required this.wheelchair,
    required this.onChanged,
  });

  final TravelMode selected;

  /// Whether walking is by wheelchair, which its segment then says.
  final bool wheelchair;
  final ValueChanged<TravelMode> onChanged;

  @override
  Widget build(BuildContext context) =>
      CupertinoSlidingSegmentedControl<TravelMode>(
        groupValue: selected,
        disabledChildren: {
          for (final mode in TravelMode.values)
            if (!mode.available) mode,
        },
        children: {
          for (final mode in TravelMode.values)
            mode: Semantics(
              label: mode.labelFor(wheelchair: wheelchair),
              excludeSemantics: true,
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 4),
                child: Icon(mode.iconFor(wheelchair: wheelchair), size: 20),
              ),
            ),
        },
        onValueChanged: (mode) {
          if (mode == null || mode == selected) return;
          Haptics.selection();
          onChanged(mode);
        },
      );
}

/// iOS's route summary: the time large, the distance under it, and Go, which starts
/// navigation, as Apple shows a route.
class _CupertinoSummary extends StatelessWidget {
  const _CupertinoSummary({
    required this.route,
    required this.units,
    required this.onStart,
  });

  final DirectionsRoute route;
  final UnitSystem units;
  final VoidCallback onStart;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return DecoratedBox(
      decoration: ShapeDecoration(
        color: groupedRowColor(context),
        shape: RoundedSuperellipseBorder(
          borderRadius: BorderRadius.circular(16),
        ),
      ),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 12, 12),
        child: Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    formatDuration(route.duration),
                    style: theme.textTheme.headlineSmall,
                  ),
                  Text(
                    formatDistance(route.distance, units: units),
                    style: theme.textTheme.bodyMedium?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ],
              ),
            ),
            // Read out as what it does; "Go" alone says little.
            Semantics(
              button: true,
              label: 'Start Navigation',
              onTap: onStart,
              excludeSemantics: true,
              child: CupertinoButton.filled(
                color: const Color(0xFF34C759),
                sizeStyle: CupertinoButtonSize.large,
                onPressed: onStart,
                child: const Text('Go'),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A numbered step of a route, in iOS's grouped list.
class _CupertinoStep extends StatelessWidget {
  const _CupertinoStep({
    required this.number,
    required this.step,
    required this.units,
  });

  final int number;
  final DirectionsStep step;
  final UnitSystem units;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AppListTile(
      leading: Text(
        '$number',
        style: theme.textTheme.titleSmall?.copyWith(
          color: theme.colorScheme.primary,
        ),
      ),
      title: Text(step.instruction, maxLines: 3),
      trailing: Text(
        formatDistance(step.distance, units: units),
        style: theme.textTheme.bodySmall?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}
