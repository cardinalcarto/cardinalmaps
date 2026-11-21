/// One transit journey in detail, top to bottom: each ride on a bar in its route's
/// color with its stops and times, the walks and waits between rides, and for each ride
/// the other departures of its route, so a rider can see what happens if they miss it.
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/rendering.dart';
import 'package:material_ui/material_ui.dart';

import '../units.dart';
import '../transit/countdown.dart';
import '../transit/nearby_page.dart' show CountdownNumber, countdownMinutes;
import '../ui/adaptive.dart';
import 'animated_rows.dart';
import 'connections.dart';
import 'format.dart';
import 'journey_list.dart' show streetTime;
import 'option_row.dart' show FittedLine, isRoomyText, textLinesHeight;
import 'transit_plan.dart';
import 'transit_sheet.dart';
import 'transit_timeline.dart';

const _figures = [FontFeature.tabularFigures()];

/// The top of a journey's details, as the Steps page's is: back to the overview; when
/// to go, and how long the journey takes, with the departures chosen in [chain]; when it
/// leaves and arrives, how many transfers it has and how long it's on the streets; its
/// timeline; and a divider, but on iOS.
///
/// With every departure of the first ride gone ([ConnectionChain.gone]), it says so in
/// place of when to go, with no duration, and when the last of them left in place of
/// its times.
///
/// It's as tall whatever the choices, so nothing below it moves when one is made: when
/// to go and how long it takes have a line each, and shrink to fit it rather than wrap;
/// the line of times keeps the room the widest times, or no connection, or when the
/// last left, would take, and wraps as it needs. At the roomier text sizes (see
/// [isRoomyText]) when to go and how long it takes are stacked.
class JourneyHeader extends StatelessWidget {
  const JourneyHeader({
    super.key,
    required this.chain,
    required this.now,
    required this.onBack,
  });

  /// The journey, with the departures chosen of its rides.
  final ConnectionChain chain;
  final DateTime now;
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final text = theme.textTheme;
    final roomy = isRoomyText(MediaQuery.textScalerOf(context));
    final journey = chain.journey;
    final offset = journeyUtcOffset(journey);
    final arrival = chain.arrival;
    final duration = chain.duration;
    final gone = chain.gone;
    final first = chain.rides.firstOrNull;
    // When the last of the first ride's departures left.
    final lastLeft = gone
        ? first!.lastRunning?.departure ?? first.ride.departure
        : null;
    final title = text.titleLarge?.copyWith(fontWeight: FontWeight.w700);
    final muted = text.bodyMedium?.copyWith(
      color: scheme.onSurfaceVariant,
      fontFeatures: _figures,
    );
    final back = IconButton(
      tooltip: 'Back',
      icon: const BackButtonIcon(),
      onPressed: onBack,
    );
    final took = FadeSwap(
      value: duration == null ? null : duration ~/ 60,
      alignment: roomy
          ? AlignmentDirectional.centerStart
          : AlignmentDirectional.centerEnd,
      child: Text(
        duration == null ? '' : formatDuration(duration.toDouble()),
        style: text.titleLarge?.copyWith(
          color: scheme.primary,
          fontWeight: FontWeight.w500,
        ),
        maxLines: 1,
        softWrap: false,
      ),
    );
    // Cross-fades when the time to leave changes with a choice, not as it counts
    // down.
    final goIn = FadeSwap(
      value: gone ? null : chain.departure,
      child: gone
          ? Text(
              'No departures left',
              style: title?.copyWith(color: scheme.error),
              maxLines: 1,
              softWrap: false,
            )
          : GoIn(
              journey: journey,
              now: now,
              leave: chain.departure,
              live: first?.realtime,
              style: title,
            ),
    );
    final facts = [
      switch (journey.transfers) {
        0 => 'No transfers',
        1 => '1 transfer',
        final n => '$n transfers',
      },
      ?streetTime(journey),
    ].join(' · ');
    TextSpan line(String times, {required bool connects}) => TextSpan(
      children: [
        TextSpan(
          text: times,
          style: connects
              ? null
              : TextStyle(color: scheme.error, fontWeight: FontWeight.w500),
        ),
        TextSpan(text: ' · $facts'),
      ],
    );
    final leaves = clock(context, chain.departure, utcOffset: offset);
    final localizations = MaterialLocalizations.of(context);
    final h24 = MediaQuery.alwaysUse24HourFormatOf(context);
    String widest(int hour) => localizations.formatTimeOfDay(
      TimeOfDay(hour: hour, minute: 0),
      alwaysUse24HourFormat: h24,
    );
    // The room the widest times take, or no connection, or when the last left, so a
    // choice, or the clock, never changes how many lines this takes.
    final reserved = [
      for (final from in const [10, 22]) ...[
        for (final to in const [10, 22])
          (text: '${widest(from)} – ${widest(to)}', connects: true),
        (text: '${widest(from)} – no connection', connects: false),
        (text: 'Last left ${widest(from)}', connects: false),
      ],
    ];
    final times = Stack(
      children: [
        for (final r in reserved)
          ExcludeSemantics(
            child: Visibility(
              visible: false,
              maintainSize: true,
              maintainAnimation: true,
              maintainState: true,
              child: Text.rich(
                line(r.text, connects: r.connects),
                style: muted,
              ),
            ),
          ),
        FadeSwap(
          value: (chain.departure, arrival, lastLeft),
          alignment: AlignmentDirectional.topStart,
          child: Text.rich(
            line(switch ((lastLeft, arrival)) {
              (final last?, _) =>
                'Last left ${clock(context, last, utcOffset: offset)}',
              (null, null) => '$leaves – no connection',
              (null, final arrival?) =>
                '$leaves – ${clock(context, arrival, utcOffset: offset)}',
            }, connects: arrival != null),
            style: muted,
          ),
        ),
      ],
    );
    final timeline = JourneyTimeline(
      journey: journey,
      chain: chain,
      // With no connection, the plan's.
      axis: arrival == null
          ? TimelineAxis(journey.departure, journey.arrival)
          : TimelineAxis(chain.departure, arrival),
      height: 22,
    );
    final Widget top;
    if (roomy) {
      top = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          FittedLine(style: title, child: goIn),
          FittedLine(style: text.titleLarge, child: took),
          const SizedBox(height: 2),
          times,
        ],
      );
    } else {
      top = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 2,
        children: [
          // One line, whatever the choices, so nothing below it moves: when to go
          // and how long it takes each shrink to fit their share rather than wrap.
          // Both are lines of one size, so they sit on one baseline at full size;
          // they aren't aligned by it, as one shrunk to fit ("No departures left")
          // has its baseline higher, and would push the row taller.
          Row(
            children: [
              Expanded(
                flex: 3,
                child: FittedLine(style: title, child: goIn),
              ),
              const SizedBox(width: 8),
              Flexible(
                flex: 2,
                child: FittedLine(
                  style: text.titleLarge,
                  alignment: AlignmentDirectional.centerEnd,
                  child: took,
                ),
              ),
            ],
          ),
          times,
        ],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(4, 0, 16, 8),
          child: Row(
            crossAxisAlignment: roomy
                ? CrossAxisAlignment.start
                : CrossAxisAlignment.center,
            children: [
              back,
              const SizedBox(width: 4),
              Expanded(child: top),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
          child: timeline,
        ),
        if (!isCupertinoOf(context)) const Divider(height: 1),
      ],
    );
  }
}

/// What's drawn on the rail left of a row, above and below its marker: nothing, the
/// dots of a walk, the thin line of a wait, or a ride's bar.
enum _Line { none, dotted, thin, bar }

/// The mark on the rail level with a row's first line: an end of the trip, a stop the
/// rider gets on or off at, one passed, or where a stay-seated ride goes on as another
/// route.
enum _Marker { none, end, stop, pass, seat }

/// The rail beside one row of the itinerary.
@immutable
class _Rail {
  const _Rail({
    this.above = _Line.none,
    this.below = _Line.none,
    this.marker = _Marker.none,
    this.aboveColor,
    this.belowColor,
    this.icon,
    this.at,
  });

  /// A row with no marker, one kind of line through it.
  const _Rail.through(_Line line, {Color? color})
    : above = line,
      below = line,
      marker = _Marker.none,
      aboveColor = color,
      belowColor = color,
      icon = null,
      at = null;

  final _Line above;
  final _Line below;
  final _Marker marker;
  final Color? aboveColor;
  final Color? belowColor;
  final IconData? icon;

  /// How far down the row the marker is, and where the line above gives way to the
  /// one below: level with the first line of its text. Null for [_markerY], or with
  /// no marker or the stay-seated one, the row's middle.
  final double? at;

  @override
  bool operator ==(Object other) =>
      other is _Rail &&
      other.above == above &&
      other.below == below &&
      other.marker == marker &&
      other.aboveColor == aboveColor &&
      other.belowColor == belowColor &&
      other.icon == icon &&
      other.at == at;

  @override
  int get hashCode =>
      Object.hash(above, below, marker, aboveColor, belowColor, icon, at);
}

/// Where a row's text starts: the rail's column, 24 wide after a 16 margin, and a
/// gap of 12.
const _contentStart = 52.0;

/// The middle of the rail's column.
const _railX = 28.0;

/// How far down a row a marker sits when nothing says: level with a line of text in
/// the middle of its first line.
const _markerY = 15.0;

/// Where a ride's bar starts in its title's row.
const _barStart = 10.0;

/// How wide a ride's bar is.
const _barWidth = 6.0;

/// How far a solid line runs past a row's edge, so rows join without a seam.
const _overlap = 1.0;

class _RailPainter extends CustomPainter {
  _RailPainter(this.rail, this.scheme);

  final _Rail rail;
  final ColorScheme scheme;

