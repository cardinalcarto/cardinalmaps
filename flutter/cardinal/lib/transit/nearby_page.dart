/// The Nearby transit tab: the routes a rider where they are can catch, soonest first,
/// each with its next departures counting down.
library;

import 'dart:async';

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:material_ui/material_ui.dart';

import '../directions/format.dart';
import '../directions/route_source.dart';
import '../directions/transit_details.dart' show RouteBadge;
import '../directions/transit_plan.dart';
import '../directions/transit_sheet.dart' show accessIcon, liveColor;
import '../map/location.dart';
import '../routes.dart';
import '../settings/accessibility.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import 'countdown.dart';
import 'nearby_rows.dart';
import 'nearby_source.dart';
import 'nearby_transit.dart';

/// How often the list asks again while it shows.
///
/// An assumption, still to be agreed.
const nearbyRefresh = Duration(seconds: 30);

/// Provides where nearby departures come from, and the clock they count down by, to the
/// widgets below it.
class NearbyTransitScope extends InheritedWidget {
  const NearbyTransitScope({
    super.key,
    required this.source,
    this.clock = DateTime.now,
    required super.child,
  });

  final NearbyTransitSource source;

  /// What time it is: the device's clock, or a test's.
  final DateTime Function() clock;

  static NearbyTransitScope of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<NearbyTransitScope>()!;

  @override
  bool updateShouldNotify(NearbyTransitScope oldWidget) =>
      source != oldWidget.source || clock != oldWidget.clock;
}

/// A direction's departures at one moment, as a row shows them: the next that runs, and
/// the rest after it.
class NearbyTimes {
  NearbyTimes._(this.next, this.following);

  /// [direction]'s departures at [now]. Those that have left are left out. The first
  /// that runs is [next], even if the walk can't make it any more; canceled and skipped
  /// ones are never [next], and are among [following] in their place.
  factory NearbyTimes.of(HeadsignPage direction, DateTime now) {
    final left = [
      for (final d in direction.departures)
        Countdown(d, now: now, walkSeconds: direction.walkSeconds),
    ].where((c) => !c.departed);
    Countdown? next;
    for (final c in left) {
      if (c.status.runs) {
        next = c;
        break;
      }
    }
    return NearbyTimes._(next, [
      for (final c in left)
        if (!identical(c, next)) c,
    ]);
  }

  /// The countdown, or null when nothing that runs is left.
  final Countdown? next;
  final List<Countdown> following;
}

/// The Nearby transit tab: one row per route near where you are, in the router's order,
/// which stays put while the list shows.
///
/// While it shows (the app is in front and no page covers it), it asks from your
/// location when it has one, again every [nearbyRefresh], and at once when wheelchair
/// access is turned on or off; countdowns tick each second in between. While it doesn't
/// show, it asks nothing and forgets the rows, so the next answer sorts them afresh.
///
/// It never asks for location permission; without it, it says so.
class NearbyTransitPage extends StatefulWidget {
  const NearbyTransitPage({super.key});

  @override
  State<NearbyTransitPage> createState() => _NearbyTransitPageState();
}

/// Why there are no rows to show.
// Placeholders until the states are agreed.
enum _Note {
  locating('Finding your location…'),
  noLocation('Turn on location to see transit near you.'),
  loading('Finding departures near you…'),
  noArea('Download an area to see transit near you.'),
  outside("You're outside your offline areas."),
  failed("Couldn't find departures."),
  none('No departures nearby.');

  const _Note(this.text);
  final String text;
}

class _NearbyTransitPageState extends State<NearbyTransitPage> with RouteAware {
  static const _header = #nearbyTransitHeader;

  NearbyTransitController? _controller;
  late LocationSource _location;
  late SettingsStore _settings;
  late DateTime Function() _clock;
  late final ValueNotifier<DateTime> _now;
  ModalRoute<Object?>? _route;

  /// Whether the app is in front.
  bool _appShown = true;

  /// Whether a page covers this one.
  bool _covered = false;

  /// Whether it's asking and ticking: while it shows.
  bool _running = false;

  /// Counts starts and stops: what's awaited on behalf of an earlier start is dropped.
  int _runs = 0;

  /// Counts questions: only the latest one's outcome counts.
  int _asks = 0;
  bool _asking = false;

  LngLat? _here;
  TransitAccess? _access;
  _Note _note = _Note.locating;

