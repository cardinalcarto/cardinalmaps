import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show PlatformDispatcher;

import 'package:cupertino_ui/cupertino_ui.dart'
    show
        CupertinoActivityIndicator,
        CupertinoButton,
        CupertinoIcons,
        CupertinoSearchTextField;
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
import '../units.dart';
import 'callouts.dart';
import 'connections.dart';
import 'departure_legs.dart';
import 'directions.dart';
import 'format.dart';
import 'mode_tabs.dart';
import 'option_row.dart';
import 'route_source.dart';
import 'transit_details.dart';
import 'transit_plan.dart';
import 'transit_sheet.dart';
import 'transit_timeline.dart';
import 'trip_buttons.dart';
import 'trip_card.dart';

/// Directions to the place with [to], as a choice among the ways to get there: the
/// trip's two ends, a tab for each way of traveling with its best time, and that way's
/// options as rows, with the routes on the map. Steps and Start, pinned under the
/// rows, act on the selected one.
///
/// Every way is asked for as soon as the trip has two ends, and each tab and list
/// fills in when its answer comes. The selected row of a street way is held in view
/// wherever the sheet is: it stops above the buttons as the rest of the sheet passes
/// behind it.
///
/// It's showing the trip, or editing one of its ends:
///
/// | In | When | Then |
/// |---|---|---|
/// | opening | | the header and up to three rows show, in no more than 60% of the window |
/// | showing the trip | a field is tapped | editing it: full, keyboard up, the field's text selected; the buttons fade out |
/// | editing | a place or My Location is chosen | that's the end; showing the trip, at the header anchor (full for transit); the rows and buttons fade in |
/// | editing | back | showing the trip, the same, the end as it was unless it was cleared |
/// | showing the trip | X, or back | the place's card |
/// | showing the trip | an end changes, or the ends are swapped | every way is asked again: placeholders, then its options, the first selected |
/// | showing the trip | another tab is tapped | that way's options, as they were left; nothing is asked |
/// | a way loading, or the start being found | | three placeholder rows, the first held; its tab's time empty; the buttons disabled |
/// | a way loading | its options arrive | its rows fade in where the placeholders were, the first selected; its tab's time fades in |
/// | a way loading | there's no route, or asking failed | the message in the first row's place, held; a dash on its tab; the buttons disabled |
/// | either end missing | | the message asking for them, in the first row's place, held; the tabs' times empty |
/// | showing a street way | a row, or its callout on the map, is tapped | that route selected: its row filled and held, the map's highlight on it |
/// | showing a street way | Steps | the steps page, pushed; back returns here, the sheet where it was |
/// | showing a street way | Start | navigating the selected route |
/// | showing a street way | transit's tab is tapped | transit's journeys, with no held row and no buttons |
/// | showing transit journeys | a journey is tapped | its legs in detail, the sheet at 40% so the map shows it |
/// | showing a journey | back, or a new plan | the journeys, the sheet full |
///
/// The sheet rests at two anchors, each with the held row above the buttons: with
/// only the handle over it, and with the ends and tabs over it. From the second up it
/// rests wherever it's let go. The rows, the placeholders and the message are one
/// height, so the anchors are the same in every street way and state.
class DirectionsPage extends StatefulWidget {
  const DirectionsPage({super.key, required this.to});

  /// The ID of the place to go to.
  final String to;

  @override
  State<DirectionsPage> createState() => _DirectionsPageState();
}