  @override
  void paint(Canvas canvas, Size size) {
    final y =
        rail.at ??
        (rail.marker == _Marker.none || rail.marker == _Marker.seat
            ? size.height / 2
            : _markerY);
    final above = rail.above;
    final below = rail.below;
    // Solid lines run a pixel past the row's edges into the next row's, whose line
    // meets this one: rows can end between pixels, where two half-covered edges
    // would show as a seam across the bar. A bar is rounded only at its ends, and one
    // of one color through the row is drawn whole, so a stop passed shows no seam.
    if (above == _Line.bar &&
        below == _Line.bar &&
        rail.aboveColor == rail.belowColor) {
      _bar(canvas, rail.aboveColor, 0, size.height, size);
    } else {
      // A line that meets a bar starting here runs on under its rounded top.
      _line(
        canvas,
        above,
        rail.aboveColor,
        0,
        below == _Line.bar && above != _Line.bar ? y + 2 : y,
        size,
        end: below != _Line.bar,
      );
      _line(
        canvas,
        below,
        rail.belowColor,
        y,
        size.height,
        size,
        start: above != _Line.bar,
      );
    }
    final center = Offset(_railX, y);
    switch (rail.marker) {
      case _Marker.none:
        break;
      case _Marker.end:
        canvas
          ..drawCircle(center, 7, Paint()..color = scheme.surface)
          ..drawCircle(
            center,
            5.5,
            Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = 3
              ..color = scheme.onSurface,
          );
      case _Marker.stop:
        final color = rail.aboveColor ?? rail.belowColor ?? scheme.primary;
        canvas
          ..drawCircle(center, 7, Paint()..color = Colors.white)
          ..drawCircle(
            center,
            5.5,
            Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = 3
              ..color = color,
          );
      case _Marker.pass:
        canvas.drawCircle(center, 2, Paint()..color = Colors.white);
      case _Marker.seat:
        final color = rail.belowColor ?? scheme.primary;
        canvas
          ..drawCircle(center, 9, Paint()..color = scheme.surface)
          ..drawCircle(
            center,
            8,
            Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = 2
              ..color = color,
          );
        final icon = rail.icon;
        if (icon != null) {
          final glyph = TextPainter(
            text: TextSpan(
              text: String.fromCharCode(icon.codePoint),
              style: TextStyle(
                fontFamily: icon.fontFamily,
                package: icon.fontPackage,
                fontSize: 11,
                height: 1,
                color: color,
              ),
            ),
            textDirection: TextDirection.ltr,
          )..layout();
          glyph.paint(
            canvas,
            center - Offset(glyph.width / 2, glyph.height / 2),
          );
          glyph.dispose();
        }
    }
  }

  /// A line from [from] to [to] down the row. A bar that [start]s or [end]s here, rather
  /// than going on into the next row, has a rounded end there; at the row's edge, it
  /// runs on past it.
  void _line(
    Canvas canvas,
    _Line line,
    Color? color,
    double from,
    double to,
    Size size, {
    bool start = false,
    bool end = false,
  }) {
    if (to <= from) return;
    switch (line) {
      case _Line.none:
        break;
      case _Line.dotted:
        final paint = Paint()..color = scheme.outline;
        // Dots at a fixed pitch from the row's top, so rows join without a seam.
        const pitch = 8.0;
        for (
          var y = ((from - 4) / pitch).ceil() * pitch + 4;
          y < to;
          y += pitch
        ) {
          canvas.drawCircle(Offset(_railX, y), 2, paint);
        }
      case _Line.thin:
        canvas.drawLine(
          Offset(_railX, from <= 0 ? -_overlap : from),
          Offset(_railX, to >= size.height ? size.height + _overlap : to),
          Paint()
            ..color = scheme.outlineVariant
            ..strokeWidth = 2,
        );
      case _Line.bar:
        // An end under a marker reaches a pixel past its middle, so it's hidden.
        _bar(
          canvas,
          color,
          from,
          end && to < size.height ? to + 1 : to,
          size,
          roundTop: start && from > 0,
          roundBottom: end && to < size.height,
        );
    }
  }

  void _bar(
    Canvas canvas,
    Color? color,
    double from,
    double to,
    Size size, {
    bool roundTop = false,
    bool roundBottom = false,
  }) {
    const half = _barWidth / 2;
    const radius = Radius.circular(half);
    canvas.drawRRect(
      RRect.fromLTRBAndCorners(
        _railX - half,
        from <= 0 ? -_overlap : from,
        _railX + half,
        to >= size.height ? size.height + _overlap : to,
        topLeft: roundTop ? radius : Radius.zero,
        topRight: roundTop ? radius : Radius.zero,
        bottomLeft: roundBottom ? radius : Radius.zero,
        bottomRight: roundBottom ? radius : Radius.zero,
      ),
      Paint()..color = color ?? scheme.primary,
    );
  }

  @override
  bool shouldRepaint(_RailPainter old) =>
      old.rail != rail || old.scheme != scheme;
}

/// A row of the itinerary: its rail, and what's said beside it, to [end] from the
/// row's end; tapping anywhere on it does [onTap], if there is one.
class _RailRow extends StatelessWidget {
  const _RailRow({
    super.key,
    required this.rail,
    required this.child,
    this.onTap,
    this.end = 16,
  });

  final _Rail rail;
  final Widget child;
  final VoidCallback? onTap;
  final double end;

  @override
  Widget build(BuildContext context) {
    final row = CustomPaint(
      painter: _RailPainter(rail, Theme.of(context).colorScheme),
      child: Padding(
        padding: EdgeInsetsDirectional.only(start: _contentStart, end: end),
        child: child,
      ),
    );
    return onTap == null ? row : InkWell(onTap: onTap, child: row);
  }
}

/// How far down a stop's row its marker goes, level with the middle of its first line:
/// its name's, or at the roomier text sizes, its time's on the line above.
double _stopAt(BuildContext context, {required bool strong}) {
  final text = Theme.of(context).textTheme;
  final roomy = isRoomyText(MediaQuery.textScalerOf(context));
  final style = strong
      ? (roomy ? text.bodyLarge : text.titleMedium)
      : text.bodySmall;
  final line = textLinesHeight(context, style ?? const TextStyle());
  return _StopText.paddingFor(line, strong: strong) + line / 2;
}

/// How far down an end's row its marker goes, as [_stopAt].
double _endAt(BuildContext context) {
  final text = Theme.of(context).textTheme;
  final roomy = isRoomyText(MediaQuery.textScalerOf(context));
  final style = roomy ? text.bodyMedium : text.titleSmall;
  return _EndText.padding +
      textLinesHeight(context, style ?? const TextStyle()) / 2;
}

/// A route's name on its color, with its vehicle's icon: as tall as the name at the
/// system's text size, and no wider than it's given room for, where a long name ends
/// in an ellipsis.
class RouteBadge extends StatelessWidget {
  const RouteBadge({super.key, required this.route});

