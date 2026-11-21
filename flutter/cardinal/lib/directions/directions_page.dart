import 'dart:async';
import 'dart:math' as math;
import 'dart:ui' show PlatformDispatcher;

import 'package:flutter/scheduler.dart';

import 'package:flutter/foundation.dart' show listEquals;

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
import 'departure_legs.dart';
import 'directions.dart';
import 'fade_layers.dart';
import 'format.dart';
import 'journey_list.dart';
import 'mode_tabs.dart';
import 'option_row.dart';
import 'route_source.dart';
import 'scrub.dart';
import 'scrub_marker.dart';
import 'scrub_track.dart';
import 'transit_plan.dart';
import 'transit_sheet.dart';
import 'transit_timeline.dart';
import 'trip_buttons.dart';
import 'trip_card.dart';
import 'trip_details_page.dart';

/// How often transit is planned again while its journeys, or one of them in Trip
/// details, show.
const transitRefresh = Duration(seconds: 30);

/// How long after the last finger lifts a newer plan waits before it adds, drops or
/// moves rows, so nothing is tapped as it goes from under the finger.
const structureSettle = Duration(milliseconds: 400);

/// Directions to the place with [to], as a choice among the ways to get there: the
/// trip's two ends, a tab for each way of traveling with its best time, and that way's
/// options as rows, with the routes on the map. The buttons pinned under the rows act
/// on the selected one: Steps and Start on a route, Trip details on a transit journey.
///
/// Every way is asked for as soon as the trip has two ends, and each tab and list
/// fills in when its answer comes. The selected row is held in view wherever the sheet
/// is: it stops above the buttons as the rest of the sheet passes behind it.
///
/// It's showing the trip, or editing one of its ends:
///
/// | In | When | Then |
/// |---|---|---|
/// | opening | | the sheet comes to rest on the header and up to three rows, in no more than 60% of the window; the trip shows nothing until the way last chosen is read, then fades in as that way |
/// | showing the trip | a field is tapped | editing it: full, keyboard up, the field's text selected; the trip (ends, tabs, rows, the held row, buttons) fades out as the search fades in |
/// | editing | a place or My Location is chosen | that's the end; showing the trip, at the header anchor; the ends, tabs, rows and buttons fade in |
/// | editing | back | showing the trip, the same, the end as it was unless it was cleared |
/// | showing the trip | X, or back | the place's card |
/// | showing the trip | an end changes, or the ends are swapped | every way is asked again: placeholders, then its options, the first selected |
/// | showing the trip | another tab is tapped | that way's options, as they were left, fading in over the last way's where each is; the held row and the buttons fade to its own in place; nothing is asked |
/// | a way loading, or the start being found | | three placeholder rows, the first held; its tab's time empty; the buttons disabled |
/// | a way loading | its options arrive | its rows fade in where the placeholders were, the first selected; its tab's time fades in |
/// | a way loading | there's no route or journey, or asking failed | the message in the first row's place, held; a dash on its tab; the buttons disabled |
/// | either end missing | | the message asking for them, in the first row's place, held; the tabs' times empty |
/// | showing a street way | a row, or its callout on the map, is tapped | that route selected: its row filled and held, the map's highlight on it |
/// | showing a street way | Steps | the steps page, pushed; back returns here, the sheet where it was |
/// | showing a street way | Start | navigating the selected route |
/// | showing transit | | over the rows, how live the times are, how stops are reached, and the times along the rows' axis; the first journey selected, and the only one on the map |
/// | showing transit | a journey is tapped | that journey selected: its row filled and held, the map showing it alone |
/// | showing a way's options | a row's graph is dragged sideways, or a screen reader steps along it | scrubbing: the row selected (for transit, its journey drawn where the map is); what was held stays held, so a row cut off by the buttons stays put; a hairline at the finger, the readout in place of when it arrives (or of a journey's times), and the marker on the map, which pans as little as it must when the marker leaves what shows, but not while the map is moved by hand |
/// | scrubbing | the finger lifts, or a second passes after the screen reader's last step | the readout fades back; the row is held, rising into the dock if it's cut off; the marker fades a second later |
/// | showing transit | how stops are reached is chosen from its menu | transit asked again that way: placeholders, then its journeys |
/// | showing transit | Trip details | the selected journey's Trip details, pushed: the journey from its top, the sheet at 40% so the map shows it |
/// | Trip details showing | back | the journeys, the sheet and the list where they were left |
/// | showing transit, or Trip details over it, the app in front | [transitRefresh] since transit last answered | transit planned again; what shows stays as it is until the answer, and if planning fails |
/// | transit planned again | the new plan has the same journeys in the same order | each row's times and timeline ease to the new ones, in place |
/// | transit planned again | the new plan adds, drops or reorders journeys | once no finger is down, and [structureSettle] after: rows grow in, shrink out and slide; the selected journey stays selected and held if it's still there, and the first is selected if not |
/// | Trip details showing, transit planned again | the open journey is in the new plan | once no finger is down: it stays open, its times ease, the departures chosen kept |
/// | Trip details showing, transit planned again | the open journey isn't in the new plan | once no finger is down: back to the journeys |
/// | Trip details showing | transit is asked again (an end, or how stops are reached, changed) | back to the journeys |
///
/// The sheet rests at two anchors, each with the held row above the buttons: with
/// only the handle over it, and with the ends and tabs over it. From the second up it
/// rests wherever it's let go. The rows, the placeholders and the message are one
/// height in every way, and so are the buttons, so the anchors are the same in every
/// way and state.
class DirectionsPage extends StatefulWidget {
  const DirectionsPage({super.key, required this.to});

