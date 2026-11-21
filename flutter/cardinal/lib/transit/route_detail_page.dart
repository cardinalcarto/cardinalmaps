/// A nearby route's detail: one of its pages from its stop, with the next departures as
/// tiles, the selected one's trip from the stop to its end, and that whole trip and the
/// route's vehicles on the map.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/foundation.dart' show ValueListenable, listEquals;
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import 'package:routing/routing.dart' show DirectionsPoint;

import '../directions/transit_details.dart' show RideStops;
import '../directions/transit_plan.dart';
import '../directions/transit_sheet.dart' show clock;
import '../map/map.dart';
import '../map/route_layer.dart';
import '../routes.dart';
import '../settings/accessibility.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import 'countdown.dart';
import 'nearby_page.dart';
import 'nearby_source.dart';
import 'nearby_transit.dart';
import 'route_detail.dart';
import 'vehicle_markers.dart';

/// How often a route's detail asks again while it shows: as often as the list.
const routeDetailRefresh = nearbyRefresh;

/// A nearby route's detail, opened from its row on the page the row showed: [routeId]
/// of [feed] from [stopId], under [headsigns].
///
/// It shows the one page it was opened on: the headsigns the row's page merged, from its
/// stop. The header is that page as the row showed it, without its times and without
/// swiping to the route's other pages. Under it, the page's next departures are tiles,
/// the first catchable selected; tapping another shows its trip below and on the map.
/// When a selected trip can't be loaded, the first catchable departure is selected
/// instead, and its trip shown.
///
/// While it shows, it asks again every [routeDetailRefresh], on the schedule first as
/// the list does, and countdowns tick each second; while it doesn't, it asks nothing.
///
/// [from] is the row's route, with its pages. Without it (when the app restores the
/// page), the page is asked for by the identifiers alone, with no walk to the stop, and
/// shows the one page.
class RouteDetailPage extends StatefulWidget {
  const RouteDetailPage({
    super.key,
    required this.feed,
    required this.routeId,
    required this.stopId,
    required this.headsigns,
    this.from,
  });

  final String feed;
  final String routeId;
  final String stopId;
  final List<String> headsigns;
  final NearbyRoute? from;

  @override
  State<RouteDetailPage> createState() => _RouteDetailPageState();
}

class _RouteDetailPageState extends State<RouteDetailPage> with RouteAware {
  /// All of the page's content, whose end the sheet never goes past.
  static const _content = #routeDetailContent;

  /// As the list's: where the sheet opens and the lowest it rests, and the end of the
  /// content (or the full screen if that comes first), as high as it goes.
  static const _lower = SheetAnchor.fraction(0.4);
  static const _upper = SheetAnchor.section(_content);

  /// Between the two, the sheet rests wherever it's let go, with no anchor at the top
  /// to spring to, and a fling up carries on into scrolling the stops, so reading
  /// through the trip is one long scroll.
  static const _range = SheetRange(_lower, _upper, carriesIntoScroll: true);

  /// As directions fits a route to what the sheet leaves of the map.
  static const _margin = 48.0;
  static const _maxZoom = 17.0;

  late final NearbyTransitSource _source;
  late final DateTime Function() _clock;
  late final VehicleMarkerPainter _markers;
  late final SparkleMapController _map;
  late final SettingsStore _settings;
  late final ValueNotifier<DateTime> _now;
  bool _started = false;
  ModalRoute<Object?>? _modal;

  /// The route: the row's, or else the first answer's.
  NearbyRoute? _route;

  /// The page shown: the row's, or else the first answer's.
  HeadsignPage? _page;

  late final RouteDetailController _controller;

  /// The ride the timeline and the map show, once there is one.
  TransitLeg? _ride;

  /// [_ride]'s trip before the stop.
  List<DirectionsPoint> _shapeToStop = const [];

  /// The ride's run the camera last fitted, and whether it fitted the stop alone.
  TripRun? _fitted;
  bool _fittedStop = false;

  /// Vehicle markers the map has.
  final _painted = <String>{};

  /// Counts drawings of the map: only the latest is set.
  int _draws = 0;

  /// What the map was last given, encoded.
  String? _drawn;

  bool _appShown = true;
  bool _covered = false;
  bool _running = false;
  int _runs = 0;
  TransitAccess? _access;
  Timer? _tick;
  Timer? _refresh;
  StreamSubscription<bool>? _wheelchair;

  late final AppLifecycleListener _lifecycle = AppLifecycleListener(
    onShow: () {
      _appShown = true;
      _update();
    },
    onHide: () {
      _appShown = false;
      _update();
    },
  );