  final TransitRoute? route;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = parseRouteColor(route?.color) ?? theme.colorScheme.primary;
    final text = routeTextColor(route, color);
    final name = route?.name ?? '';
    final style = theme.textTheme.labelLarge!.copyWith(
      color: text,
      fontWeight: FontWeight.w600,
      height: 1,
    );
    final height = math.max(
      22.0,
      MediaQuery.textScalerOf(context).scale(style.fontSize!) + 8,
    );
    return Container(
      constraints: BoxConstraints(minHeight: height),
      padding: const EdgeInsets.symmetric(horizontal: 6),
      decoration: BoxDecoration(
        color: color,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            vehicleIcon(route?.kind ?? VehicleKind.other),
            size: 15,
            color: text,
          ),
          if (name.isNotEmpty) ...[
            const SizedBox(width: 3),
            Flexible(
              child: Text(
                name,
                style: style,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// A ride's stops from where it's boarded to where it ends, each with its time, on a bar
/// in its route's color: the itinerary's rows for a ride, with every stop between
/// showing. A time from real-time data is in the live color.
///
/// A stay-seated ride shows each of its trips' stops on a bar in that trip's route's
/// color, and where the vehicle goes on as the next trip, the itinerary's stay-on-board
/// row, naming the route it continues as. A join at one stop shows that stop once, before
/// the stay-on-board row.
class RideStops extends StatelessWidget {
  const RideStops({super.key, required this.ride});

  final TransitLeg ride;

  @override
  Widget build(BuildContext context) {
    final primary = Theme.of(context).colorScheme.primary;
    Color colorOf(TransitRoute? route) =>
        parseRouteColor(route?.color) ?? primary;
    final stops = ride.stops;
    final last = stops.length - 1;
    final rows = <Widget>[];
    void stop(int i, Color color) {
      final visit = stops[i];
      rows.add(
        _RailRow(
          key: ValueKey((i, visit.stop.id)),
          rail: _Rail(
            above: i == 0 ? _Line.none : _Line.bar,
            below: i == last ? _Line.none : _Line.bar,
            marker: i == 0 || i == last ? _Marker.stop : _Marker.pass,
            aboveColor: color,
            belowColor: color,
            at: _stopAt(context, strong: i == 0),
          ),
          child: _StopText(
            visit: visit,
            time: i == last ? visit.arrival : visit.departure,
            live: visit.predicted,
            scheduled: i == last
                ? visit.scheduledArrival
                : visit.scheduledDeparture,
            keepsScheduled: visit.predicted,
            skipped: visit.skipped,
            strong: i == 0,
          ),
        ),
      );
    }

    if (ride.segments.length < 2) {
      final color = colorOf(ride.route);
      for (var i = 0; i <= last; i++) {
        stop(i, color);
      }
    } else {
      final segments = ride.segments;
      // The next stop to show: a join at one stop is shown with the trip arriving there.
      var next = 0;
      for (final (s, segment) in segments.indexed) {
        final color = colorOf(segmentRoute(ride, segment));
        final to = segment.lastStop.clamp(0, last);
        for (
          var i = math.max(next, segment.firstStop.clamp(0, last));
          i <= to;
          i++
        ) {
          stop(i, color);
        }
        next = math.max(next, to + 1);
        if (s + 1 < segments.length) {
          final following = segments[s + 1];
          rows.add(
            _RailRow(
              key: ValueKey(('seat', s)),
              rail: _Rail(
                above: _Line.bar,
                below: _Line.bar,
                marker: _Marker.seat,
                aboveColor: color,
                belowColor: colorOf(segmentRoute(ride, following)),
                icon: Icons.airline_seat_recline_normal,
              ),
              child: _ContinueText(
                ride: ride,
                segment: following,
                at: stops[following.firstStop.clamp(0, last)],
              ),
            ),
          );
        }
      }
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: rows,
    );
  }
}

/// A journey's legs from top to bottom, as slivers' worth of rows, with the departures
/// chosen in [chain]: each ride's departures to choose from, and its route, times and
/// stops as the one chosen has them.
///
/// The plan lists the stops of each ride's suggested trip alone, so for another
/// departure only its boarding and alighting stops show until its leg, looked up apart
/// from the plan, is [fetched]; then its stops and trips grow in below its departures,
/// as the suggested trip's show.
class JourneyItinerary extends StatefulWidget {
  const JourneyItinerary({
    super.key,
    required this.chain,
    required this.onSelect,
    required this.start,
    required this.destination,
    required this.units,
    required this.access,
    required this.now,
    this.fetched = const [],
  });

  /// The journey, with the departures chosen of its rides.
  final ConnectionChain chain;

  /// For each of [chain]'s rides, by its index there, the leg of the departure it takes
  /// when that's another than the plan's own and has been looked up; null, or past the
  /// end, where it hasn't. A leg of another departure than the one taken is ignored.
  final List<TransitLeg?> fetched;

  /// The rider chose departure `departure` of ride `ride` (by index in
  /// [Journey.rides]).
  final void Function(int ride, int departure) onSelect;

  /// What the trip's ends are called.
  final String start;
  final String destination;

  /// Metric or imperial, for the distances on foot.
  final UnitSystem units;
  final TransitAccess access;
  final DateTime now;

  @override
  State<JourneyItinerary> createState() => _JourneyItineraryState();
}

class _JourneyItineraryState extends State<JourneyItinerary> {
  /// The rides' trips whose stops between are showing, as (leg, trip): each ride's,
  /// whichever of its departures it takes, so one open stays open as the choice
  /// changes, while the leg taken has that trip.
  final _expanded = <(int, int)>{};

  /// Whether each ride, by its index in the journey's legs, has a row for its wait
  /// before it, as first decided: a newer list of its departures, or a newer plan of
  /// the journey, could decide otherwise, and the row would come or go above them.
  final _waits = <int, bool>{};

  /// The room each ride's title keeps, by its index in the journey's legs, for the
  /// opening: see [_TitleRoom].
  final _titles = <int, _TitleRoom>{};

  /// The leg of the departure ride [ride] (by its index in [ConnectionChain.rides])
  /// takes, when its stops are known.
  TransitLeg? _taken(int ride) =>
      widget.chain.rides[ride].legTaken(widget.fetched.elementAtOrNull(ride));

  /// Whether [a] and [b] are laid out alike: the same legs, each ride between the
  /// same two stops, as a newer plan of a journey has them, or another journey on
  /// other trips of the same rides.
  static bool _alike(Journey a, Journey b) {
    if (a.legs.length != b.legs.length) return false;
    for (final (i, leg) in a.legs.indexed) {
      final other = b.legs[i];
      if (leg.runtimeType != other.runtimeType) return false;
      if (leg is TransitLeg &&
          other is TransitLeg &&
          (leg.boarding.stop.id != other.boarding.stop.id ||
              leg.alighting.stop.id != other.alighting.stop.id)) {
        return false;
      }
    }
    return true;
  }

  @override
  void didUpdateWidget(JourneyItinerary oldWidget) {
    super.didUpdateWidget(oldWidget);
    final chain = widget.chain;
    // A newer plan of the same journey, or a journey on other trips between the same
    // stops, keeps what's open of it, and its rows.
    if (!_alike(oldWidget.chain.journey, chain.journey)) {
      _expanded.clear();
      _waits.clear();
      _titles.clear();
    }
    // A trip the leg taken doesn't have closes. While the leg isn't known, the ride's
    // stay as they are, to show as they were once it is.
    _expanded.removeWhere((key) {
      final ride = chain.rides.indexWhere(
        (r) => identical(r.ride, chain.journey.legs[key.$1]),
      );
      if (ride < 0) return true;
      final leg = _taken(ride);
      return leg != null && key.$2 >= _trips(leg).length;
    });
  }

  /// A leg's trips, each with the range of its stops: one, all of them, unless it's
  /// stay-seated.
  static List<RideSegment?> _trips(TransitLeg? leg) =>
      leg != null && leg.segments.length >= 2 ? leg.segments : const [null];

  @override
  Widget build(BuildContext context) {
    return SliverPadding(
      padding: const EdgeInsets.only(top: 10, bottom: 16),
      sliver: SteadySliver(child: AnimatedRows(rows: _rows(context))),
    );
  }

  List<Widget> _rows(BuildContext context) {
    final chain = widget.chain;
    final journey = chain.journey;
    final legs = journey.legs;
    final offset = journeyUtcOffset(journey);
    final arrival = chain.arrival;
    final rows = <Widget>[];

    _Line connector(JourneyLeg? leg) => switch (leg) {
      null => _Line.none,
      StreetLeg() when _isChange(leg) => _Line.thin,
      StreetLeg() => _Line.dotted,
      TransitLeg() => _Line.thin,
    };

    rows.add(
      _RailRow(
        key: const ValueKey('start'),
        rail: _Rail(
          marker: _Marker.end,
          below: connector(legs.firstOrNull),
          at: _endAt(context),
        ),
        child: _EndText(
          name: widget.start,
          // With every departure of the first ride gone, there's no time to leave.
          time: chain.gone
              ? ''
              : 'Leave ${clock(context, chain.departure, utcOffset: offset)}',
          prefix: 'Leave ',
        ),
      ),
    );

    var ride = 0;
    for (final (i, leg) in legs.indexed) {
      final next = i + 1 < legs.length ? legs[i + 1] : null;
      final previous = i > 0 ? legs[i - 1] : null;
      switch (leg) {
        case StreetLeg() when _isChange(leg):
          // Shown with the next ride, as a change of vehicle.
          break;
        case StreetLeg():
          rows.add(
            _StreetRow(
              key: ValueKey(('street', i)),
              leg: leg,
              units: widget.units,
            ),
          );
        case TransitLeg():
          final choice = chain.rides[ride];
          final index = ride++;
          final change =
              previous is TransitLeg ||
              (previous is StreetLeg && _isChange(previous));
          final before = previous == null
              ? _Line.thin
              : connector(previous) == _Line.dotted
              ? _Line.dotted
              : _Line.thin;
          // Whether there's a row for the wait hangs on nothing chosen, nor the clock,
          // and it's held for the opening, so nothing ever adds or drops a row above
          // the ride's departures.
          final waits = _waits.putIfAbsent(i, () => change || _mayWait(choice));
          if (waits) {
            rows.add(
              _RailRow(
                key: ValueKey(('wait', i)),
                rail: const _Rail.through(_Line.thin),
                child: _WaitText(
                  change: change ? leg.boarding.stop.name : null,
                  seconds: choice.made ? choice.waitSeconds : null,
                ),
              ),
            );
          }
          rows.addAll(
            _rideRows(
              context,
              i,
              choice,
              _taken(index),
              onSelect: (departure) => widget.onSelect(index, departure),
              above: waits ? _Line.thin : before,
              below: connector(next),
            ),
          );
      }
    }

    rows.add(
      _RailRow(
        key: const ValueKey('end'),
        rail: _Rail(
          marker: _Marker.end,
          above: connector(legs.lastOrNull),
          at: _endAt(context),
        ),
        child: _EndText(
          name: widget.destination,
          time: switch (arrival) {
            _ when chain.gone => '',
            null => 'No connection',
            final arrival =>
              'Arrive ${clock(context, arrival, utcOffset: offset)}',
          },
          prefix: 'Arrive ',
          connects: arrival != null,
        ),
      ),
    );
    return rows;
  }

  /// A ride's rows: its route, its stops with their times, its departures, and any
  /// trips it goes on as, as the departure chosen has them. Its stops between and its
  /// trips are [taken]'s, the leg of the departure chosen, when that's known.
  ///
  /// Everything above its departures is the same whether [taken] is known or not, so
  /// its arrival moves nothing above them. Its boarding and alighting times are the
  /// choice's, as its tile and the rides after have them.
  List<Widget> _rideRows(
    BuildContext context,
    int index,
    RideChoice choice,
    TransitLeg? taken, {
    required ValueChanged<int> onSelect,
    required _Line above,
    required _Line below,
  }) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final ride = choice.ride;
    final route = choice.route;
    final rows = <Widget>[];
    final segments = _trips(taken);
    Color colorOf(RideSegment? segment) =>
        parseRouteColor(
          (segment == null ? route : segmentRoute(taken!, segment))?.color,
        ) ??
        scheme.primary;
    // Another departure's rows above its departures are in its route's color before
    // its leg is known, and stay so after.
    final first = choice.suggested ? colorOf(segments.first) : colorOf(null);
    final last = colorOf(segments.last);
    final leaves = choice.departureTime;
    final arrives = choice.arrival;
    final live = choice.suggested
        ? (ride.boarding.predicted, ride.alighting.predicted)
        : (choice.realtime, choice.realtime);
    final departure = choice.departure;
    // Whether any of its departures may show a scheduled time beside a live one, so
    // the boarding stop's row keeps the room for one whichever is chosen.
    final mayBeOff = ride.realtime || choice.departures.any((d) => d.realtime);

    rows.add(
      _RailRow(
        key: ValueKey(('title', index)),
        rail: _Rail(
          above: above,
          below: _Line.bar,
          belowColor: first,
          at: _barStart,
        ),
        child: _RideTitle(
          ride: ride,
          room: _titles.putIfAbsent(index, _TitleRoom.new)..add(choice),
          route: route,
          headsign: choice.headsign,
        ),
      ),
    );
    rows.add(
      _RailRow(
        key: ValueKey(('boarding', index)),
        rail: _Rail(
          above: _Line.bar,
          below: _Line.bar,
          marker: _Marker.stop,
          aboveColor: first,
          belowColor: first,
          at: _stopAt(context, strong: true),
        ),
        child: _StopText(
          visit: ride.boarding,
          time: leaves,
          live: live.$1,
          scheduled: departure != null
              ? departure.scheduledDeparture
              : ride.boarding.scheduledDeparture,
          keepsScheduled: mayBeOff,
          skipped: taken?.boarding.skipped ?? false,
          strong: true,
        ),
      ),
    );
    if (choice.connection != RideConnection.unknown) {
      rows.add(
        _RailRow(
          key: ValueKey(('departuresRow', index)),
          rail: _Rail.through(_Line.bar, color: first),
          // The tiles run on to the screen's edge.
          end: 0,
          child: DepartureRange(
            key: ValueKey(('departures', index)),
            choice: choice,
            access: widget.access,
            now: widget.now,
            onSelect: onSelect,
          ),
        ),
      );
    }
    final stops = taken?.stops ?? const <StopVisit>[];
    final lastStop = stops.length - 1;
    // The leg's own times, but at the ends the choice's.
    DateTime leftAt(int i) =>
        i == 0 && leaves != null ? leaves : stops[i].departure;
    DateTime reachedAt(int i) =>
        i == lastStop && arrives != null ? arrives : stops[i].arrival;
    for (final (s, segment) in segments.indexed) {
      final color = colorOf(segment);
      final from = segment?.firstStop.clamp(0, lastStop) ?? 0;
      final to = segment?.lastStop.clamp(0, lastStop) ?? lastStop;
      final between = taken == null
          ? const <StopVisit>[]
          : stops.sublist(
              (from + 1).clamp(0, lastStop),
              to.clamp(from + 1, lastStop + 1),
            );
      final key = (index, s);
      final open = _expanded.contains(key);
      if (between.isNotEmpty) {
        final minutes = reachedAt(to).difference(leftAt(from));
        rows.add(
          _RailRow(
            key: ValueKey(('stops', index, s)),
            rail: _Rail.through(_Line.bar, color: color),
            onTap: () => setState(() {
              if (!_expanded.remove(key)) _expanded.add(key);
            }),
            child: _StopsToggle(
              count: between.length,
              minutes: formatDuration(minutes.inSeconds.toDouble()),
              open: open,
            ),
          ),
        );
        if (open) {
          for (final (k, visit) in between.indexed) {
            rows.add(
              _RailRow(
                key: ValueKey(('between', index, s, k, visit.stop.id)),
                rail: _Rail(
                  above: _Line.bar,
                  below: _Line.bar,
                  marker: _Marker.pass,
                  aboveColor: color,
                  belowColor: color,
                  at: _stopAt(context, strong: false),
                ),
                child: _StopText(
                  visit: visit,
                  time: visit.departure,
                  live: visit.predicted,
                  scheduled: visit.scheduledDeparture,
                  keepsScheduled: visit.predicted,
                  skipped: visit.skipped,
                ),
              ),
            );
          }
        }
      }
      if (segment != null && s + 1 < segments.length) {
        final nextSegment = segments[s + 1]!;
        final nextColor = colorOf(nextSegment);
        rows.add(
          _RailRow(
            key: ValueKey(('seat', index, s)),
            rail: _Rail(
              above: _Line.bar,
              below: _Line.bar,
              marker: _Marker.seat,
              aboveColor: color,
              belowColor: nextColor,
              icon: Icons.airline_seat_recline_normal,
            ),
            child: _ContinueText(
              ride: taken!,
              segment: nextSegment,
              at: stops[nextSegment.firstStop.clamp(0, lastStop)],
            ),
          ),
        );
      }
    }
    rows.add(
      _RailRow(
        key: ValueKey(('alighting', index)),
        rail: _Rail(
          above: _Line.bar,
          below: below,
          marker: _Marker.stop,
          aboveColor: last,
          at: _stopAt(context, strong: true),
        ),
        child: _StopText(
          visit: ride.alighting,
          time: arrives,
          live: live.$2,
          scheduled: departure != null
              ? departure.scheduledArrival
              : ride.alighting.scheduledArrival,
          keepsScheduled: mayBeOff,
          skipped: taken?.alighting.skipped ?? false,
          strong: true,
        ),
      ),
    );
    return rows;
  }
}

/// Whether a street leg is a change of vehicle at the same stop, with nothing to walk.
bool _isChange(StreetLeg leg) =>
    leg.kind == StreetLegKind.transfer &&
    (leg.walk == null || leg.walk!.distance < 1) &&
    (leg.from?.name == leg.to?.name || leg.duration < 30);

/// Whether the rider may wait for [choice]'s ride, whatever's taken of it and of the
/// rides before, and whatever the time: the plan has a wait of a minute or more; or,
/// for a ride after the first whose walk to it is known, more than one of the
/// departures it lists runs, since a later one is a wait.
bool _mayWait(RideChoice choice) {
  final ride = choice.ride;
  if (ride.waitSeconds >= 60) return true;
  if (choice.first || ride.transferSeconds == null) return false;
  return choice.departures.where((d) => d.status.runs).length > 1;
}

/// [scheduled] as it'd show beside [live]: without the day period they share, so "5:36"
/// before "5:40 PM".
String _beside(String scheduled, String live) {
  final cut = scheduled.lastIndexOf(' ');
  if (cut <= 0) return scheduled;
  final suffix = scheduled.substring(cut);
  return live.endsWith(suffix) ? scheduled.substring(0, cut) : scheduled;
}

/// [text], a clock time at the end of a row, after [prefix], in a space as wide as
/// any clock time there, so what's beside it lays out the same whatever the time; a
/// change cross-fades.
///
/// Where a live time isn't the scheduled one, the [scheduled] time comes first, small
/// and struck through. Where one may come, it keeps the room for it, so what's beside
/// it lays out the same whether it comes or not.
class _TimeSlot extends StatelessWidget {
  const _TimeSlot(
    this.text, {
    this.prefix = '',
    this.style,
    this.alignment = AlignmentDirectional.centerEnd,
    this.scheduled,
    this.scheduledStyle,
    this.keepsScheduled = false,
  });

  final String text;
  final String prefix;
  final TextStyle? style;

  /// Where the time sits in its space: at its end beside a row's text, at its start
  /// on a line of its own.
  final AlignmentGeometry alignment;

  /// The scheduled time, struck through before [text], or null for none.
  final String? scheduled;
  final TextStyle? scheduledStyle;

  /// Whether to keep the room for a scheduled time while there's none.
  final bool keepsScheduled;

  @override
  Widget build(BuildContext context) {
    final style = (this.style ?? DefaultTextStyle.of(context).style).copyWith(
      fontFeatures: _figures,
    );
    final localizations = MaterialLocalizations.of(context);
    final h24 = MediaQuery.alwaysUse24HourFormatOf(context);
    String widest(int hour) => localizations.formatTimeOfDay(
      TimeOfDay(hour: hour, minute: 0),
      alwaysUse24HourFormat: h24,
    );
    Widget hidden(String text, TextStyle? style) => ExcludeSemantics(
      child: Visibility(
        visible: false,
        maintainSize: true,
        maintainAnimation: true,
        maintainState: true,
        child: Text(text, style: style),
      ),
    );
    final time = Stack(
      alignment: alignment,
      children: [
        for (final hour in const [10, 22])
          hidden('$prefix${widest(hour)}', style),
        FadeSwap(
          value: text,
          alignment: alignment,
          // No time is nothing for a screen reader to announce.
          child: ExcludeSemantics(
            excluding: text.isEmpty,
            child: Text(text, style: style, maxLines: 1, softWrap: false),
          ),
        ),
      ],
    );
    final scheduled = this.scheduled;
    if (scheduled == null && !keepsScheduled) return time;
    final struck = (scheduledStyle ?? style).copyWith(
      fontFeatures: _figures,
      decoration: TextDecoration.lineThrough,
    );
    return Row(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.baseline,
      textBaseline: TextBaseline.alphabetic,
      children: [
        Stack(
          alignment: AlignmentDirectional.centerEnd,
          children: [
            for (final hour in const [10, 22])
              hidden(_beside(widest(hour), widest(hour)), struck),
            FadeSwap(
              value: scheduled,
              alignment: AlignmentDirectional.centerEnd,
              child: Text(
                scheduled ?? '',
                style: struck,
                maxLines: 1,
                softWrap: false,
              ),
            ),
          ],
        ),
        const SizedBox(width: 6),
        time,
      ],
    );
  }
}

/// A row's [text] with its clock [time]: at the row's end beside it, where the text
/// gives way after two lines; or, at the roomier text sizes (see [isRoomyText]), on a
/// line of its own above it, where the text takes as many lines as it needs. Other
/// things after the text are [trailing].
class _Timed extends StatelessWidget {
  const _Timed({
    required this.text,
    required this.style,
    required this.time,
    required this.timeStyle,
    this.trailing = const [],
  });

  final String text;
  final TextStyle? style;

  /// The style of the time's text, for the room its line takes.
  final TextStyle? timeStyle;

  /// The time, in its slot, or null for none.
  final Widget Function(AlignmentGeometry alignment)? time;
  final List<Widget> trailing;

  @override
  Widget build(BuildContext context) {
    final roomy = isRoomyText(MediaQuery.textScalerOf(context));
    Widget name(double? timeRoom) => Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // The text, and right after it what follows it, in all the room the time
        // leaves.
        Expanded(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Flexible(
                child: Text(
                  text,
                  style: style,
                  maxLines: roomy ? null : 2,
                  overflow: roomy ? null : TextOverflow.ellipsis,
                ),
              ),
              ...trailing,
            ],
          ),
        ),
        if (timeRoom != null) ...[
          const SizedBox(width: 8),
          // The time keeps its room at the row's end, but for a row too narrow for it
          // and any text, where it shrinks into half of it.
          ConstrainedBox(
            constraints: BoxConstraints(maxWidth: timeRoom),
            child: FittedLine(
              style: timeStyle,
              alignment: AlignmentDirectional.centerEnd,
              child: time!(AlignmentDirectional.centerEnd),
            ),
          ),
        ],
      ],
    );
    if (!roomy) {
      if (time == null) return name(null);
      return LayoutBuilder(
        builder: (context, constraints) => name(constraints.maxWidth / 2),
      );
    }
    if (time == null) return name(null);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        FittedLine(
          style: timeStyle,
          child: time!(AlignmentDirectional.centerStart),
        ),
        name(null),
      ],
    );
  }
}