  Timer? _tick;
  Timer? _refresh;
  StreamSubscription<Fix>? _positions;
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

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_controller == null) {
      final scope = NearbyTransitScope.of(context);
      _clock = scope.clock;
      _now = ValueNotifier(_clock());
      _location = DirectionsScope.of(context).location;
      _settings = StoreScope.of(context).settings;
      _controller = NearbyTransitController(scope.source)
        ..addListener(_changed);
      _appShown = switch (WidgetsBinding.instance.lifecycleState) {
        AppLifecycleState.hidden ||
        AppLifecycleState.paused ||
        AppLifecycleState.detached => false,
        _ => true,
      };
      _lifecycle;
      _update();
    }
    final route = ModalRoute.of(context);
    if (route != _route) {
      sheetPages.unsubscribe(this);
      if (route is PageRoute<Object?>) sheetPages.subscribe(this, route);
      _route = route;
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

  void _changed() {
    if (mounted) setState(() {});
  }

  /// Starts or stops as the page comes to show or stops showing.
  void _update() {
    final showing = _appShown && !_covered;
    if (showing && !_running) {
      _start();
    } else if (!showing && _running) {
      _stop();
      _controller!.reset();
    }
  }

  void _start() {
    _running = true;
    final run = ++_runs;
    _controller!.source.prepare();
    _now.value = _clock();
    _tick = Timer.periodic(
      const Duration(seconds: 1),
      (_) => _now.value = _clock(),
    );
    _refresh = Timer.periodic(nearbyRefresh, (_) {
      // A slow answer isn't abandoned for the next.
      if (!_asking) _ask();
    });
    unawaited(_follow(run));
  }

  void _stop() {
    _running = false;
    _runs++;
    _asks++;
    _asking = false;
    _tick?.cancel();
    _refresh?.cancel();
    _positions?.cancel();
    _wheelchair?.cancel();
    _tick = _refresh = null;
    _positions = null;
    _wheelchair = null;
    _here = null;
    _access = null;
    _note = _Note.locating;
  }

  /// Follows the wheelchair setting and where you are, and asks once both are known.
  Future<void> _follow(int run) async {
    final wheelchair = await _settings.get(wheelchairByDefault);
    if (run != _runs) return;
    _access = nearbyAccess(wheelchair: wheelchair);
    _wheelchair = _settings.watch(wheelchairByDefault).listen((wheelchair) {
      final access = nearbyAccess(wheelchair: wheelchair);
      if (access == _access) return;
      _access = access;
      _ask();
    });
    bool allowed;
    try {
      allowed =
          locationAllowed(await _location.checkPermission()) &&
          await _location.serviceEnabled();
    } on Object {
      // A platform without a location service.
      allowed = false;
    }
    if (run != _runs) return;
    if (!allowed) {
      setState(() => _note = _Note.noLocation);
      return;
    }
    _positions = _location.positions().listen(_moved, onError: (Object _) {});
    Fix? fix;
    try {
      fix = await _location.current();
    } on Object {
      fix = null;
    }
    if (run != _runs || fix == null) return;
    _moved(fix);
  }

  /// Keeps where you are for the next question, and asks if it's the first.
  void _moved(Fix fix) {
    final first = _here == null;
    _here = LngLat(fix.lng, fix.lat);
    if (first) _ask();
  }

  Future<void> _ask() async {
    final here = _here;
    final access = _access;
    if (!_running || here == null || access == null) return;
    final ask = ++_asks;
    _asking = true;
    if (_controller!.rows.latest == null) setState(() => _note = _Note.loading);
    final watch = Stopwatch()..start();
    _Note? failed;
    try {
      await _controller!.refresh(origin: here, access: access);
    } on OutsideOfflineArea {
      failed = _Note.outside;
    } on NoOfflineArea {
      failed = _Note.noArea;
    } on Object catch (e) {
      debugPrint('nearby transit: $e');
      failed = _Note.failed;
    }
    if (ask != _asks || !mounted) return;
    _asking = false;
    final rows = _controller!.rows;
    debugPrint(
      'cardinal-timing: nearby ${access.mode} '
      '${watch.elapsedMilliseconds} ms '
      '(${failed?.name ?? '${rows.latest?.routes.length ?? 0} routes'})',
    );
    // A failed refresh leaves the rows there were.
    setState(() => _note = failed ?? _Note.none);
  }

  @override
  void dispose() {
    _stop();
    sheetPages.unsubscribe(this);
    _lifecycle.dispose();
    _controller?.dispose();
    _now.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final routes = _controller!.rows.routes;
    final access = _controller!.rows.sortedBy ?? TransitAccess.walking;
    return SheetContent(
      showsBottomBar: true,
      anchors: const [
        SheetAnchor.section(_header),
        SheetAnchor.fraction(0.5),
        SheetAnchor.full(),
      ],
      initialAnchor: 1,
      slivers: [
        const SliverToBoxAdapter(
          child: SheetSection(id: _header, child: _Header()),
        ),
        SliverToBoxAdapter(
          child: routes.isEmpty
              ? _Placeholder(_note)
              : Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: AppListSection(
                    children: [
                      for (final route in routes)
                        NearbyRow(
                          key: ValueKey(route.key),
                          route: route,
                          access: access,
                          now: _now,
                        ),
                    ],
                  ),
                ),
        ),
      ],
    );
  }
}

class _Header extends StatelessWidget {
  const _Header();

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsetsDirectional.only(
      start: 20,
      end: 12,
      top: 4,
      bottom: 4,
    ),
    child: ConstrainedBox(
      constraints: const BoxConstraints(minHeight: 48),
      child: Align(
        alignment: AlignmentDirectional.centerStart,
        child: Text(
          'Nearby transit',
          style: Theme.of(context).textTheme.titleLarge,
        ),
      ),
    ),
  );
}

