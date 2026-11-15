/// The Nearby transit tab: the routes a rider where they are can catch, soonest first,
/// each on one of its pages with its next departures counting down.
library;

import 'dart:async';

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../directions/format.dart';
import '../directions/route_source.dart';
import '../directions/transit_details.dart' show RouteBadge;
import '../directions/transit_plan.dart';
import '../directions/transit_sheet.dart' show accessIcon, clock, liveColor;
import '../map/location.dart';
import '../routes.dart';
import '../settings/accessibility.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import '../ui/haptics.dart';
import 'countdown.dart';
import 'nearby_rows.dart';
import 'nearby_source.dart';
import 'nearby_transit.dart';
import 'vehicle_markers.dart';

/// Departures further off than this many minutes show the clock time they leave at,
/// instead of a countdown.
const countdownMinutes = 60;

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
    this.vehicleMarkers = paintVehicleMarkers,
    required super.child,
  });

  final NearbyTransitSource source;

  /// What time it is: the device's clock, or a test's.
  final DateTime Function() clock;

  /// Draws a route's vehicle markers for the map: the device's drawing, or a test's.
  final VehicleMarkerPainter vehicleMarkers;

  static NearbyTransitScope of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<NearbyTransitScope>()!;

  @override
  bool updateShouldNotify(NearbyTransitScope oldWidget) =>
      source != oldWidget.source ||
      clock != oldWidget.clock ||
      vehicleMarkers != oldWidget.vehicleMarkers;
}

/// A page's departures at one moment, as a row shows them: the next that runs, and
/// the rest after it.
class NearbyTimes {
  NearbyTimes._(this.next, this.following);