/// The trip's start or its end, with when it leaves or arrives, or that it doesn't
/// connect.
class _EndText extends StatelessWidget {
  const _EndText({
    required this.name,
    required this.time,
    required this.prefix,
    this.connects = true,
  });

  /// The room above and below its text.
  static const padding = 5.0;

  final String name;
  final String time;

  /// What [time] says before its clock time.
  final String prefix;

  /// Whether the trip gets there: if not, [time] says so in the error color.
  final bool connects;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final style = theme.textTheme.bodyMedium?.copyWith(
      color: connects ? theme.colorScheme.onSurfaceVariant : null,
    );
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: padding),
      child: _Timed(
        text: name,
        style: theme.textTheme.titleSmall,
        timeStyle: theme.textTheme.bodyMedium,
        time: (alignment) => _TimeSlot(
          time,
          prefix: prefix,
          style: connects
              ? style
              : style?.copyWith(
                  color: theme.colorScheme.error,
                  fontWeight: FontWeight.w500,
                ),
          alignment: alignment,
        ),
      ),
    );
  }
}

/// The change or wait before a ride. Its height is the same whatever the wait, so the
/// rows below it stay where they are: on one line, or at the roomier text sizes (see
/// [isRoomyText]) a line for the change, if there is one, and a line for the wait,
/// each shrunk to fit or cut short rather than wrapped.
///
/// With nothing to wait for, the ride having no connection, it says only the change,
/// if there is one, or nothing, with no icon; the rail runs through it all the same.
/// What it says cross-fades.
class _WaitText extends StatelessWidget {
  const _WaitText({required this.change, required this.seconds});