  /// The ID of the place to go to.
  final String to;

  @override
  State<DirectionsPage> createState() => _DirectionsPageState();
}

class _DirectionsPageState extends State<DirectionsPage>
    with TickerProviderStateMixin {
  /// The trip's ends and the tabs.
  static const _headerSection = #directionsHeader;

  /// Ends under the last row the sheet first opens to show.
  static const _opening = #directionsOpening;

  /// How often the times that follow the clock are shown again: when a route
  /// arrives, how long until a journey does, and "Go in".
  static const _tick = Duration(seconds: 30);

  /// With the ends and tabs showing, over the held row. Flings always stop here, on
  /// the way up or down.
  static const _headerAnchor = SheetAnchor.section(
    _headerSection,
    gravity: double.infinity,
  );

  /// From there up the sheet rests wherever it's let go, with no anchor at the top to
  /// spring to, and a fling up carries on into scrolling the list.
  static const _tripRange = SheetRange(
    _headerAnchor,
    SheetAnchor.full(),
    carriesIntoScroll: true,
  );

  /// Space kept around the routes, in logical pixels.
  static const _margin = 48.0;

  /// The closest the map moves in to show a short trip.
  static const _maxZoom = 17.0;

  /// Transit's list, among the lists of [OptionList]s' slots.
  static const _transitGroup = #transit;

  /// How long the map has to be still for the callouts to move.
  static const _calloutDelay = Duration(milliseconds: 150);

  /// How far a callout's point stays from the edge of what's showing, in logical pixels:
  /// enough for the bubble, which sits above the point.
  static const _calloutMargin = EdgeInsets.fromLTRB(44, 40, 44, 4);

  /// How far the scrub marker is kept from the edge of what's showing of the map, in
  /// logical pixels, before the map pans to keep it in view.
  static const _scrubMargin = EdgeInsets.all(32);

  /// How long the map pans for to bring the scrub marker back into view.
  static const _scrubPan = Duration(milliseconds: 300);

  /// How long after the last finger lifts from the map, and the map stops moving, it's
  /// taken to be done with the gesture.
  static const _mapSettle = Duration(milliseconds: 300);

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

  /// Whether the way last chosen has been read. Until it has, the trip is laid out,
  /// for the sheet to come to rest on, but none of it shows, so the page is never
  /// seen in one way and then another.
  bool _ready = false;

  /// Whether the start has been looked for too, so the first questions have been
  /// asked if they could be.
  bool _resumed = false;

  /// What shows of the trip, which keeps its state as it goes from the sheet's
  /// content to fading out over the search for an end, and back.
  final _tripKey = GlobalKey();

  /// How far the trip was scrolled when an end was last tapped, for it to fade out
  /// where it was.
  double _tripScrolled = 0;

  /// Whether the app is in front.
  bool _appShown = true;
  late final AppLifecycleListener _lifecycle = AppLifecycleListener(
    onShow: () {
      _appShown = true;
      _scheduleRefresh();
    },
    onHide: () {
      _appShown = false;
      _scheduleRefresh();
    },
  );

  /// Plans transit again when it runs out.
  Timer? _refreshTimer;

  /// Transit's plan and whether it was loading, as last seen, and when it last
  /// answered.
  TransitPlan? _transitPlan;
  bool _transitLoading = false;
  DateTime? _transitAnswered;

  /// The fingers down anywhere in the window.
  final _pointers = <int>{};

  /// The fingers that went down on the map, out from under the sheet, and the time
  /// after the last lifted until the map is still.
  final _mapPointers = <int>{};
  Timer? _mapSettling;

  /// Whether the map is being moved by hand, or still moving from it.
  bool get _mapTouched => _mapPointers.isNotEmpty || _mapSettling != null;

  /// The row being scrubbed, and the slot that was held when the scrub started, which
  /// stays held until it ends: a row selected by a scrub rises into the dock only once
  /// the finger lifts, as one that's tapped does.
  Object? _scrubbing;
  Object? _scrubHeld;

  /// The slot held as the page was last built.
  Object? _held;

  late final _scrubSink = ScrubSink(
    onStart: _scrubStarted,
    onMove: _scrubMoved,
    onEnd: _scrubEnded,
  );

  late final _marker = ScrubMarker(
    map: _map,
    vsync: this,
    onPlaced: _keepInView,
  );

  /// Runs for [structureSettle] after the last of them lifts.
  Timer? _settling;

  /// How much the trip shows, with its rows and buttons: none of it until the way
  /// last chosen is read, then all, but not while an end is edited.
  late final _chrome = AnimationController(
    vsync: this,
    value: 0,
    duration: const Duration(milliseconds: 200),
  )..addStatusListener((_) => setState(() {}));

  final _text = TextEditingController();
  final _focus = FocusNode(debugLabel: 'directions');

  /// The end being edited, if one is.
  RouteField? _editing;

  /// The transit journey the map shows, and which it is of which question's plans:
  /// the map moves to show another journey, not the same one planned again.
  Journey? _shownJourney;
  (int, JourneyKey)? _shownKey;

  /// Which journey is open in Trip details, of which plan, of which of the times
  /// transit has been asked.
  JourneyKey? _openKey;
  TransitPlan? _openPlan;
  int _openQuestion = 0;

  /// The journey open in Trip details, if one is, with the departures chosen of its
  /// rides and the legs of those other than the plan's, for the map to draw.
  late final OpenJourney _open;

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
    _open = OpenJourney(
      directions: directions,
      legs: DepartureLegs(scope.transit),
    )..addListener(_chainChanged);
    // The map draws one that's arrived along its own line.
    _open.legs.addListener(_drawRoutes);
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
      _open.units = units;
      if (units != _units) setState(() => _units = units);
    });
    _taps = routeTaps.stream.listen(directions.select);
    _appShown = switch (WidgetsBinding.instance.lifecycleState) {
      AppLifecycleState.hidden ||
      AppLifecycleState.paused ||
      AppLifecycleState.detached => false,
      _ => true,
    };
    _lifecycle;
    WidgetsBinding.instance.pointerRouter.addGlobalRoute(_pointer);
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
  ///
  /// The three settings are read at once, and the trip shows only once they're known,
  /// in the way last chosen from the first.
  Future<void> _resume(Directions directions) async {
    final settings = _settings!;
    final (wheelchair, access, mode) = await (
      settings.get(wheelchairByDefault),
      settings.get(lastTransitAccess),
      settings.get(lastTravelMode),
    ).wait;
    if (!mounted) return;
    _rememberedAccess = access;
    directions
      ..setWheelchair(wheelchair)
      ..setTransitAccess(transitAccess(access, wheelchair: wheelchair))
      ..setMode(mode);
    setState(() => _ready = true);
    _chrome.forward();
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
    _refreshTimer?.cancel();
    _settling?.cancel();
    _mapSettling?.cancel();
    _marker.dispose();
    if (_directions != null) {
      _lifecycle.dispose();
      WidgetsBinding.instance.pointerRouter.removeGlobalRoute(_pointer);
    }
    _map.cameraMoved.removeListener(_viewChanged);
    _sheet?.removeListener(_viewChanged);
    _navigation?.removeListener(_navigationChanged);
    _map
      ..setGeoJson(routeSource, empty)
      ..setGeoJson(routeCalloutSource, empty)
      ..setGeoJson(transitSource, empty)
      ..setGeoJson(scrubSource, empty);
    _directions?.dispose();
    _open.dispose();
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
      _marker.hide();
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
    // Showing a newer plan tells this again, with it.
    if (_showRefreshed()) return;
    _followTransit();
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
    _followOpen();
    final journey = directions.journey;
    if (!identical(journey, _shownJourney)) {
      _shownJourney = journey;
      final key = switch (directions.plan) {
        final plan? when journey != null => (
          directions.transitQuestion,
          journeyKeys(plan.journeys)[directions.selectedJourney],
        ),
        _ => null,
      };
      // The same journey planned again is drawn again, where the map is; one a scrub
      // selects is drawn where the map is too, which pans only to follow the marker.
      if (key != _shownKey && _scrubbing == null) _showJourney();
      _shownKey = key;
    }
    _followChain();
    _drawRoutes();
    _placeCallouts();
    setState(() {});
  }

  /// Keeps Trip details on its journey as transit's plan changes: open, on the
  /// journey as a newer plan of the same trip has it, with the departures chosen, or
  /// closed if that plan doesn't have it or the trip is another.
  void _followOpen() {
    if (!_open.isOpen) return;
    final directions = _directions!;
    final plan = directions.plan;
    if (plan == null || directions.transitQuestion != _openQuestion) {
      _open.close();
      return;
    }
    if (identical(plan, _openPlan)) return;
    final index = journeyKeys(plan.journeys).indexOf(_openKey!);
    if (index < 0) {
      _open.close();
      return;
    }
    _openPlan = plan;
    _open.rebase(plan.journeys[index]);
  }

  /// Whether transit's journeys, or one of them in Trip details, are showing, with
  /// the app in front.
  bool get _transitShowing {
    final directions = _directions!;
    return _ready &&
        _appShown &&
        _editing == null &&
        directions.mode == TravelMode.transit &&
        directions.from != null &&
        directions.to != null;
  }

  /// Notes when transit answers, and has it planned again in time.
  void _followTransit() {
    final options = _directions!.options(TravelMode.transit);
    if (!options.loading &&
        (_transitLoading || !identical(options.plan, _transitPlan))) {
      _transitAnswered = DirectionsScope.of(context).clock();
    }
    _transitLoading = options.loading;
    _transitPlan = options.plan;
    _scheduleRefresh();
  }

  /// Sets transit to be planned again [transitRefresh] after it last answered, or at
  /// once if that's passed, while it's showing; and not to be while it isn't.
  void _scheduleRefresh() {
    final directions = _directions;
    if (directions == null) return;
    if (!_transitShowing || directions.options(TravelMode.transit).loading) {
      _refreshTimer?.cancel();
      _refreshTimer = null;
      return;
    }
    if (_refreshTimer != null || directions.refreshing) return;
    final since = switch (_transitAnswered) {
      final answered? => DirectionsScope.of(
        context,
      ).clock().difference(answered),
      null => transitRefresh,
    };
    final wait = transitRefresh - since;
    _refreshTimer = Timer(wait.isNegative ? Duration.zero : wait, _refresh);
  }

  /// Plans transit again. A slow answer isn't given up for the next; one that fails
  /// leaves what's showing, and the next is asked for in its turn.
  Future<void> _refresh() async {
    _refreshTimer = null;
    await _directions!.refreshTransit();
    if (!mounted) return;
    _transitAnswered = DirectionsScope.of(context).clock();
    _scheduleRefresh();
  }

  /// Whether a finger is down, or one lifted within [structureSettle].
  bool get _touched => _pointers.isNotEmpty || _settling != null;

  /// Whether showing [fresh] in place of transit's plan would add, drop or move
  /// anything that shows: its journeys aren't the same ones in the same order, or one
  /// is open in Trip details, where a ride's departures and rows may differ.
  bool _restructures(TransitPlan fresh) {
    final directions = _directions!;
    if (directions.mode != TravelMode.transit) return false;
    final plan = directions.plan;
    if (plan == null || _open.isOpen) return true;
    return !listEquals(journeyKeys(plan.journeys), journeyKeys(fresh.journeys));
  }

  /// Shows the newer plan of transit that's waiting, if there is one and nothing
  /// would move under a finger; returns whether it did.
  bool _showRefreshed() {
    final directions = _directions!;
    final fresh = directions.refreshed;
    if (fresh == null || (_touched && _restructures(fresh))) return false;
    directions.showRefreshed();
    return true;
  }

  /// A finger went down or lifted, anywhere: a newer plan that would move rows waits
  /// until none is down, and a little longer.
  ///
  /// It notes too which fingers are on the map, for the scrub marker not to pan it
  /// while it's moved by hand.
  void _pointer(PointerEvent event) {
    if (event is PointerDownEvent) {
      _pointers.add(event.pointer);
      _settling?.cancel();
      _settling = null;
      if (mounted && _onMap(event.position)) {
        _mapPointers.add(event.pointer);
        _mapSettling?.cancel();
        _mapSettling = null;
      }
    } else if (event is PointerUpEvent || event is PointerCancelEvent) {
      _pointers.remove(event.pointer);
      if (_mapPointers.remove(event.pointer) && _mapPointers.isEmpty) {
        _settleMap();
      }
      if (_pointers.isNotEmpty) return;
      _settling?.cancel();
      _settling = Timer(structureSettle, () {
        _settling = null;
        if (mounted) _showRefreshed();
      });
    }
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

  /// Has the legs fetched of the departures the open journey takes but the plan's own,
  /// and no others.
  void _followChain() {
    final directions = _directions!;
    final plan = directions.plan;
    if (plan == null) return;
    _open.legs.follow(plan, _open.chain, directions.transitAccess);
  }

  /// How the map draws each ride of the open journey that takes another departure than
  /// the plan's, by its index in the journey's legs: along the departure's own line once
  /// it's fetched, and until then along the plan's, which is between the same two stops,
  /// in the departure's route's colors.
  Map<int, DrawnRide> _chosenRides() {
    final journey = _shownJourney;
    final chain = _open.chain;
    if (journey == null ||
        chain == null ||
        !identical(chain.journey, journey)) {
      return const {};
    }
    final access = _directions!.transitAccess;
    return {
      for (final (i, leg) in journey.legs.indexed)
        if (chain.forLeg(i) case final choice?
            when choice.made && !choice.suggested)
          i: switch (_open.legs.legOf(choice, access)) {
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

  /// Whether [position], in the window, is on the map: not under the sheet or the
  /// status bar.
  bool _onMap(Offset position) {
    final view = SheetScaffold.viewOf(context);
    final clear =
        EdgeInsets.only(top: MediaQuery.paddingOf(context).top) + view.obscured;
    return clear
        .deflateRect(Offset.zero & MediaQuery.sizeOf(context))
        .contains(position);
  }

  /// The last finger lifted from the map, or the map moved since: it's done with the
  /// gesture once it's been still for [_mapSettle].
  void _settleMap() {
    _mapSettling?.cancel();
    _mapSettling = Timer(_mapSettle, () => _mapSettling = null);
  }

  /// The map or the sheet moved: once they're still, the callouts follow.
  void _viewChanged() {
    if (_mapSettling != null) _settleMap();
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
      final outline = _visibleOutline(_calloutMargin);
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

  /// The ground showing outside the sheet and the status bar, [margin] in from its
  /// edges, or null if it can't be told: the sheet covers the map, or the view reaches
  /// past the horizon.
  List<({double lng, double lat})>? _visibleOutline(EdgeInsets margin) {
    final view = SheetScaffold.viewOf(context);
    final clear =
        EdgeInsets.only(top: MediaQuery.paddingOf(context).top) + view.obscured;
    final area = margin.deflateRect(
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

  /// A row's graph is scrubbed: it's selected, but whatever was held stays held until
  /// the scrub ends, so a row cut off by the buttons stays where it is under the
  /// finger.
  void _scrubStarted(Object row, VoidCallback select) {
    if (_scrubbing == null) _scrubHeld = _held;
    _scrubbing = row;
    select();
  }

  void _scrubMoved(Object row, DirectionsPoint? point) {
    if (row != _scrubbing || _navigating) return;
    _marker.move(point);
  }

  /// The scrub ended: the row it selected is held, rising into the dock if it's cut
  /// off, and the marker fades a while after.
  void _scrubEnded(Object row) {
    if (row != _scrubbing) return;
    _scrubbing = null;
    _marker.release();
    // A row that goes takes its scrub with it, as the tree is being finalized.
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && _scrubbing == null) setState(() => _scrubHeld = null);
      });
    } else {
      setState(() => _scrubHeld = null);
    }
  }

  /// Pans the map, as little as it can, for [point], where the scrub marker is, to
  /// be [_scrubMargin] inside what shows of it, if it isn't; not while the map is
  /// moved by hand. The zoom stays as it is.
  void _keepInView(DirectionsPoint point) {
    if (!mounted || _navigating || _mapTouched) return;
    final outline = _visibleOutline(_scrubMargin);
    if (outline == null) return;
    final shift = panInto(outline, point.lng, point.lat);
    final camera = _map.camera;
    if (shift == null || camera == null) return;
    _map.easeTo(
      lng: camera.lng + shift.lng,
      lat: camera.lat + shift.lat,
      zoom: camera.zoom,
      duration: _scrubPan,
      curve: EaseCurve.easeOut,
    );
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

  /// Opens the selected journey's Trip details over this page, which bring the sheet
  /// low enough for the map to show it.
  void _openDetails() {
    final directions = _directions!;
    final plan = directions.plan;
    final journey = directions.journey;
    if (plan == null || journey == null) return;
    _openKey = journeyKeys(plan.journeys)[directions.selectedJourney];
    _openPlan = plan;
    _openQuestion = directions.transitQuestion;
    _open.open(journey);
    unawaited(context.push(tripDetailsLocation(widget.to), extra: _open));
  }

  /// The journey open in Trip details, or the departures chosen of it, changed: fetch
  /// the legs they need, and draw them.
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
    // The trip fades out where it is, however far it's scrolled.
    _tripScrolled = switch (_tripKey.currentContext) {
      final trip? => Scrollable.maybeOf(trip)?.position.pixels ?? 0,
      null => 0,
    };
    setState(() => _editing = field);
    _chrome.reverse();
    _focus.requestFocus();
    _sheet!.animateTo(const SheetAnchor.full());
    _scheduleRefresh();
  }

  /// Back to showing the trip, with the ends and tabs over the held row.
  void _stopEditing() {
    if (_editing == null) return;
    setState(() => _editing = null);
    _chrome.forward();
    _focus.unfocus();
    _sheet!.animateTo(_headerAnchor);
    _scheduleRefresh();
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

  /// Fades the search for an end in as the trip fades out.
  late final Animation<double> _searching = ReverseAnimation(_chrome);

  @override
  Widget build(BuildContext context) {
    final directions = _directions!;
    final editing = _editing;
    // The trip outlasts itself by as long as it takes to fade when an end is edited.
    final trip = editing == null || !_chrome.isDismissed
        ? _trip(directions)
        : null;
    final transit = directions.mode == TravelMode.transit;
    _held = _scrubHeld ?? trip?.held;
    return PopScope(
      canPop: editing == null,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _stopEditing();
      },
      child: SheetContent(
        anchors: editing != null
            ? const [SheetAnchor.full()]
            : const [SheetAnchor.footer(), _headerAnchor],
        freeRanges: editing != null ? const [] : const [_tripRange],
        initialAnchor: 1,
        initialSection: _opening,
        initialMaxFraction: 0.6,
        // The trip and editing each scroll on their own: the list is where it was
        // left on the way back from editing.
        scrollId: editing != null ? 'editing' : 'trip',
        held: _held,
        footer: trip == null
            ? null
            : FadeTransition(
                opacity: _chrome,
                child: IgnorePointer(
                  ignoring: editing != null,
                  // One way's buttons fade into another's, the same height.
                  child: AnimatedSwitcher(
                    key: ValueKey(_ready),
                    duration: const Duration(milliseconds: 200),
                    switchInCurve: Curves.easeOut,
                    switchOutCurve: Curves.easeOut,
                    layoutBuilder: (current, previous) => Stack(
                      fit: StackFit.passthrough,
                      children: [...previous, ?current],
                    ),
                    child: transit
                        ? TripButtons.details(
                            key: const ValueKey(#details),
                            onDetails:
                                directions.journey == null ||
                                    directions.loading ||
                                    _waiting(directions)
                                ? null
                                : _openDetails,
                          )
                        : TripButtons(
                            key: const ValueKey(#street),
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
                ),
              ),
        slivers: editing != null
            ? _editingSlivers(editing, trip: trip?.content)
            : [if (trip != null) SliverToBoxAdapter(child: trip.content)],
      ),
    );
  }

  /// What shows of the trip, and the slot of it the sheet holds: the ends and tabs,
  /// and under them the rows of the way that's showing.
  ///
  /// One way's rows fade into another's where each is, as transit's start lower,
  /// under its line; and within transit, its placeholders or its message into its
  /// journeys. Each of those is a layer that fades itself, inside its slots, as the
  /// sheet paints a held row apart from the rest.
  ({Widget content, Object held}) _trip(Directions directions) {
    final now = DirectionsScope.of(context).clock();
    final units = _units.onDevice;
    final waiting = directions.loading || _waiting(directions);
    final Object held;
    final LayerBuilder rows;
    if (directions.mode != TravelMode.transit) {
      final routes = directions.routes;
      final selected = directions.selected;
      final message = _messageFor(directions);
      final driving = directions.mode == TravelMode.driving;
      held = OptionList.slotId(waiting || routes.isEmpty ? 0 : selected);
      rows = (shown, {required leaving}) => OptionList(
        routes: routes,
        selected: selected,
        loading: waiting,
        message: message,
        driving: driving,
        units: units,
        now: now,
        opening: leaving ? null : _opening,
        reveal: _Product(shown, _chrome),
        onSelect: directions.select,
        scrub: leaving ? null : _scrubSink,
      );
    } else {
      final plan = directions.plan;
      final journeys = waiting
          ? const <Journey>[]
          : plan?.journeys ?? const <Journey>[];
      final keys = journeyKeys(journeys);
      final selected = directions.selectedJourney;
      final axis = TimelineAxis.of(journeys);
      final access = directions.transitAccess;
      final message = plan != null && directions.problem == null
          ? noJourneysMessage(plan, access)
          : _messageFor(directions);
      final listed = journeys.isNotEmpty;
      held = listed
          ? JourneyRows.slotId(keys[selected])
          : OptionList.slotId(0, group: _transitGroup);
      rows = (shown, {required leaving}) {
        final transitLeaving = leaving;
        final reveal = _Product(shown, _chrome);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            FadeTransition(
              opacity: reveal,
              child: TransitStrip(
                plan: listed ? plan : null,
                axis: listed ? axis : null,
                access: access,
                onAccess: _setTransitAccess,
              ),
            ),
            FadeLayers(
              layer: listed ? #journeys : #waiting,
              builder: (shown, {required leaving}) {
                final opening = leaving || transitLeaving ? null : _opening;
                final within = _Product(reveal, shown);
                return listed
                    ? JourneyRows(
                        journeys: journeys,
                        keys: keys,
                        selected: selected,
                        axis: axis,
                        now: now,
                        opening: opening,
                        reveal: within,
                        onSelect: directions.selectJourney,
                        scrub: leaving || transitLeaving ? null : _scrubSink,
                      )
                    : OptionList(
                        group: _transitGroup,
                        routes: const [],
                        selected: 0,
                        loading: waiting,
                        message: message,
                        driving: false,
                        units: units,
                        now: now,
                        opening: opening,
                        reveal: within,
                        onSelect: (_) {},
                      );
              },
            ),
          ],
        );
      };
    }
    return (
      held: held,
      content: Column(
        key: _tripKey,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          FadeTransition(opacity: _chrome, child: _header(directions, now)),
          FadeLayers(
            // What was laid out before the way last chosen was read isn't one way
            // fading into another: it never showed.
            key: ValueKey(_ready),
            layer: directions.mode == TravelMode.transit ? #transit : #street,
            builder: rows,
          ),
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
  Widget _header(Directions directions, DateTime now) => SheetSection(
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
          // The tab last chosen is selected from the first it shows, not moved to.
          key: ValueKey(_ready),
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
  );

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

  /// The search for the end [field], fading in; and over it [trip], what showed of the
  /// trip, fading out where it was, for as long as that takes.
  List<Widget> _editingSlivers(RouteField field, {required Widget? trip}) {
    final search = _search!;
    final input = _endInput(field, search);
    return [
      SliverToBoxAdapter(
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            FadeTransition(opacity: _searching, child: input),
            if (trip != null)
              Positioned(
                top: -_tripScrolled,
                left: 0,
                right: 0,
                child: IgnorePointer(child: ExcludeSemantics(child: trip)),
              ),
          ],
        ),
      ),
      SliverFadeTransition(
        opacity: _searching,
        sliver: ListenableBuilder(
          listenable: Listenable.merge([search, _directions!]),
          builder: (context, _) => _EndChoices(
            search: search,
            saved: _saved,
            locating: _directions!.locating,
            onMyLocation: _useMyLocation,
            onPlace: _choose,
          ),
        ),
      ),
    ];
  }

  /// The field the end [field] is typed in.
  Widget _endInput(RouteField field, PlaceSearch search) {
    return Padding(
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
    );
  }
}

/// Two animations' values multiplied: how much shows of what fades for two reasons.
class _Product extends CompoundAnimation<double> {
  _Product(Animation<double> first, Animation<double> next)
    : super(first: first, next: next);

  @override
  double get value => first.value * next.value;
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