  RouteDetailController _controllerFor(NearbyRoute? route, HeadsignPage? page) {
    final query = route != null && page != null
        ? RouteDetailQuery.of(route, page)
        : RouteDetailQuery(
            feed: widget.feed,
            routeId: widget.routeId,
            headsigns: widget.headsigns,
            stopId: widget.stopId,
            // Unknown without the row.
            walkSeconds: 0,
          );
    return RouteDetailController(
      _source,
      query,
      seed: page?.departures ?? const [],
      clock: _clock,
    )..addListener(_changed);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_started) {
      _started = true;
      final scope = NearbyTransitScope.of(context);
      _source = scope.source;
      _clock = scope.clock;
      _markers = scope.vehicleMarkers;
      _map = MapScope.of(context);
      _settings = StoreScope.of(context).settings;
      _now = ValueNotifier(_clock());
      _route = widget.from;
      _page =
          _route?.pages
              .where(
                (p) =>
                    p.stop.id == widget.stopId &&
                    listEquals(p.headsigns, widget.headsigns),
              )
              .firstOrNull ??
          _route?.pages.firstOrNull;
      _controller = _controllerFor(_route, _page);
      _appShown = switch (WidgetsBinding.instance.lifecycleState) {
        AppLifecycleState.hidden ||
        AppLifecycleState.paused ||
        AppLifecycleState.detached => false,
        _ => true,
      };
      _lifecycle;
      _update();
    }
    final modal = ModalRoute.of(context);
    if (modal != _modal) {
      sheetPages.unsubscribe(this);
      if (modal is PageRoute<Object?>) sheetPages.subscribe(this, modal);
      _modal = modal;
    }
  }

  @override
  void didPushNext() {
    _covered = true;
    _update();
  }

  @override
  void didPopNext() {
    _covered = false;
    _update();
  }

  void _update() {
    final showing = _appShown && !_covered;
    if (showing && !_running) {
      _start();
    } else if (!showing && _running) {
      _stop();
    }
  }

  void _start() {
    _running = true;
    final run = ++_runs;
    _now.value = _clock();
    _controller.keepSelection(_now.value);
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      _now.value = _clock();
      _controller.keepSelection(_now.value);
    });
    _refresh = Timer.periodic(routeDetailRefresh, (_) {
      // A slow answer isn't abandoned for the next.
      if (!_controller.asking) _ask();
    });
    unawaited(_follow(run));
  }

  void _stop() {
    _running = false;
    _runs++;
    _tick?.cancel();
    _refresh?.cancel();
    _wheelchair?.cancel();
    _tick = _refresh = null;
    _wheelchair = null;
    _access = null;
    _controller.abandon();
  }

  /// Follows the wheelchair setting, and asks once it's known.
  Future<void> _follow(int run) async {
    final wheelchair = await _settings.get(wheelchairByDefault);
    if (run != _runs || !mounted) return;
    _access = nearbyAccess(wheelchair: wheelchair);
    _wheelchair = _settings.watch(wheelchairByDefault).listen((wheelchair) {
      final access = nearbyAccess(wheelchair: wheelchair);
      if (access == _access) return;
      _access = access;
      _ask();
    });
    _ask();
  }

  void _ask() {
    final access = _access;
    if (!_running || access == null) return;
    unawaited(
      _controller.refresh(access).catchError((Object e) {
        debugPrint('route detail: $e');
      }),
    );
  }

  void _changed() {
    if (!mounted) return;
    final controller = _controller;
    if (_route == null && controller.detail != null) {
      final route = _route = NearbyRoute.ofDetail(controller.detail!);
      _page = route.pages.first;
    }
    setState(_showRide);
    _drawMap();
  }

  /// The controller's ride, once it has one.
  void _showRide() {
    final controller = _controller;
    if (controller.ride != null) {
      _ride = controller.ride;
      _shapeToStop = controller.shapeToStop;
    } else if (controller.detail != null) {
      _ride = null;
      _shapeToStop = const [];
    }
  }

  /// Sets the map's route, stops, and vehicles to what's shown, drawing the vehicles'
  /// markers first if the map doesn't have them; and fits the camera to a new ride.
  Future<void> _drawMap() async {
    final draw = ++_draws;
    final controller = _controller;
    // The latest answer's colors and names.
    final route = controller.detail?.route ?? _ride?.route ?? _route?.route;
    final ride = _ride;
    final stop = controller.detail?.stop ?? _page?.stop;
    final vehicles = controller.detail?.vehicles ?? const <TransitVehicle>[];
    final marker = vehicleMarkerName(route);
    final heading = vehicleHeadingName(route);
    if (vehicles.isNotEmpty &&
        !(_painted.contains(marker) && _painted.contains(heading))) {
      try {
        final images = await _markers(route);
        if (!mounted) return;
        _map.addImages(images);
        _painted.addAll(images.map((i) => i.name));
      } on Object catch (e) {
        debugPrint('route detail: vehicle markers: $e');
      }
    }
    if (!mounted || draw != _draws) return;
    final data = nearbyRouteGeoJson(
      route: route,
      ride: ride,
      shapeToStop: _shapeToStop,
      boarding: stop,
      vehicles: vehicles,
      vehicleIcon: marker,
      headingIcon: heading,
    );
    // The map lays out a source again when it's set: only when it has changed.
    final encoded = jsonEncode(data);
    if (encoded != _drawn) {
      _drawn = encoded;
      _map.setGeoJson(nearbyRouteSource, data);
    }
    _fit(ride, stop, answered: controller.detail != null);
  }

  /// Fits the camera to [ride]'s line and [stop] when the ride is new; or to the stop
  /// alone once an answer has no ride. The trip before the stop isn't fitted: the rider
  /// can't board it there, and it may run off the screen.
  void _fit(TransitLeg? ride, StopRef? stop, {required bool answered}) {
    final points = <DirectionsPoint>[?stop?.point];
    if (ride != null) {
      final run = rideRun(ride);
      if (run == _fitted && !_fittedStop) return;
      _fitted = run;
      _fittedStop = false;
      points.addAll(ride.shape);
    } else {
      if (!answered || _fittedStop) return;
      _fittedStop = true;
      _fitted = null;
    }
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

  @override
  void dispose() {
    _stop();
    sheetPages.unsubscribe(this);
    _lifecycle.dispose();
    _controller.dispose();
    _now.dispose();
    _draws++;
    _map
      ..setGeoJson(nearbyRouteSource, empty)
      ..clearSubject();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final controller = _controller;
    final route = _route;
    final walkSeconds = controller.query.walkSeconds;
    final ride = _ride;
    final departures = controller.departures;
    return SheetContent(
      showsBottomBar: true,
      collapsible: true,
      anchors: const [_lower],
      freeRanges: const [_range],
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _content,
            child: AnimatedSize(
              duration: const Duration(milliseconds: 250),
              curve: Curves.easeOutCubic,
              alignment: Alignment.topCenter,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _Header(
                    route: route,
                    page: _page,
                    // As the list's rows show the walk.
                    access: _access ?? TransitAccess.walking,
                  ),
                  if (departures.isNotEmpty)
                    DepartureTiles(
                      departures: departures,
                      walkSeconds: walkSeconds,
                      merged:
                          (controller.detail?.headsigns ??
                                  controller.query.headsigns)
                              .length >
                          1,
                      selected: controller.selected,
                      onSelect: controller.select,
                      now: _now,
                    )
                  else if (controller.detail != null)
                    const _Placeholder('No departures in the next 2 hours.')
                  else if (controller.failed)
                    const _Placeholder("Couldn't find departures."),
                  if (controller.failed && controller.detail != null)
                    const _Placeholder("Couldn't refresh departures."),
                  Padding(
                    padding: const EdgeInsets.only(top: 4, bottom: 16),
                    child: AnimatedSwitcher(
                      duration: const Duration(milliseconds: 250),
                      layoutBuilder: (current, previous) => Stack(
                        alignment: Alignment.topCenter,
                        children: [...previous, ?current],
                      ),
                      child: ride == null
                          ? const SizedBox(
                              key: ValueKey('no ride'),
                              width: double.infinity,
                            )
                          : RideStops(
                              key: ValueKey(('ride', rideRun(ride))),
                              ride: ride,
                            ),
                    ),
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

/// The row's badge, headsign, stop, and walk for the page shown, and a close button.
class _Header extends StatelessWidget {
  const _Header({
    required this.route,
    required this.page,
    required this.access,
  });

  final NearbyRoute? route;
  final HeadsignPage? page;
  final TransitAccess access;

  @override
  Widget build(BuildContext context) {
    final route = this.route;
    return Padding(
      padding: const EdgeInsetsDirectional.only(top: 4, end: 4),
      child: Row(
        children: [
          Expanded(
            child: route == null
                ? const _Placeholder('Finding departures…')
                : NearbyHeader(
                    key: const ValueKey('header'),
                    route: route,
                    page: page,
                    access: access,
                  ),
          ),
          AppCloseButton(
            onPressed: () =>
                context.canPop() ? context.pop() : context.go('/transit'),
          ),
        ],
      ),
    );
  }
}

/// A one-line message in place of the tiles or the trip, with room around it.
class _Placeholder extends StatelessWidget {
  const _Placeholder(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 20),
      child: Text(
        text,
        style: theme.textTheme.bodyMedium?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}

/// A page's departures as tiles in a row that scrolls sideways, counting down by [now]:
/// those that have left are hidden.
class DepartureTiles extends StatelessWidget {
  const DepartureTiles({
    super.key,
    required this.departures,
    required this.walkSeconds,
    required this.merged,
    required this.selected,
    required this.onSelect,
    required this.now,
  });

  final List<StopDeparture> departures;

  /// The walk to the stop.
  final int walkSeconds;

  /// Whether the page merges several headsigns, so each tile says its own.
  final bool merged;
  final TripRun? selected;
  final ValueChanged<StopDeparture> onSelect;
  final ValueListenable<DateTime> now;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: now,
    builder: (context, now, _) {
      final shown = [
        for (final d in departures)
          (d, Countdown(d, now: now, walkSeconds: walkSeconds)),
      ].where((e) => !e.$2.departed);
      return SingleChildScrollView(
        key: const ValueKey('tiles'),
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.fromLTRB(12, 4, 12, 8),
        child: IntrinsicHeight(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (final (i, (d, c)) in shown.indexed)
                Padding(
                  key: ValueKey(runOf(d)),
                  padding: EdgeInsetsDirectional.only(start: i == 0 ? 0 : 8),
                  child: DepartureTile(
                    departure: d,
                    countdown: c,
                    merged: merged,
                    selected: runOf(d) == selected,
                    onTap: () => onSelect(d),
                  ),
                ),
            ],
          ),
        ),
      );
    },
  );
}

/// One departure as a tile: its countdown, large, and the clock time it leaves at,
/// with the live mark when its time is real-time; its own headsign when the page merges
/// several; and the route its vehicle goes on as. One that doesn't run is struck
/// through, with a label, and can't be selected; one the walk can't make is dimmed.
class DepartureTile extends StatelessWidget {
  const DepartureTile({
    super.key,
    required this.departure,
    required this.countdown,
    required this.merged,
    required this.selected,
    required this.onTap,
  });

  final StopDeparture departure;
  final Countdown countdown;
  final bool merged;
  final bool selected;
  final VoidCallback onTap;

  /// The narrowest a tile is, and the widest its smaller lines are.
  static const _minWidth = 76.0;
  static const _textWidth = 128.0;
  static const _radius = 12.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final state = countdown.state;
    final struck =
        state == CountdownState.canceled || state == CountdownState.skipped;
    final dimmed = state == CountdownState.unreachable;
    final faded = scheme.onSurface.withValues(alpha: 0.38);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: dimmed ? faded : scheme.onSurfaceVariant,
    );
    final continuesAs = continuesAsOf(departure);
    final continuedName =
        continuesAs?.shortName ?? continuesAs?.headsign ?? continuesAs?.routeId;
    final showsClock = countdown.minutesUntil <= countdownMinutes;
    return Semantics(
      selected: selected,
      button: !struck,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        decoration: BoxDecoration(
          color: scheme.surfaceContainerHigh,
          borderRadius: BorderRadius.circular(_radius),
          border: Border.all(
            width: 2,
            color: selected ? scheme.primary : Colors.transparent,
          ),
        ),
        child: Material(
          type: MaterialType.transparency,
          child: InkWell(
            borderRadius: BorderRadius.circular(_radius - 2),
            onTap: struck ? null : onTap,
            child: ConstrainedBox(
              constraints: const BoxConstraints(minWidth: _minWidth),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    CountdownNumber(
                      countdown,
                      key: const ValueKey('countdown'),
                      numberStyle: theme.textTheme.headlineSmall?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                      unitStyle: theme.textTheme.labelLarge,
                      liveSize: 16,
                      alignment: AlignmentDirectional.centerStart,
                    ),
                    if (showsClock)
                      Text(
                        clock(context, departure.departure),
                        key: const ValueKey('clock'),
                        style: muted?.copyWith(
                          decoration: struck
                              ? TextDecoration.lineThrough
                              : null,
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                    if (merged && departure.headsign != null)
                      ConstrainedBox(
                        constraints: const BoxConstraints(maxWidth: _textWidth),
                        child: Text(
                          departure.headsign!,
                          key: const ValueKey('headsign'),
                          style: theme.textTheme.bodySmall?.copyWith(
                            color: dimmed ? faded : null,
                          ),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                    if (continuedName != null)
                      ConstrainedBox(
                        constraints: const BoxConstraints(maxWidth: _textWidth),
                        child: Text(
                          'continues as $continuedName',
                          key: const ValueKey('continues'),
                          style: muted,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                    if (struck)
                      Text(
                        state == CountdownState.canceled
                            ? 'Canceled'
                            : 'Skipped',
                        key: const ValueKey('canceled'),
                        style: theme.textTheme.labelMedium?.copyWith(
                          color: scheme.onSurfaceVariant,
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