  /// The stop where the rider changes vehicles without walking, if they do.
  final String? change;

  /// The wait, or null with nothing to wait for.
  final int? seconds;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final roomy = isRoomyText(MediaQuery.textScalerOf(context));
    final muted = theme.textTheme.bodyMedium?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final seconds = this.seconds;
    final blank = seconds == null;
    final wait = seconds != null && seconds >= 60
        ? formatDuration(seconds.toDouble())
        : null;
    final change = this.change;
    const iconSize = 18.0;
    // Kept in its room when there's none, so the row is as tall either way.
    final icon = FadeSwap(
      value: change == null && blank,
      child: change == null && blank
          ? const SizedBox.square(dimension: iconSize)
          : Icon(
              change != null ? Icons.sync_alt : Icons.hourglass_empty,
              size: iconSize,
              color: theme.colorScheme.onSurfaceVariant,
            ),
    );
    final changing = change == null
        ? null
        : Text(
            'Change at $change',
            style: muted,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          );
    Widget waiting({required bool after}) {
      final said = switch ((after, blank, wait)) {
        (_, true, _) => '',
        (false, false, null) => 'No wait',
        (false, false, final wait?) => 'Wait $wait',
        (true, false, null) => '',
        (true, false, final wait?) => ', wait $wait',
      };
      return FadeSwap(
        value: (blank, wait),
        // Nothing said is nothing for a screen reader to announce.
        child: ExcludeSemantics(
          excluding: said.isEmpty,
          child: Text(said, style: muted, maxLines: 1, softWrap: false),
        ),
      );
    }

    final Widget content;
    if (roomy) {
      content = Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (changing != null)
            Row(
              children: [
                icon,
                const SizedBox(width: 8),
                Expanded(child: changing),
              ],
            ),
          Row(
            children: [
              if (changing == null) ...[icon, const SizedBox(width: 8)],
              Expanded(
                child: FittedLine(style: muted, child: waiting(after: false)),
              ),
            ],
          ),
        ],
      );
    } else {
      content = Row(
        children: [
          icon,
          const SizedBox(width: 8),
          if (changing != null) Flexible(child: changing),
          waiting(after: changing != null),
        ],
      );
    }
    return ConstrainedBox(
      constraints: const BoxConstraints(minHeight: 30),
      child: Align(
        alignment: AlignmentDirectional.centerStart,
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 4),
          child: content,
        ),
      ),
    );
  }
}

/// A walk or ride on the streets, how long and how far, its steps a tap away, and any
/// moved stop at its end above it.
class _StreetRow extends StatefulWidget {
  const _StreetRow({super.key, required this.leg, required this.units});

  final StreetLeg leg;

  /// Metric or imperial, for the distances on foot.
  final UnitSystem units;

  @override
  State<_StreetRow> createState() => _StreetRowState();
}