// A placeholder until the states are agreed.
class _Placeholder extends StatelessWidget {
  const _Placeholder(this.note);

  final _Note note;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
      child: Text(
        note.text,
        style: theme.textTheme.bodyMedium?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}

/// One route near you, in its first direction: its badge and where it goes, the stop and
/// the walk there, and its next departures counting down by [now].
class NearbyRow extends StatelessWidget {
  const NearbyRow({
    super.key,
    required this.route,
    required this.access,
    required this.now,
  });

  final NearbyRoute route;

  /// How the walk to the stop goes.
  final TransitAccess access;
  final ValueListenable<DateTime> now;

  /// The most the departures take of the row's width.
  static const _timesWidth = 160.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final direction = route.pages.firstOrNull;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 8),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    RouteBadge(route: route.route),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        direction?.headsigns.join(' • ') ?? '',
                        style: theme.textTheme.titleSmall,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 4),
                Row(
                  children: [
                    Flexible(
                      child: Text(
                        direction?.stop.name ?? '',
                        style: muted,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    if (direction != null) ...[
                      const SizedBox(width: 6),
                      Icon(accessIcon(access), size: 14, color: muted?.color),
                      Text(
                        formatDuration(direction.walkSeconds.toDouble()),
                        style: muted,
                      ),
                    ],
                  ],
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          if (direction != null)
            // Room for where it goes, however many times follow.
            ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: _timesWidth),
              child: ValueListenableBuilder(
                valueListenable: now,
                builder: (context, now, _) =>
                    _Times(NearbyTimes.of(direction, now)),
              ),
            ),
        ],
      ),
    );
  }
}

/// Whole minutes until [c] leaves, or "Now" in its last minute.
String countdownText(Countdown c) =>
    c.minutesUntil < 1 ? 'Now' : '${c.minutesUntil} min';

/// The color of a departure the walk can't make any more.
Color _dimmed(ThemeData theme) =>
    theme.colorScheme.onSurface.withValues(alpha: 0.38);

class _Times extends StatelessWidget {
  const _Times(this.times);

  final NearbyTimes times;

  static const _figures = [FontFeature.tabularFigures()];

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final next = times.next;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        // The line keeps its height when nothing that runs is left.
        Visibility.maintain(
          visible: next != null,
          child: next == null ? const _Next.empty() : _Next(next),
        ),
        const SizedBox(height: 2),
        Text.rich(
          TextSpan(
            children: [
              for (final (i, c) in times.following.indexed) ...[
                if (i > 0) const TextSpan(text: ' · '),
                ..._following(theme, c),
              ],
            ],
          ),
          key: const ValueKey('following'),
          style: theme.textTheme.bodySmall?.copyWith(
            color: theme.colorScheme.onSurfaceVariant,
            fontFeatures: _figures,
          ),
          maxLines: 1,
          softWrap: false,
          overflow: TextOverflow.fade,
        ),
      ],
    );
  }

  List<InlineSpan> _following(ThemeData theme, Countdown c) =>
      switch (c.state) {
        CountdownState.canceled || CountdownState.skipped => [
          TextSpan(
            text: countdownText(c),
            style: const TextStyle(decoration: TextDecoration.lineThrough),
          ),
          TextSpan(
            text: c.state == CountdownState.canceled ? ' Canceled' : ' Skipped',
          ),
        ],
        CountdownState.unreachable => [
          TextSpan(
            text: countdownText(c),
            style: TextStyle(color: _dimmed(theme)),
          ),
        ],
        CountdownState.upcoming ||
        CountdownState.departed => [TextSpan(text: countdownText(c))],
      };
}

/// The next departure's countdown, large, with the live mark when it's real-time.
class _Next extends StatelessWidget {
  const _Next(Countdown this.countdown);
  const _Next.empty() : countdown = null;

  final Countdown? countdown;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final c = countdown;
    final dimmed = c?.state == CountdownState.unreachable;
    final color = dimmed ? _dimmed(theme) : theme.colorScheme.onSurface;
    final minutes = c?.minutesUntil ?? 0;
    final number = minutes < 1 ? 'Now' : '$minutes';
    return Row(
      key: const ValueKey('next'),
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.baseline,
      textBaseline: TextBaseline.alphabetic,
      children: [
        if (c?.status == DepartureStatus.live) ...[
          Icon(
            Icons.sensors,
            key: const ValueKey('live'),
            size: 14,
            color: dimmed ? color : liveColor(theme),
          ),
          const SizedBox(width: 3),
        ],
        AnimatedSwitcher(
          duration: const Duration(milliseconds: 250),
          layoutBuilder: (current, previous) => Stack(
            alignment: Alignment.centerRight,
            children: [...previous, ?current],
          ),
          child: Text(
            number,
            key: ValueKey(number),
            style: theme.textTheme.titleLarge?.copyWith(
              color: color,
              fontWeight: FontWeight.w600,
              height: 1.1,
              fontFeatures: _Times._figures,
            ),
          ),
        ),
        if (minutes >= 1)
          Text(
            ' min',
            style: theme.textTheme.labelMedium?.copyWith(color: color),
          ),
      ],
    );
  }
}