  /// [page]'s departures at [now]. Those that have left are left out. The first
  /// that runs is [next], even if the walk can't make it any more; canceled and skipped
  /// ones are never [next], and are among [following] in their place.
  factory NearbyTimes.of(HeadsignPage page, DateTime now) {
    final left = [
      for (final d in page.departures)
        Countdown(d, now: now, walkSeconds: page.walkSeconds),
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

/// The Nearby transit tab: one row per route near where you are, each on one of its
/// pages, ordered as [NearbyRows] orders them, which stays put while the list shows.
///
/// While it shows (the app is in front and no page covers it), it asks from your
/// location when it has one, again every [nearbyRefresh], and at once when wheelchair
/// access is turned on or off; countdowns tick each second in between. The first answer
/// is on the schedule while real-time data is fetched, and is updated in place once it
/// arrives ([NearbyTransitController]). While it doesn't show, it asks nothing, but
/// keeps its rows: when it shows again they're still there, and the next answer updates
/// them, re-sorting them only as any answer would.
///
/// It never asks for location permission; without it, it says so.
class NearbyTransitPage extends StatefulWidget {
  const NearbyTransitPage({super.key});

  @override
  State<NearbyTransitPage> createState() => _NearbyTransitPageState();
}

/// Why there are no rows to show, as plain text: what the user settled on for these
/// states.
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
  /// All of the page's content, whose end the sheet never goes past.
  static const _content = #nearbyTransitContent;

  /// Where the sheet opens and the lowest it rests: 40% of the screen.
  ///
  /// Content shorter than that ends below it, and then the end of the content is the
  /// lower anchor and this the upper one: the sheet can't say "whichever is lower".
  static const _lower = SheetAnchor.fraction(0.4);

  /// The highest the sheet goes: the end of the content, or the full screen if that
  /// comes first.
  static const _upper = SheetAnchor.section(_content);

  /// Between the two, the sheet rests wherever it's let go, with no anchor at the top
  /// to spring to, and a fling up carries on into scrolling the routes.
  static const _range = SheetRange(_lower, _upper, carriesIntoScroll: true);

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
      _controller = NearbyTransitController(
        scope.source,
        headsigns: StoreScope.of(context).headsigns,
        clock: _clock,
      )..addListener(_changed);
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
      _controller!.abandon();
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

  /// Opens [route]'s detail on the page its row shows. This page stays under it as it
  /// is, rows, pages, and scroll, for coming back to.
  void _open(NearbyRoute route) {
    final page = _controller!.rows.shownPage(route);
    if (page == null) return;
    unawaited(
      context.push<void>(
        routeDetailLocation(
          feed: route.feed,
          routeId: route.route.id,
          stopId: page.stop.id,
          headsigns: page.headsigns,
        ),
        extra: route,
      ),
    );
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
      anchors: const [_lower],
      freeRanges: const [_range],
      slivers: [
        SliverToBoxAdapter(
          child: SheetSection(
            id: _content,
            // The sheet follows as rows come and go, rather than jumping.
            child: AnimatedSize(
              duration: const Duration(milliseconds: 250),
              curve: Curves.easeOutCubic,
              alignment: Alignment.topCenter,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  const _Header(),
                  if (routes.isEmpty)
                    _Placeholder(_note)
                  else
                    Padding(
                      padding: const EdgeInsets.only(bottom: 8),
                      child: AppListSection(
                        children: [
                          for (final route in routes)
                            NearbyRow(
                              key: ValueKey(route.key),
                              route: route,
                              shown: _controller!.rows.shown(route.key),
                              onShown: (page) =>
                                  _controller!.show(route.key, page),
                              onTap: () => _open(route),
                              access: access,
                              now: _now,
                            ),
                        ],
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

/// A one-line message in place of the rows, with room around it.
class _Placeholder extends StatelessWidget {
  const _Placeholder(this.note);

  final _Note note;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: _placeholderPadding,
      child: Text(
        note.text,
        style: theme.textTheme.bodyMedium?.copyWith(
          color: theme.colorScheme.onSurfaceVariant,
        ),
      ),
    );
  }
}

/// The room around a message shown in place of the rows.
const _placeholderPadding = EdgeInsets.symmetric(horizontal: 24, vertical: 20);

/// The space above and below a nearby row's lines.
const nearbyRowPadding = 11.0;

/// How [page]'s headsigns read: "A / B", or nothing for a page without any.
String headsignsText(HeadsignPage page) => page.headsigns.join(' / ');

/// One route near you, on one of its pages: its badge and where the page goes, the stop
/// and the walk there, and its next departures counting down by [now].
///
/// With more than one page, the rider swipes between them from anywhere on the row,
/// the dots below that say which is shown among it. A swipe that settles on another page gives a light haptic and calls
/// [onShown]; the list decides what's [shown]. Every page has the same lines, and the
/// row is as tall as its first page, so it keeps its height from one page to the next.
class NearbyRow extends StatefulWidget {
  const NearbyRow({
    super.key,
    required this.route,
    this.shown = 0,
    this.onShown,
    this.onTap,
    required this.access,
    required this.now,
  });

  final NearbyRoute route;

  /// The page it shows, by index into the route's pages.
  final int shown;

  /// Called with the page a swipe settles on, when it isn't [shown].
  final ValueChanged<int>? onShown;

  /// Called when the row is tapped.
  final VoidCallback? onTap;

  /// How the walk to the stop goes.
  final TransitAccess access;
  final ValueListenable<DateTime> now;

  @override
  State<NearbyRow> createState() => _NearbyRowState();
}

class _NearbyRowState extends State<NearbyRow> {
  late var _pages = _controller(widget.shown);

  /// Pages starting at [page]. The list says which page shows, not page storage.
  static PageController _controller(int page) =>
      PageController(initialPage: page, keepPage: false);

  /// The page the pages rest on, or last rested on while they move.
  late var _at = widget.shown;

  /// Whether a swipe is under way or settling.
  var _moving = false;

  @override
  void didUpdateWidget(NearbyRow oldWidget) {
    super.didUpdateWidget(oldWidget);
    // The list shows another page than the one at rest, as when a refresh changed the
    // route's pages: go there without sliding. A swipe under way settles first, and has
    // the last word.
    if (!_moving && widget.shown != _at) {
      final old = _pages;
      _pages = _controller(widget.shown);
      _at = widget.shown;
      // The old page view lets go of it as the new one replaces it.
      WidgetsBinding.instance.addPostFrameCallback((_) => old.dispose());
    }
  }

  @override
  void dispose() {
    _pages.dispose();
    super.dispose();
  }

  bool _scrolled(ScrollNotification n) {
    if (n.depth != 0) return false;
    if (n is ScrollStartNotification) {
      _moving = true;
    } else if (n is ScrollEndNotification) {
      _moving = false;
      _settled();
    }
    return false;
  }

  void _settled() {
    if (!_pages.hasClients) return;
    final page = _pages.page!.round();
    _at = page;
    if (page == widget.shown) return;
    Haptics.impact();
    widget.onShown?.call(page);
  }

  @override
  Widget build(BuildContext context) {
    final onTap = widget.onTap;
    final row = _row();
    return onTap == null ? row : InkWell(onTap: onTap, child: row);
  }

  Widget _row() {
    final pages = widget.route.pages;
    if (pages.length < 2) return _page(pages.firstOrNull);
    // The pages take the whole row, the dots' strip with them, so a swipe that starts
    // anywhere on it, the dots and the gaps between lines too, moves them. The dots are
    // drawn over the pages, and let swipes and taps through.
    return Stack(
      children: [
        // The row's height: the first page's, which every page shares, and the dots'.
        Visibility(
          visible: false,
          maintainSize: true,
          maintainAnimation: true,
          maintainState: true,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              _page(pages.first),
              const SizedBox(height: _Dots.height),
            ],
          ),
        ),
        Positioned.fill(
          child: NotificationListener<ScrollNotification>(
            onNotification: _scrolled,
            child: PageView.builder(
              // A new controller starts a new page view: one that went on would
              // take the old one's place, not the controller's first page.
              key: ObjectKey(_pages),
              controller: _pages,
              itemCount: pages.length,
              findChildIndexCallback: (key) {
                final i = pages.indexWhere((p) => _key(p) == key);
                return i < 0 ? null : i;
              },
              itemBuilder: (context, i) => Align(
                key: _key(pages[i]),
                alignment: Alignment.topCenter,
                child: _page(pages[i]),
              ),
            ),
          ),
        ),
        Positioned(
          left: 0,
          right: 0,
          bottom: 0,
          child: IgnorePointer(
            child: _Dots(count: pages.length, controller: _pages, shown: _at),
          ),
        ),
      ],
    );
  }

  /// Which page [page] is, from one answer to the next.
  static ValueKey<String> _key(HeadsignPage page) =>
      ValueKey(page.headsigns.join('\n'));

  Widget _page(HeadsignPage? page, {Key? key}) => _NearbyPage(
    key: key,
    route: widget.route,
    page: page,
    access: widget.access,
    now: widget.now,
  );
}

/// Which of a row's pages is shown: a dot each, the shown one darker.
class _Dots extends StatelessWidget {
  const _Dots({
    required this.count,
    required this.controller,
    required this.shown,
  });

  final int count;
  final PageController controller;

  /// The page shown before the pages are laid out.
  final int shown;

  static const _size = 6.0;
  static const _below = 6.0;

  /// The strip's height: the dots and the space below them.
  static const height = _size + _below;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.onSurfaceVariant;
    return Padding(
      padding: const EdgeInsets.only(bottom: _below),
      child: AnimatedBuilder(
        animation: controller,
        builder: (context, _) {
          final at = controller.hasClients && controller.position.haveDimensions
              ? controller.page!.round()
              : shown;
          return Row(
            key: const ValueKey('dots'),
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              for (var i = 0; i < count; i++)
                Container(
                  width: _size,
                  height: _size,
                  margin: const EdgeInsets.symmetric(horizontal: 3),
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: i == at ? color : color.withValues(alpha: 0.3),
                  ),
                ),
            ],
          );
        },
      ),
    );
  }
}

/// A route detail's header: [page] of [route] as its row shows it, badge, headsigns,
/// stop, and walk, without its times or the route's other pages, its lines where the
/// row's are.
class NearbyHeader extends StatelessWidget {
  const NearbyHeader({
    super.key,
    required this.route,
    required this.page,
    required this.access,
  });

  final NearbyRoute route;
  final HeadsignPage? page;
  final TransitAccess access;

  @override
  Widget build(BuildContext context) =>
      _NearbyPage(route: route, page: page, access: access);
}

/// One page of a row: the route's badge and where [page] goes, its stop and the walk
/// there, and its departures counting down by [now].
class _NearbyPage extends StatelessWidget {
  const _NearbyPage({
    super.key,
    required this.route,
    required this.page,
    required this.access,
    this.now,
  });

  final NearbyRoute route;
  final HeadsignPage? page;
  final TransitAccess access;

  /// What its departures count down by; without it, they aren't shown.
  final ValueListenable<DateTime>? now;

  /// The most the departures take of the row's width.
  static const _timesWidth = 160.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final page = this.page;
    final now = this.now;
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        12,
        nearbyRowPadding,
        12,
        nearbyRowPadding,
      ),
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
                        page == null ? '' : headsignsText(page),
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
                        page?.stop.name ?? '',
                        style: muted,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    if (page != null) ...[
                      const SizedBox(width: 6),
                      Icon(accessIcon(access), size: 14, color: muted?.color),
                      Text(
                        formatDuration(page.walkSeconds.toDouble()),
                        style: muted,
                      ),
                    ],
                  ],
                ),
              ],
            ),
          ),
          if (page != null && now != null) ...[
            const SizedBox(width: 12),
            // Room for where it goes, however many times follow.
            ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: _timesWidth),
              child: ValueListenableBuilder(
                valueListenable: now,
                builder: (context, now, _) => _Times(NearbyTimes.of(page, now)),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// Whether [c] shows the clock time it leaves at, rather than a countdown.
bool _showsClock(Countdown c) => c.minutesUntil > countdownMinutes;

/// Whole minutes until [c] leaves, or "Now" in its last minute; or the time it leaves
/// at, in the locale's format, when that's more than [countdownMinutes] off.
String countdownText(BuildContext context, Countdown c) {
  if (_showsClock(c)) return clock(context, c.departure);
  return c.minutesUntil < 1 ? 'Now' : '${c.minutesUntil} min';
}

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
          child: CountdownNumber(next, key: const ValueKey('next')),
        ),
        const SizedBox(height: 2),
        Text.rich(
          TextSpan(
            children: [
              for (final (i, c) in times.following.indexed) ...[
                if (i > 0) const TextSpan(text: ' · '),
                ..._following(context, theme, c),
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

  List<InlineSpan> _following(
    BuildContext context,
    ThemeData theme,
    Countdown c,
  ) => switch (c.state) {
    CountdownState.canceled || CountdownState.skipped => [
      TextSpan(
        text: countdownText(context, c),
        style: const TextStyle(decoration: TextDecoration.lineThrough),
      ),
      TextSpan(
        text: c.state == CountdownState.canceled ? ' Canceled' : ' Skipped',
      ),
    ],
    CountdownState.unreachable => [
      TextSpan(
        text: countdownText(context, c),
        style: TextStyle(color: _dimmed(theme)),
      ),
    ],
    CountdownState.upcoming ||
    CountdownState.departed => [TextSpan(text: countdownText(context, c))],
  };
}

/// A departure's countdown, large, with the live mark when it's real-time; or the time
/// it leaves at, when that's more than [countdownMinutes] off. Dimmed when the walk
/// can't make it any more, and struck through when it doesn't run. A changed number
/// fades into the new one.
///
/// Without a [countdown], it's as tall as one, for a line that keeps its height.
class CountdownNumber extends StatelessWidget {
  const CountdownNumber(
    this.countdown, {
    super.key,
    this.numberStyle,
    this.unitStyle,
    this.liveSize = 14,
    this.alignment = Alignment.centerRight,
  });

  final Countdown? countdown;

  /// The number's style, over the theme's `titleLarge` at weight 600.
  final TextStyle? numberStyle;

  /// The style of " min" after it, over the theme's `labelMedium`.
  final TextStyle? unitStyle;

  /// The live mark's size.
  final double liveSize;

  /// Where a number fading out sits under the one fading in.
  final AlignmentGeometry alignment;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final c = countdown;
    final state = c?.state;
    final dimmed = state == CountdownState.unreachable;
    final struck =
        state == CountdownState.canceled || state == CountdownState.skipped;
    final color = dimmed
        ? _dimmed(theme)
        : struck
        ? theme.colorScheme.onSurfaceVariant
        : theme.colorScheme.onSurface;
    final decoration = struck ? TextDecoration.lineThrough : null;
    final minutes = c?.minutesUntil ?? 0;
    final counts = c == null || !_showsClock(c);
    final number = !counts
        ? clock(context, c.departure)
        : minutes < 1
        ? 'Now'
        : '$minutes';
    return Row(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.baseline,
      textBaseline: TextBaseline.alphabetic,
      children: [
        if (c?.status == DepartureStatus.live) ...[
          Icon(
            Icons.sensors,
            key: const ValueKey('live'),
            size: liveSize,
            color: dimmed ? color : liveColor(theme),
          ),
          const SizedBox(width: 3),
        ],
        AnimatedSwitcher(
          duration: const Duration(milliseconds: 250),
          layoutBuilder: (current, previous) =>
              Stack(alignment: alignment, children: [...previous, ?current]),
          child: Text(
            number,
            key: ValueKey(number),
            style: theme.textTheme.titleLarge
                ?.copyWith(
                  fontWeight: FontWeight.w600,
                  height: 1.1,
                  fontFeatures: _Times._figures,
                )
                .merge(numberStyle)
                .copyWith(color: color, decoration: decoration),
          ),
        ),
        if (counts && minutes >= 1)
          Text(
            ' min',
            style: theme.textTheme.labelMedium
                ?.merge(unitStyle)
                .copyWith(color: color, decoration: decoration),
          ),
      ],
    );
  }
}