class _StreetRowState extends State<_StreetRow> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: scheme.onSurfaceVariant,
    );
    final roomy = isRoomyText(MediaQuery.textScalerOf(context));
    final leg = widget.leg;
    final walk = leg.walk;
    final steps = walk?.maneuvers ?? const [];
    return _RailRow(
      rail: const _Rail.through(_Line.dotted),
      onTap: steps.isEmpty ? null : () => setState(() => _open = !_open),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // A moved stop is said first, where the rider reads before setting out.
          for (final notice in leg.stopMoved) StopMovedBanner(notice: notice),
          ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 44),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 4),
              child: Row(
                children: [
                  Icon(
                    streetIcon(leg.mode),
                    size: 18,
                    color: scheme.onSurfaceVariant,
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text.rich(
                      TextSpan(
                        children: [
                          TextSpan(
                            text:
                                '${streetVerb(leg.mode)} '
                                '${formatDuration(leg.duration.toDouble())}',
                          ),
                          if (walk != null)
                            TextSpan(
                              text:
                                  ' · '
                                  '${formatDistance(walk.distance, units: widget.units)}',
                              style: TextStyle(color: scheme.onSurfaceVariant),
                            ),
                        ],
                      ),
                      style: theme.textTheme.bodyMedium,
                      maxLines: roomy ? null : 2,
                      overflow: roomy ? null : TextOverflow.ellipsis,
                    ),
                  ),
                  if (steps.isNotEmpty)
                    Icon(
                      _open ? Icons.expand_less : Icons.expand_more,
                      size: 18,
                      color: scheme.onSurfaceVariant,
                    ),
                ],
              ),
            ),
          ),
          if (leg.mode == 'wheelchair' &&
              leg.accessibility != Accessibility.unknown)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: AccessibilityLine(accessibility: leg.accessibility),
            ),
          if (_open)
            Padding(
              padding: const EdgeInsetsDirectional.only(start: 26, bottom: 10),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                spacing: 8,
                children: [
                  for (final m in steps)
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Expanded(
                          child: Text(
                            m.instruction,
                            style: theme.textTheme.bodySmall,
                          ),
                        ),
                        if (m.distance > 0) ...[
                          const SizedBox(width: 12),
                          Text(
                            formatDistance(m.distance, units: widget.units),
                            style: muted,
                          ),
                        ],
                      ],
                    ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// What a ride's title has said, or would say, for any departure it has listed in an
/// opening of Trip details: the room it keeps at the roomier text sizes. It only
/// grows, so a newer list, or a newer plan of the journey, that drops a long headsign
/// never makes the title shorter; one that brings a longer one makes it taller as the
/// plan is shown, once no finger is down, so choosing that departure later moves
/// nothing.
class _TitleRoom {
  final headings = <String>{};
  final runnings = <String>{};

  /// Adds what [choice]'s ride would say for each of its departures, and for its own.
  void add(RideChoice choice) {
    final ride = choice.ride;
    headings.add(_RideTitle.heading(ride.route, ride.headsign));
    runnings.add(_RideTitle.running(ride, ride.route));
    for (final d in choice.departures) {
      final route = d.route ?? ride.route;
      headings.add(
        _RideTitle.heading(
          route,
          d.suggested ? ride.headsign : d.headsign ?? ride.headsign,
        ),
      );
      runnings.add(_RideTitle.running(ride, route));
    }
    headings.add(_RideTitle.heading(choice.route, choice.headsign));
    runnings.add(_RideTitle.running(ride, choice.route));
  }
}

/// A ride's route, where it's headed and who runs it, as the departure taken has them.
///
/// Its height doesn't change with the departure chosen, as it's above the ride's
/// departures: on one line each for where it's headed and who runs it; or, at the
/// roomier text sizes (see [isRoomyText]), the badge on a line and those two under it,
/// each wrapping as it needs and keeping the room of the longest any of the ride's
/// departures would need ([room]).
class _RideTitle extends StatelessWidget {
  const _RideTitle({
    required this.ride,
    required this.room,
    required this.route,
    required this.headsign,
  });

  final TransitLeg ride;

  /// What any of the ride's departures would say here, for the room the longest
  /// takes: see [_TitleRoom].
  final _TitleRoom room;

  /// The route and headsign of the departure taken.
  final TransitRoute? route;
  final String? headsign;

  static String heading(TransitRoute? route, String? headsign) =>
      headsign ?? route?.longName ?? '';

  static String running(TransitLeg ride, TransitRoute? route) =>
      [?ride.agency, (route?.kind ?? VehicleKind.other).label].join(' · ');

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final roomy = isRoomyText(MediaQuery.textScalerOf(context));
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final route = this.route;
    final badge = AnimatedSize(
      duration: rowDuration,
      curve: rowCurve,
      alignment: AlignmentDirectional.centerStart,
      child: FadeSwap(
        value: route?.id,
        child: RouteBadge(route: route),
      ),
    );
    if (roomy) {
      Widget keeping(
        String shown,
        Iterable<String> all,
        TextStyle? style,
        Object? value,
      ) => Stack(
        children: [
          for (final other in all.toSet())
            if (other != shown)
              ExcludeSemantics(
                child: Visibility(
                  visible: false,
                  maintainSize: true,
                  maintainAnimation: true,
                  maintainState: true,
                  child: Text(other, style: style),
                ),
              ),
          FadeSwap(
            value: value,
            alignment: AlignmentDirectional.topStart,
            child: Text(shown, style: style),
          ),
        ],
      );
      return Padding(
        padding: const EdgeInsets.only(top: 8, bottom: 4),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            badge,
            const SizedBox(height: 2),
            keeping(
              heading(route, headsign),
              room.headings,
              theme.textTheme.titleSmall,
              headsign ?? route?.longName,
            ),
            keeping(running(ride, route), room.runnings, muted, route?.kind),
          ],
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.only(top: 8, bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // One line each, so a choice of departure above the ride's departures
          // never changes their height. A long route's name gives way, in at most
          // 40% of the row.
          Flexible(flex: 0, child: _BadgeRoom(child: badge)),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                FadeSwap(
                  value: headsign ?? route?.longName,
                  child: Text(
                    heading(route, headsign),
                    style: theme.textTheme.titleSmall,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                FadeSwap(
                  value: route?.kind,
                  child: Text(
                    running(ride, route),
                    style: muted,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// Gives a route's badge, beside what else a row says, at most 40% of the row.
class _BadgeRoom extends StatelessWidget {
  const _BadgeRoom({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => ConstrainedBox(
    constraints: BoxConstraints(
      maxWidth: (MediaQuery.sizeOf(context).width - _contentStart) * 0.4,
    ),
    child: child,
  );
}

/// A stop on a ride, with its time at the row's end: one the rider gets on or off at
/// ([strong]) as large as the rider reads it, one passed small. A live time is in the
/// live color; where it isn't the [scheduled] one, that comes before it, struck
/// through.
///
/// A stop got on or off at keeps the room for its time while it has none (its ride
/// has no connection, or follows one without), so the row is as tall either way and
/// the departures below it stay where they are; the time cross-fades in and out.
class _StopText extends StatelessWidget {
  const _StopText({
    required this.visit,
    required this.time,
    required this.live,
    required this.skipped,
    this.scheduled,
    this.keepsScheduled = false,
    this.strong = false,
  });

  /// The room above and below a stop's text whose first line is [line] tall: a stop
  /// passed is in a row at least 28 high.
  static double paddingFor(double line, {required bool strong}) =>
      strong ? 4 : math.max(2, (28 - line) / 2);

  final StopVisit visit;

  /// When it's left or reached, or null when that isn't known.
  final DateTime? time;
  final bool live;

  /// When it's scheduled to be left or reached, if that's known.
  final DateTime? scheduled;

  /// Whether to keep the room for a scheduled time while none shows, as a row above a
  /// ride's departures does where another of them may bring one.
  final bool keepsScheduled;

  /// Whether the trip skips it.
  final bool skipped;

  /// Whether the rider gets on or off here, rather than passing it.
  final bool strong;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final text = theme.textTheme;
    final muted = theme.colorScheme.onSurfaceVariant;
    final nameStyle = strong ? text.titleMedium : text.bodySmall;
    final timeStyle = strong ? text.bodyLarge : text.bodySmall;
    final stop = visit.stop;
    final name = [
      stop.name,
      if (stop.platform != null) '(platform ${stop.platform})',
    ].join(' ');
    final time = this.time;
    final shown = time == null
        ? null
        : clock(context, time, utcOffset: visit.utcOffset);
    final planned = switch (this.scheduled) {
      final s? when live && shown != null && !skipped => clock(
        context,
        s,
        utcOffset: visit.utcOffset,
      ),
      _ => null,
    };
    final scheduled = planned == null || planned == shown
        ? null
        : _beside(planned, shown!);
    final roomy = isRoomyText(MediaQuery.textScalerOf(context));
    final line = textLinesHeight(
      context,
      (roomy && strong ? timeStyle : nameStyle) ?? const TextStyle(),
    );
    return Padding(
      padding: EdgeInsets.symmetric(vertical: paddingFor(line, strong: strong)),
      child: _Timed(
        text: name,
        style: nameStyle?.copyWith(
          decoration: skipped ? TextDecoration.lineThrough : null,
          color: skipped ? muted : null,
        ),
        trailing: [
          if (stop.accessibility == Accessibility.inaccessible)
            Padding(
              padding: const EdgeInsetsDirectional.only(start: 6, top: 2),
              child: Icon(
                Icons.not_accessible,
                size: 16,
                semanticLabel: 'Not wheelchair accessible',
                color: theme.colorScheme.error,
              ),
            ),
        ],
        timeStyle: timeStyle,
        // A stop got on or off at keeps its time's room, empty while the time isn't
        // known, so the row is as tall either way; one passed has its time or none.
        time: switch (shown) {
          _ when shown != null || strong => (alignment) => _TimeSlot(
            shown ?? '',
            style: timeStyle?.copyWith(
              color: skipped
                  ? muted
                  : live
                  ? liveColor(theme)
                  : strong
                  ? null
                  : muted,
              decoration: skipped ? TextDecoration.lineThrough : null,
            ),
            alignment: alignment,
            scheduled: scheduled,
            scheduledStyle: text.bodySmall?.copyWith(color: muted),
            // On a line of its own, a scheduled time moves nothing beside it.
            keepsScheduled: keepsScheduled && !roomy,
          ),
          _ => null,
        },
      ),
    );
  }
}

/// How many stops a ride passes between two the rider gets on and off at, and how long
/// that takes, as a toggle for them.
class _StopsToggle extends StatelessWidget {
  const _StopsToggle({
    required this.count,
    required this.minutes,
    required this.open,
  });

  final int count;
  final String minutes;
  final bool open;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.colorScheme.onSurfaceVariant;
    return ConstrainedBox(
      constraints: const BoxConstraints(minHeight: 44),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 4),
        child: Row(
          children: [
            Expanded(
              child: Text.rich(
                TextSpan(
                  children: [
                    TextSpan(text: count == 1 ? '1 stop' : '$count stops'),
                    TextSpan(
                      text: ' · $minutes',
                      style: TextStyle(color: muted),
                    ),
                  ],
                ),
                style: theme.textTheme.bodyMedium,
              ),
            ),
            const SizedBox(width: 6),
            Icon(
              open ? Icons.expand_less : Icons.expand_more,
              size: 18,
              color: muted,
            ),
          ],
        ),
      ),
    );
  }
}

/// Where a stay-seated ride's vehicle goes on as another route.
class _ContinueText extends StatelessWidget {
  const _ContinueText({
    required this.ride,
    required this.segment,
    required this.at,
  });

  final TransitLeg ride;
  final RideSegment segment;
  final StopVisit at;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Stay on board', style: theme.textTheme.titleSmall),
          const SizedBox(height: 3),
          Wrap(
            spacing: 6,
            runSpacing: 3,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text('At ${at.stop.name} it continues as', style: muted),
              RouteBadge(route: segmentRoute(ride, segment)),
              if (segment.headsign case final headsign?)
                Text('to $headsign', style: muted),
            ],
          ),
        ],
      ),
    );
  }
}

/// A ride's departures, as tiles counting down to compare and choose from, and one
/// place that says how the chosen one is running: live or scheduled, how accessible,
/// and its alerts. Or, when the rider can make none of them, that there's no
/// connection.
///
/// The suggested departure, the plan's own, says so. Departures the rider can't make,
/// and ones that don't run, are muted and can't be chosen. The countdowns tick each
/// second from [now].
class DepartureRange extends StatefulWidget {
  const DepartureRange({
    super.key,
    required this.choice,
    required this.access,
    required this.now,
    required this.onSelect,
  });

  final RideChoice choice;
  final TransitAccess access;
  final DateTime now;

  /// The rider chose a departure, by its index in the ride's departures.
  final ValueChanged<int> onSelect;

  @override
  State<DepartureRange> createState() => _DepartureRangeState();
}

class _DepartureRangeState extends State<DepartureRange> {
  late final _now = ValueNotifier(widget.now);
  Timer? _tick;

  /// The tiles' row, scrolled so the chosen tile shows: a choice upstream can choose one
  /// far along the row, out of sight.
  final _tiles = ScrollController();

  /// Each departure's tile, by its run, to scroll to.
  final _tileKeys = <DepartureRun, GlobalKey>{};

  /// The departure the rider last tapped, until the choice comes back.
  DepartureRun? _tapped;

  static DepartureRun? _run(RideDeparture? d) =>
      d == null ? null : (tripId: d.tripId, startDate: d.startDate);

  GlobalKey _keyOf(RideDeparture d) =>
      _tileKeys.putIfAbsent(_run(d)!, GlobalKey.new);

  @override
  void initState() {
    super.initState();
    _start();
  }

  /// Scrolls the tiles' row, and only that row, to the chosen tile after this frame:
  /// centered on it for a choice made elsewhere, and only as far as shows all of it for
  /// one the rider tapped, so it doesn't slide from under their finger.
  void _reveal({required bool center}) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final chosen = widget.choice.departure;
      final tile = chosen == null
          ? null
          : _tileKeys[_run(chosen)]?.currentContext?.findRenderObject();
      if (!mounted || tile == null || !_tiles.hasClients) return;
      final position = _tiles.position;
      final viewport = RenderAbstractViewport.maybeOf(tile);
      if (viewport == null) return;
      final double to;
      if (center) {
        to = viewport.getOffsetToReveal(tile, 0.5).offset;
      } else {
        final start = viewport.getOffsetToReveal(tile, 0).offset;
        final end = viewport.getOffsetToReveal(tile, 1).offset;
        to = position.pixels.clamp(math.min(start, end), math.max(start, end));
      }
      final target = to.clamp(
        position.minScrollExtent,
        position.maxScrollExtent,
      );
      if ((target - position.pixels).abs() < 0.5) return;
      position.animateTo(target, duration: rowDuration, curve: rowCurve);
    });
  }

  /// Counts on from [DepartureRange.now], a second at a time.
  void _start() {
    _tick?.cancel();
    _now.value = widget.now;
    _tick = Timer.periodic(const Duration(seconds: 1), (timer) {
      _now.value = widget.now.add(Duration(seconds: timer.tick));
    });
  }

  @override
  void didUpdateWidget(DepartureRange oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.now != widget.now) _start();
    final chosen = _run(widget.choice.departure);
    if (_run(oldWidget.choice.departure) != chosen) {
      _reveal(center: chosen != _tapped);
    }
    _tapped = null;
    final runs = {for (final d in widget.choice.departures) _run(d)};
    _tileKeys.removeWhere((run, _) => !runs.contains(run));
  }

  @override
  void dispose() {
    _tick?.cancel();
    _tiles.dispose();
    _now.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final choice = widget.choice;
    final ride = choice.ride;
    final departures = choice.departures;
    // Each tile badges its route when they aren't all the ride's.
    final mixed = departures.any((d) => d.route != null);
    return Padding(
      padding: const EdgeInsets.only(top: 6, bottom: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (departures.length > 1) ...[
            // The tiles scroll on to the end of the row, which can be the screen's
            // edge, and end a margin short of it.
            LayoutBuilder(
              builder: (context, constraints) => ValueListenableBuilder(
                valueListenable: _now,
                builder: (context, now, _) {
                  final countdowns = [
                    for (final d in departures) Countdown(d, now: now),
                  ];
                  return SingleChildScrollView(
                    controller: _tiles,
                    scrollDirection: Axis.horizontal,
                    child: _CenterAtFirst(
                      controller: _tiles,
                      tile: choice.departure == null
                          ? null
                          : _keyOf(choice.departure!),
                      viewport: constraints.maxWidth,
                      child: Padding(
                        padding: const EdgeInsetsDirectional.only(end: 16),
                        child: IntrinsicHeight(
                          child: Row(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            spacing: 8,
                            children: [
                              for (final (i, d) in departures.indexed)
                                ConnectionTile(
                                  key: _keyOf(d),
                                  departure: d,
                                  route: mixed ? d.route ?? ride.route : null,
                                  countdown: countdowns[i],
                                  catchable: choice.catchable[i],
                                  selected: i == choice.index,
                                  utcOffset: ride.boarding.utcOffset,
                                  onTap: () {
                                    _tapped = _run(d);
                                    widget.onSelect(i);
                                  },
                                ),
                            ],
                          ),
                        ),
                      ),
                    ),
                  );
                },
              ),
            ),
            const SizedBox(height: 8),
          ],
          Padding(
            padding: const EdgeInsetsDirectional.only(end: 16),
            child: FadeSwap(
              value: (choice.connection, _run(choice.departure)),
              alignment: AlignmentDirectional.topStart,
              fit: StackFit.passthrough,
              child: choice.connection == RideConnection.none
                  ? NoConnection(choice: choice)
                  : VehicleStatus(
                      ride: ride,
                      departure: choice.departure,
                      access: widget.access,
                    ),
            ),
          ),
        ],
      ),
    );
  }
}