class _DirectionsPageState extends State<DirectionsPage>
    with SingleTickerProviderStateMixin {
  /// The trip's ends and the tabs.
  static const _headerSection = #directionsHeader;

  /// Ends under the last row the sheet first opens to show.
  static const _opening = #directionsOpening;
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

  /// How often the times that follow the clock are shown again: when a route
  /// arrives, how long until a journey does, and "Go in".
  static const _tick = Duration(seconds: 30);

  /// With the ends and tabs showing, over the held row. Flings always stop here, on
  /// the way up or down.
  static const _headerAnchor = SheetAnchor.section(
    _headerSection,
    gravity: double.infinity,
  );

  /// From there up, as for a journey's details.
  static const _tripRange = SheetRange(
    _headerAnchor,
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

  /// Whether the mode last chosen and the start have been looked for, so the first
  /// questions have been asked if they could be.
  bool _resumed = false;

  /// How much the rows and the buttons show: all, but not while an end is edited.
  late final _chrome = AnimationController(
    vsync: this,
    value: 1,
    duration: const Duration(milliseconds: 200),
  )..addStatusListener((_) => setState(() {}));

  final _text = TextEditingController();
  final _focus = FocusNode(debugLabel: 'directions');

  /// The end being edited, if one is.
  RouteField? _editing;

  /// Whether the selected transit journey's legs are showing.
  bool _journeyOpen = false;

  /// How many times a journey has been opened, so each opening scrolls on its own.
  int _journeyOpens = 0;

  /// The transit journey the map shows.
  Journey? _shownJourney;

  /// The departures chosen of the open journey's rides, if they've been worked out.
  ConnectionChain? _chain;

  /// The legs of the departures chosen other than the plan's, for the map to draw and
  /// the itinerary to list the stops of.
  DepartureLegs? _legs;

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
    final geocoders = GeocoderScope.of(context);
    _search = PlaceSearch(
      offline: geocoders.offline,
      online: geocoders.online,
      settings: store.settings,
      lang: PlatformDispatcher.instance.locale.toLanguageTag(),
    );
    final directions = _directions = Directions(
      routes: scope.routes,
      transit: scope.transit,
      location: scope.location,
    )..addListener(_changed);
    _legs = DepartureLegs(scope.transit)
      ..addListener(() {
        if (!mounted) return;
        // The itinerary lists the stops of one that's arrived.
        setState(() {});
        _drawRoutes();
      });
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
    _clock = Timer.periodic(_tick, (_) {
      if (mounted) setState(() {});
    });
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
    if (mounted) setState(() => _resumed = true);
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
    _legs?.dispose();
    _search?.dispose();
    _focus.dispose();
    _text.dispose();
    _chrome.dispose();
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

  /// Opens the selected route's steps over this page.
  void _openSteps() {
    unawaited(context.push(stepsLocation(widget.to), extra: _directions!));
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
        // A new plan: back to its list.
        _journeyOpen = false;
        _animateToTrip();
      }
      _shownJourney = journey;
      _showJourney();
    }
    // A newer plan of the open journey keeps its choices, as far as it can.
    if (_journeyOpen && journey != null) _chainOf(journey);
    _followChain();
    _drawRoutes();
    _placeCallouts();
    setState(() {});
  }

  /// Springs the sheet to [anchor] once this frame has laid out the content that has it.
  void _animateAfterFrame(SheetAnchor anchor) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _sheet!.animateTo(anchor);
    });
  }

  /// Springs the sheet back to where it rests for the trip, from a journey's details
  /// or from editing, once the sheet has the trip's anchors: full for transit, whose
  /// journeys have no row held in view, and to the ends and tabs over the held row
  /// otherwise.
  ///
  /// A journey's details and editing can rest at full too, so asking for it while
  /// the sheet still has their anchors would spring it to theirs, which then goes.
  void _animateToTrip() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _editing != null || _journeyOpen) return;
      final sheet = _sheet!;
      if (!sheet.ranges.any((range) => range.range == _tripRange)) {
        _animateToTrip();
        WidgetsBinding.instance.scheduleFrame();
        return;
      }
      sheet.animateTo(
        _directions!.mode == TravelMode.transit
            ? const SheetAnchor.full()
            : _headerAnchor,
      );
    });
  }

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
      journeyGeoJson(
        navigating ? null : _shownJourney,
        rides: navigating ? const {} : _chosenRides(),
      ),
    );
  }

  /// The open journey's choices, rebased onto [journey] when that's a newer plan of it.
  ConnectionChain _chainOf(Journey journey) => _chain = switch (_chain) {
    final chain? when identical(chain.journey, journey) => chain,
    final chain? => chain.rebase(journey),
    null => ConnectionChain(journey),
  };

  /// Has the legs fetched of the departures the open journey takes but the plan's own,
  /// and no others.
  void _followChain() {
    final directions = _directions!;
    final plan = directions.plan;
    if (plan == null) return;
    _legs!.follow(plan, _journeyOpen ? _chain : null, directions.transitAccess);
  }

  /// How the map draws each ride of the open journey that takes another departure than
  /// the plan's, by its index in the journey's legs: along the departure's own line once
  /// it's fetched, and until then along the plan's, which is between the same two stops,
  /// in the departure's route's colors.
  Map<int, DrawnRide> _chosenRides() {
    final journey = _shownJourney;
    final chain = _chain;
    if (!_journeyOpen ||
        journey == null ||
        chain == null ||
        !identical(chain.journey, journey)) {
      return const {};
    }
    final access = _directions!.transitAccess;
    return {
      for (final (i, leg) in journey.legs.indexed)
        if (chain.forLeg(i) case final choice?
            when choice.made && !choice.suggested)
          i: switch (_legs!.legOf(choice, access)) {
            final fetched? => (shape: fetched.shape, route: fetched.route),
            null => (shape: leg.shape, route: choice.route),
          },
    };
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
    setState(() {
      _journeyOpens++;
      _journeyOpen = true;
      _chain = null;
    });
    _chainChanged();
    _animateAfterFrame(_detailsAnchor);
  }

  void _closeJourney() {
    setState(() => _journeyOpen = false);
    _chainChanged();
    _animateToTrip();
  }

  /// The departures chosen changed: fetch the legs they need, and draw them.
  void _chainChanged() {
    _followChain();
    _drawRoutes();
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
    _chrome.reverse();
    _focus.requestFocus();
    _sheet!.animateTo(const SheetAnchor.full());
  }

  /// Back to showing the trip. Its sections aren't laid out while editing, so the
  /// sheet moves to its anchor once they are.
  void _stopEditing() {
    if (_editing == null) return;
    setState(() => _editing = null);
    _chrome.forward();
    _focus.unfocus();
    _animateToTrip();
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
    final directions = _directions!;
    final editing = _editing;
    final journey = _journeyOpen ? directions.journey : null;
    final street = directions.mode != TravelMode.transit;
    final trip = editing == null && journey == null;
    final list = street ? _optionList(directions) : null;
    // The buttons outlast the trip by as long as they take to fade when an end is
    // edited.
    final buttons =
        street && journey == null && (editing == null || !_chrome.isDismissed);
    return PopScope(
      canPop: trip,
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
            : const [SheetAnchor.footer(), _headerAnchor],
        freeRanges: editing != null
            ? const []
            : journey != null
            ? const [_detailsRange]
            : const [_tripRange],
        initialAnchor: 1,
        initialSection: _opening,
        initialMaxFraction: 0.6,
        // The trip, a journey and editing each scroll on their own: a journey opens
        // at its top whatever the list was scrolled to, and the list is where it was
        // left on the way back. Each time a journey opens is new content.
        scrollId: editing != null
            ? 'editing'
            : journey != null
            ? ('journey', _journeyOpens)
            : 'trip',
        held: trip ? list?.held : null,
        footer: buttons
            ? FadeTransition(
                opacity: _chrome,
                child: IgnorePointer(
                  ignoring: editing != null,
                  child: TripButtons(
                    onSteps: directions.selectedRoute == null
                        ? null
                        : _openSteps,
                    onStart: directions.selectedRoute == null
                        ? null
                        : () => startNavigation(
                            context,
                            directions,
                            to: widget.to,
                          ),
                  ),
                ),
              )
            : null,
        slivers: editing != null
            ? _editingSlivers(editing)
            : journey != null
            ? _journeySlivers(directions, journey)
            : [
                _header(directions),
                if (list != null)
                  SliverToBoxAdapter(child: list)
                else
                  ..._transitSlivers(directions),
              ],
      ),
    );
  }

  /// Whether the trip's start is still being found, before there's a question to
  /// ask: rows wait as they do for an answer.
  bool _waiting(Directions directions) => !_resumed || directions.locating;

  /// The time a way's tab shows: how long its best option takes, a dash if it has
  /// none, and nothing while that's not known.
  String? _tabTime(Directions directions, TravelMode mode, DateTime now) {
    final options = directions.options(mode);
    if (options.best(now) case final best?) {
      return formatDuration(best.inMilliseconds / 1000);
    }
    final asked = directions.from != null && directions.to != null;
    return options.loading || _waiting(directions) || !asked
        ? null
        : noTimeDash;
  }

  /// The trip's ends and the tabs: what shows above the selected row at the header
  /// anchor. It's the same height whatever the tabs say and whichever is selected.
  Widget _header(Directions directions) {
    final now = DirectionsScope.of(context).clock();
    return SliverToBoxAdapter(
      child: SheetSection(
        id: _headerSection,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            TripCard(
              from: directions.from,
              to: directions.to,
              onEdit: _edit,
              onClose: () => context.canPop() ? context.pop() : context.go('/'),
              onSwap: directions.from != null && directions.to != null
                  ? directions.swap
                  : null,
            ),
            ModeTabs(
              selected: directions.mode,
              wheelchair: directions.wheelchair,
              times: {
                for (final mode in TravelMode.values)
                  mode: _tabTime(directions, mode, now),
              },
              onChanged: _setMode,
            ),
            SizedBox(height: OptionRowMetrics.of(context).top),
          ],
        ),
      ),
    );
  }

  /// The rows of the street way of traveling that's showing.
  OptionList _optionList(Directions directions) => OptionList(
    routes: directions.routes,
    selected: directions.selected,
    loading: directions.loading || _waiting(directions),
    message: _messageFor(directions),
    driving: directions.mode == TravelMode.driving,
    units: _units.onDevice,
    now: DirectionsScope.of(context).clock(),
    opening: _opening,
    reveal: _chrome,
    onSelect: directions.select,
  );

  /// Transit's journeys under the tabs: how you get to stops, how live the times are,
  /// and each journey as a timeline, tapped to open it.
  List<Widget> _transitSlivers(Directions directions) {
    final cupertino = isCupertinoOf(context);
    final plan = directions.plan;
    final journeys = plan?.journeys ?? const <Journey>[];
    final axis = TimelineAxis.of(journeys);
    final now = DirectionsScope.of(context).clock();
    final waiting = directions.loading || _waiting(directions);
    Widget row(int i) => JourneyRow(
      journey: journeys[i],
      axis: axis,
      now: now,
      onTap: () => _openJourney(i),
    );
    const divider = Divider(height: 1, indent: 16, endIndent: 16);
    final first = math.min(journeys.length, OptionList.shownAtFirst);
    final status = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TransitAccessRow(
          access: directions.transitAccess,
          onChanged: _setTransitAccess,
        ),
        if (!cupertino)
          SizedBox(
            height: 2,
            child: waiting ? const LinearProgressIndicator() : null,
          )
        else if (waiting)
          const SizedBox(height: 28, child: CupertinoActivityIndicator()),
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
                : waiting
                ? 'Finding routes…'
                : _messageFor(directions),
          ),
      ],
    );
    return [
      // The sheet first opens to show the first few journeys, or why there are none.
      if (journeys.isEmpty)
        SliverToBoxAdapter(
          child: SheetSection(id: _opening, child: status),
        )
      else ...[
        SliverToBoxAdapter(child: status),
        SliverToBoxAdapter(
          child: SheetSection(
            id: _opening,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < first; i++) ...[if (i > 0) divider, row(i)],
              ],
            ),
          ),
        ),
        SliverPadding(
          padding: const EdgeInsets.only(bottom: 12),
          sliver: SliverList.builder(
            itemCount: journeys.length - first,
            itemBuilder: (context, i) => Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [divider, row(first + i)],
            ),
          ),
        ),
      ],
    ];
  }

  /// A transit journey's legs in detail, under a header to go back to the journeys,
  /// with the departures chosen of its rides: kept, as far as they can be, when the
  /// journey is planned again.
  List<Widget> _journeySlivers(Directions directions, Journey journey) {
    final units = _units.onDevice;
    final now = DirectionsScope.of(context).clock();
    final chain = _chainOf(journey);
    return [
      SliverToBoxAdapter(
        child: SheetSection(
          id: _journeySummary,
          child: JourneyHeader(
            chain: chain,
            now: now,
            units: units,
            onBack: _closeJourney,
          ),
        ),
      ),
      JourneyItinerary(
        chain: chain,
        fetched: [
          for (final choice in chain.rides)
            _legs!.legOf(choice, directions.transitAccess),
        ],
        onSelect: (ride, departure) {
          setState(() => _chain = chain.select(ride, departure));
          _chainChanged();
        },
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