/// [child], a row of tiles in [controller]'s horizontal scroll view [viewport] wide,
/// scrolled to center [tile] in its first layout, before it's painted.
class _CenterAtFirst extends SingleChildRenderObjectWidget {
  const _CenterAtFirst({
    required this.controller,
    required this.tile,
    required this.viewport,
    super.child,
  });

  final ScrollController controller;
  final GlobalKey? tile;
  final double viewport;

  @override
  _RenderCenterAtFirst createRenderObject(BuildContext context) =>
      _RenderCenterAtFirst(controller, tile, viewport);

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderCenterAtFirst renderObject,
  ) {
    renderObject
      ..tile = tile
      ..viewport = viewport;
  }
}

class _RenderCenterAtFirst extends RenderProxyBox {
  _RenderCenterAtFirst(this.controller, this.tile, this.viewport);

  final ScrollController controller;
  GlobalKey? tile;
  double viewport;
  bool _done = false;

  @override
  void performLayout() {
    super.performLayout();
    if (_done) return;
    _done = true;
    final box = tile?.currentContext?.findRenderObject();
    if (box is! RenderBox || !box.hasSize || !controller.hasClients) return;
    double leftOf(RenderBox b) =>
        b.localToGlobal(Offset.zero, ancestor: this).dx;
    // Its right, from the next tile's left: only its row may read its size here.
    final row = box.parent;
    final next = row is RenderFlex ? row.childAfter(box) : null;
    final left = leftOf(box);
    final right = next == null
        ? size.width
        : leftOf(next) - (row as RenderFlex).spacing;
    final to = ((left + right) / 2 - viewport / 2)
        .clamp(0.0, math.max(0.0, size.width - viewport))
        .toDouble();
    // The scroll view applies its extent after this, and paints from here.
    controller.position.correctPixels(to);
  }
}

/// One departure of a ride, as a tile to choose: its route's badge, when the ride's
/// departures are of several routes; its countdown, with the live mark and in the live
/// color when its time is real-time, and the clock time it leaves at, with how late or
/// early it runs; and whether it's the suggested one or the day's last. One that
/// doesn't run is struck through with a label, and one the rider can't make shows its
/// clock time with a label; neither can be chosen, and both are flat and muted.
///
/// As whether it can be made changes, its countdown and clock time, and its label,
/// cross-fade. Every tile keeps a line for its clock time and one for its label,
/// empty or not, so no tile, and no row of them, changes height with a choice, with
/// time passing, or with new data.
class ConnectionTile extends StatelessWidget {
  const ConnectionTile({
    super.key,
    required this.departure,
    required this.route,
    required this.countdown,
    required this.catchable,
    required this.selected,
    required this.utcOffset,
    required this.onTap,
  });

  /// Whether a departure leaving as [countdown] says would count down, if it can be
  /// made: within the hour, and not yet gone.
  static bool inCountdown(Countdown countdown) =>
      !countdown.departed && countdown.minutesUntil <= countdownMinutes;

  final RideDeparture departure;

  /// Its route, to badge, or null for no badge.
  final TransitRoute? route;
  final Countdown countdown;

  /// Whether the rider can make it, as the rides before have it.
  final bool catchable;
  final bool selected;

  /// The boarding stop's offset from UTC, for its clock time.
  final int? utcOffset;
  final VoidCallback onTap;

  static const _minWidth = 76.0;
  static const _radius = 12.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final text = theme.textTheme;
    final d = departure;
    final struck = !d.status.runs;
    final usable = !struck && catchable;
    final faded = scheme.onSurface.withValues(alpha: 0.45);
    final muted = text.bodySmall?.copyWith(
      color: scheme.onSurfaceVariant,
      fontFeatures: _figures,
    );
    final time = clock(context, d.departure, utcOffset: utcOffset);
    // One the rider can't make, or past the hour, or once it has left, shows the clock
    // time in place of a countdown.
    final counts = usable && inCountdown(countdown);
    final (label, labelColor) = switch (d) {
      RideDeparture(status: DepartureStatus.canceled) => ('Canceled', faded),
      RideDeparture(status: DepartureStatus.skipped) => ('Skips stop', faded),
      _ when !catchable => ('Missed', faded),
      RideDeparture(suggested: true, lastOfDay: true) => (
        'Suggested · Last',
        scheme.primary,
      ),
      RideDeparture(suggested: true) => ('Suggested', scheme.primary),
      RideDeparture(lastOfDay: true) => ('Last', scheme.error),
      _ => (null, null),
    };
    // How far off its schedule it runs, after its clock time.
    final delay = d.delaySeconds ?? 0;
    final off = (delay.abs() / 60).round();
    final (offText, offColor) = !usable || off == 0
        ? (null, null)
        : delay > 0
        ? ('$off min late', scheme.error)
        : ('$off min early', liveColor(theme));
    final clockLine = [if (counts) time, ?offText].join(' · ');
    final clockStyle = muted?.copyWith(
      color: selected ? scheme.onSurface : null,
    );
    final number = text.titleMedium?.copyWith(fontWeight: FontWeight.w700);
    final route = this.route;
    final border = selected
        ? Border.all(color: scheme.primary, width: 2)
        : Border.all(
            color: usable
                ? scheme.outline
                : scheme.outline.withValues(alpha: 0),
          );
    final fill = selected
        ? scheme.primary.withValues(alpha: 0.12)
        : usable
        ? scheme.primary.withValues(alpha: 0)
        : scheme.surfaceContainerHigh;
    return Semantics(
      button: usable,
      selected: selected,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        // The border is drawn over the tile, so its width never moves what's in it.
        decoration: BoxDecoration(
          color: fill,
          borderRadius: BorderRadius.circular(_radius),
        ),
        foregroundDecoration: BoxDecoration(
          borderRadius: BorderRadius.circular(_radius),
          border: border,
        ),
        child: Material(
          type: MaterialType.transparency,
          child: InkWell(
            borderRadius: BorderRadius.circular(_radius),
            onTap: usable ? onTap : null,
            child: ConstrainedBox(
              constraints: const BoxConstraints(minWidth: _minWidth),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(12, 9, 12, 9),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  spacing: 1,
                  children: [
                    if (route != null)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 3),
                        child: AnimatedOpacity(
                          duration: rowDuration,
                          curve: rowCurve,
                          opacity: usable ? 1 : 0.5,
                          child: RouteBadge(route: route),
                        ),
                      ),
                    FadeSwap(
                      value: counts ? null : (time, usable, struck),
                      child: counts
                          ? CountdownNumber(
                              countdown,
                              key: const ValueKey('countdown'),
                              numberStyle: number,
                              unitStyle: number,
                              liveSize: 12,
                              liveColored: true,
                              alignment: AlignmentDirectional.centerStart,
                            )
                          : Text(
                              time,
                              key: const ValueKey('countdown'),
                              style: number?.copyWith(
                                color: usable ? null : faded,
                                fontFeatures: _figures,
                                decoration: struck
                                    ? TextDecoration.lineThrough
                                    : null,
                              ),
                            ),
                    ),
                    FadeSwap(
                      value: clockLine,
                      child: Row(
                        key: const ValueKey('clock'),
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(
                            counts ? time : '',
                            style: clockStyle,
                            maxLines: 1,
                            softWrap: false,
                          ),
                          if (offText != null)
                            Text(
                              counts ? ' · $offText' : offText,
                              style: clockStyle?.copyWith(
                                color: offColor,
                                fontWeight: FontWeight.w500,
                              ),
                              maxLines: 1,
                              softWrap: false,
                            ),
                        ],
                      ),
                    ),
                    FadeSwap(
                      value: (label, labelColor),
                      child: Text(
                        label ?? '',
                        key: const ValueKey('label'),
                        style: text.labelSmall?.copyWith(
                          color: labelColor,
                          fontWeight: FontWeight.w700,
                        ),
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

/// That a ride has no connection: each of its departures leaves before the rider can
/// be at its stop, with the departures chosen before it. For the first ride, every one
/// has gone: the day's last, if the last of them that runs is, and the rider is
/// pointed back to the other trips.
class NoConnection extends StatelessWidget {
  const NoConnection({super.key, required this.choice});

  final RideChoice choice;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final ready = choice.ready;
    final stop = choice.ride.boarding;
    final last = choice.first ? choice.lastRunning : null;
    final (icon, title, body) = switch (last) {
      _ when !choice.first => (
        Icons.link_off,
        'No connection',
        ready == null
            ? "You can't make any of these departures."
            : "You'd be at the stop at "
                  '${clock(context, ready, utcOffset: stop.utcOffset)}'
                  ', after all of these leave. An earlier departure on a ride '
                  'before may connect.',
      ),
      RideDeparture(lastOfDay: true) => (
        Icons.bedtime_outlined,
        'No more departures today',
        'The last one between these stops left at '
            '${clock(context, last.departure, utcOffset: stop.utcOffset)}. '
            'Go back to see other trips.',
      ),
      _ => (
        Icons.link_off,
        'No departures left',
        "You can't make any of these departures. Go back to see other trips.",
      ),
    };
    return Container(
      margin: const EdgeInsets.only(bottom: 2),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
      decoration: BoxDecoration(
        color: scheme.errorContainer,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 18, color: scheme.onErrorContainer),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              spacing: 1,
              children: [
                Text(
                  title,
                  style: theme.textTheme.labelLarge?.copyWith(
                    color: scheme.onErrorContainer,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                Text(
                  body,
                  style: theme.textTheme.bodySmall?.copyWith(
                    color: scheme.onErrorContainer,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// How one departure of a ride is running, in one place: a row of chips saying whether
/// it's live and how it stands against its schedule, or scheduled; how accessible it
/// is; how full, when that's known; whether it takes bikes, for a rider cycling to
/// stops; and whether it's the day's last. Then its alerts, a row each, each one's
/// description a tap away. When and where it goes are the ride's rows'.
///
/// With no [departure], it describes the ride's own.
class VehicleStatus extends StatelessWidget {
  const VehicleStatus({
    super.key,
    required this.ride,
    required this.departure,
    required this.access,
  });

  final TransitLeg ride;
  final RideDeparture? departure;
  final TransitAccess access;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final d = departure;
    final own = d == null || d.suggested;
    final status = d == null ? rideStatus(theme, ride) : _status(theme, d);
    final accessibility = d?.accessibility ?? ride.accessibility;
    final bikes = d?.bikesAllowed ?? ride.bikesAllowed;
    final occupancy = own ? ride.vehicle?.occupancy : null;
    final alerts = (own ? ride.alerts : d.alerts)
        .where((a) => a.effect != 'stop_moved')
        .where((a) => (a.header ?? a.effect) != null)
        .toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            _Chip(
              icon: status.live ? Icons.sensors : Icons.schedule,
              text: status.text,
              tone: !status.live
                  ? _Tone.neutral
                  : status.color == theme.colorScheme.error
                  ? _Tone.error
                  : _Tone.live,
            ),
            switch (accessibility) {
              Accessibility.accessible => const _Chip(
                icon: Icons.accessible,
                text: 'Wheelchair accessible',
              ),
              Accessibility.inaccessible => const _Chip(
                icon: Icons.not_accessible,
                text: 'Not wheelchair accessible',
                tone: _Tone.error,
              ),
              Accessibility.unknown => const _Chip(
                icon: Icons.accessible,
                text: 'Wheelchair access unknown',
              ),
            },
            if (occupancy != null)
              _Chip(icon: Icons.group_outlined, text: occupancyText(occupancy)),
            if (access == TransitAccess.cycling && bikes != null)
              _Chip(
                icon: Icons.pedal_bike,
                text: bikes == 'yes' ? 'Bikes allowed' : 'No bikes',
              ),
            if (d?.lastOfDay ?? false)
              const _Chip(
                icon: Icons.bedtime_outlined,
                text: 'The last departure of the day between these stops',
                tone: _Tone.error,
              ),
          ],
        ),
        if (alerts.isNotEmpty) const SizedBox(height: 2),
        for (final alert in alerts) _AlertLine(alert: alert),
      ],
    );
  }

  static ({String text, Color color, bool live}) _status(
    ThemeData theme,
    RideDeparture d,
  ) {
    final muted = theme.colorScheme.onSurfaceVariant;
    switch (d.status) {
      case DepartureStatus.canceled:
        return (text: 'Canceled', color: theme.colorScheme.error, live: true);
      case DepartureStatus.skipped:
        return (
          text: "Doesn't stop here any more",
          color: theme.colorScheme.error,
          live: true,
        );
      case DepartureStatus.scheduled:
        return (text: 'Scheduled', color: muted, live: false);
      case DepartureStatus.live:
        final delay = d.delaySeconds ?? 0;
        final minutes = (delay.abs() / 60).round();
        if (minutes == 0) {
          return (text: 'Live · on time', color: liveColor(theme), live: true);
        }
        return delay > 0
            ? (
                text: 'Live · $minutes min late',
                color: theme.colorScheme.error,
                live: true,
              )
            : (
                text: 'Live · $minutes min early',
                color: liveColor(theme),
                live: true,
              );
    }
  }
}

/// How a [_Chip] is colored: plainly, in the live color, or as something wrong.
enum _Tone { neutral, live, error }

/// A fact about a departure as a pill: an icon and a few words, 26 high at the
/// system's default text size and growing with its text, which wraps rather than run
/// past the row.
class _Chip extends StatelessWidget {
  const _Chip({
    required this.icon,
    required this.text,
    this.tone = _Tone.neutral,
  });

  final IconData icon;
  final String text;
  final _Tone tone;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final live = liveColor(theme);
    final (fill, ink) = switch (tone) {
      _Tone.neutral => (scheme.surfaceContainerHigh, scheme.onSurfaceVariant),
      _Tone.live => (live.withValues(alpha: 0.14), live),
      _Tone.error => (scheme.errorContainer, scheme.onErrorContainer),
    };
    final style = theme.textTheme.labelMedium?.copyWith(color: ink);
    final line = textLinesHeight(context, style ?? const TextStyle());
    return Container(
      constraints: const BoxConstraints(minHeight: 26),
      padding: EdgeInsetsDirectional.fromSTEB(
        8,
        math.max(2, (26 - line) / 2),
        10,
        math.max(2, (26 - line) / 2),
      ),
      decoration: BoxDecoration(
        color: fill,
        borderRadius: BorderRadius.circular(13),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 14, color: ink),
          const SizedBox(width: 5),
          Flexible(child: Text(text, style: style)),
        ],
      ),
    );
  }
}

/// An alert's header, as a row of its own with a divider under it; its description,
/// if it has one, a tap away below it.
class _AlertLine extends StatefulWidget {
  const _AlertLine({required this.alert});

  final LegAlert alert;

  @override
  State<_AlertLine> createState() => _AlertLineState();
}

class _AlertLineState extends State<_AlertLine> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final alert = widget.alert;
    final header = alert.header ?? alert.effect?.replaceAll('_', ' ');
    if (header == null) return const SizedBox.shrink();
    final description = alert.description == header ? null : alert.description;
    // With nothing more to say, its header shows whole; with more, it's one line
    // until it's opened.
    final shut = description != null && !_open;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        InkWell(
          onTap: description == null
              ? null
              : () => setState(() => _open = !_open),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              ConstrainedBox(
                constraints: const BoxConstraints(minHeight: 44),
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: 6),
                  child: Row(
                    children: [
                      Icon(
                        Icons.warning_amber,
                        size: 16,
                        color: warningColors(theme).icon,
                      ),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          header,
                          style: theme.textTheme.bodyMedium?.copyWith(
                            fontWeight: FontWeight.w500,
                          ),
                          maxLines: shut ? 1 : null,
                          overflow: shut ? TextOverflow.ellipsis : null,
                        ),
                      ),
                      if (description != null) ...[
                        const SizedBox(width: 6),
                        Icon(
                          _open ? Icons.expand_less : Icons.expand_more,
                          size: 18,
                          color: scheme.onSurfaceVariant,
                        ),
                      ],
                    ],
                  ),
                ),
              ),
              if (_open && description != null)
                Padding(
                  padding: const EdgeInsetsDirectional.fromSTEB(22, 0, 24, 12),
                  child: Text(
                    description,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: scheme.onSurfaceVariant,
                    ),
                  ),
                ),
            ],
          ),
        ),
        Divider(height: 1, thickness: 1, color: scheme.surfaceContainerHigh),
      ],
    );
  }
}
