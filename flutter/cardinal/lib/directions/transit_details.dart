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
import 'animated_rows.dart';
import 'connections.dart';
import 'format.dart';
import 'transit_plan.dart';
import 'transit_sheet.dart';
import 'transit_timeline.dart';

/// The top of a journey's details: back to the overview, when it leaves and arrives
/// with the departures chosen in [chain], and its timeline.
class JourneyHeader extends StatelessWidget {
  const JourneyHeader({
    super.key,
    required this.chain,
    required this.now,
    required this.units,
    required this.onBack,
  });

  /// The journey, with the departures chosen of its rides.
  final ConnectionChain chain;
  final DateTime now;

  /// Metric or imperial, for the distances on foot.
  final UnitSystem units;
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final journey = chain.journey;
    final offset = journeyUtcOffset(journey);
    final street = journey.streetMeters;
    final arrival = chain.arrival;
    final duration = chain.duration;
    return Padding(
      padding: const EdgeInsets.fromLTRB(4, 0, 16, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              IconButton(
                tooltip: 'Back',
                icon: const Icon(Icons.arrow_back),
                onPressed: onBack,
              ),
              Expanded(
                child: Text(
                  'Trip details',
                  style: theme.textTheme.titleLarge,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              FadeSwap(
                value: duration == null ? null : duration ~/ 60,
                alignment: AlignmentDirectional.centerEnd,
                child: Text(
                  duration == null ? '' : formatDuration(duration.toDouble()),
                  style: theme.textTheme.titleLarge?.copyWith(
                    color: theme.colorScheme.primary,
                  ),
                  maxLines: 1,
                ),
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.only(left: 12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // One line, whatever the choices, so nothing below it moves: the
                // times shrink to fit beside a long "Go in" rather than wrap.
                Row(
                  crossAxisAlignment: CrossAxisAlignment.baseline,
                  textBaseline: TextBaseline.alphabetic,
                  children: [
                    // Cross-fades when the time to leave changes with a choice, not
                    // as it counts down.
                    FadeSwap(
                      value: chain.departure,
                      child: GoIn(
                        journey: journey,
                        now: now,
                        leave: chain.departure,
                        live: chain.rides.firstOrNull?.realtime,
                      ),
                    ),
                    const SizedBox(width: 8),
                    Expanded(
                      child: FittedBox(
                        fit: BoxFit.scaleDown,
                        alignment: AlignmentDirectional.centerEnd,
                        child: FadeSwap(
                          value: (chain.departure, arrival),
                          alignment: AlignmentDirectional.centerEnd,
                          child: Text(
                            '${clock(context, chain.departure, utcOffset: offset)}'
                            ' – '
                            '${arrival == null ? 'no connection' : clock(context, arrival, utcOffset: offset)}',
                            style: theme.textTheme.bodyMedium?.copyWith(
                              color: arrival == null
                                  ? theme.colorScheme.error
                                  : null,
                            ),
                            maxLines: 1,
                            softWrap: false,
                          ),
                        ),
                      ),
                    ),
                  ],
                ),
                Text(
                  [
                    switch (journey.transfers) {
                      0 => 'No transfers',
                      1 => '1 transfer',
                      final n => '$n transfers',
                    },
                    if (street > 0)
                      '${formatDistance(street, units: units)} '
                          '${streetDistanceWord(journey)}',
                  ].join(' · '),
                  style: muted,
                ),
                const SizedBox(height: 8),
                JourneyTimeline(
                  journey: journey,
                  chain: chain,
                  // With no connection, the plan's.
                  axis: arrival == null
                      ? TimelineAxis(journey.departure, journey.arrival)
                      : TimelineAxis(chain.departure, arrival),
                  height: 22,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// How a journey's distance off vehicles is covered, after its distance.
String streetDistanceWord(Journey journey) =>
    switch (journey.legs.whereType<StreetLeg>().firstOrNull?.mode) {
      'cycling' => 'by bike',
      'wheelchair' => 'by wheelchair',
      _ => 'on foot',
    };

/// What's drawn on the rail left of a row, above and below its marker.
enum _Line { none, dotted, thin, bar }

enum _Marker { none, end, stop, pass, vehicle, seat }

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
  });

  /// A row with no marker, one kind of line through it.
  const _Rail.through(_Line line, {Color? color})
    : above = line,
      below = line,
      marker = _Marker.none,
      aboveColor = color,
      belowColor = color,
      icon = null;

  final _Line above;
  final _Line below;
  final _Marker marker;
  final Color? aboveColor;
  final Color? belowColor;
  final IconData? icon;

  @override
  bool operator ==(Object other) =>
      other is _Rail &&
      other.above == above &&
      other.below == below &&
      other.marker == marker &&
      other.aboveColor == aboveColor &&
      other.belowColor == belowColor &&
      other.icon == icon;

  @override
  int get hashCode =>
      Object.hash(above, below, marker, aboveColor, belowColor, icon);
}

const _railWidth = 40.0;
const _railX = 20.0;

/// Where the marker sits from a row's top: level with its first line of text.
const _markerY = 13.0;

/// How far a solid line runs past a row's edge, so rows join without a seam.
const _overlap = 1.0;

class _RailPainter extends CustomPainter {
  _RailPainter(this.rail, this.scheme);

  final _Rail rail;
  final ColorScheme scheme;

  @override
  void paint(Canvas canvas, Size size) {
    final markerY = rail.marker == _Marker.none ? size.height / 2 : _markerY;
    // Solid lines run a pixel past the row's edges into the next row's, whose line
    // meets this one: rows can end between pixels, where two half-covered edges
    // would show as a seam across the bar.
    _line(canvas, rail.above, rail.aboveColor, 0, markerY, size);
    _line(canvas, rail.below, rail.belowColor, markerY, size.height, size);
    final center = Offset(_railX, markerY);
    switch (rail.marker) {
      case _Marker.none:
        break;
      case _Marker.end:
        canvas
          ..drawCircle(center, 7, Paint()..color = scheme.surface)
          ..drawCircle(
            center,
            6,
            Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = 3
              ..color = scheme.onSurface,
          );
      case _Marker.stop:
        final color = rail.aboveColor ?? rail.belowColor ?? scheme.primary;
        canvas
          ..drawCircle(center, 6.5, Paint()..color = color)
          ..drawCircle(center, 3.5, Paint()..color = Colors.white);
      case _Marker.pass:
        canvas.drawCircle(center, 2, Paint()..color = Colors.white);
      case _Marker.vehicle || _Marker.seat:
        final color = rail.belowColor ?? scheme.primary;
        canvas.drawCircle(center, 12, Paint()..color = color);
        final icon = rail.icon;
        if (icon != null) {
          final glyph = TextPainter(
            text: TextSpan(
              text: String.fromCharCode(icon.codePoint),
              style: TextStyle(
                fontFamily: icon.fontFamily,
                package: icon.fontPackage,
                fontSize: 16,
                height: 1,
                color: color.computeLuminance() > 0.4
                    ? Colors.black
                    : Colors.white,
              ),
            ),
            textDirection: TextDirection.ltr,
          )..layout();
          glyph.paint(
            canvas,
            center - Offset(glyph.width / 2, glyph.height / 2),
          );
        }
    }
  }

  void _line(
    Canvas canvas,
    _Line line,
    Color? color,
    double from,
    double to,
    Size size,
  ) {
    if (to <= from) return;
    // Where a solid line reaches the row's edge, it runs on past it.
    final top = from <= 0 ? -_overlap : from;
    final bottom = to >= size.height ? size.height + _overlap : to;
    switch (line) {
      case _Line.none:
        break;
      case _Line.dotted:
        final paint = Paint()..color = scheme.onSurfaceVariant;
        // Dots at a fixed pitch from the row's top, so rows join without a seam.
        const pitch = 7.0;
        for (var y = (from / pitch).ceil() * pitch + 3; y < to; y += pitch) {
          canvas.drawCircle(Offset(_railX, y), 2, paint);
        }
      case _Line.thin:
        canvas.drawLine(
          Offset(_railX, top),
          Offset(_railX, bottom),
          Paint()
            ..color = scheme.outlineVariant
            ..strokeWidth = 2,
        );
      case _Line.bar:
        canvas.drawRect(
          Rect.fromLTRB(_railX - 3.5, top, _railX + 3.5, bottom),
          Paint()..color = color ?? scheme.primary,
        );
    }
  }

  @override
  bool shouldRepaint(_RailPainter old) =>
      old.rail != rail || old.scheme != scheme;
}

/// A row of the itinerary: its rail, and what's said beside it.
class _RailRow extends StatelessWidget {
  const _RailRow({
    super.key,
    required this.rail,
    required this.child,
    this.onTap,
  });

  final _Rail rail;
  final Widget child;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final row = CustomPaint(
      painter: _RailPainter(rail, Theme.of(context).colorScheme),
      child: Padding(
        padding: const EdgeInsets.only(left: _railWidth + 4, right: 16),
        child: child,
      ),
    );
    return onTap == null ? row : InkWell(onTap: onTap, child: row);
  }
}

/// A route's name on its color, with its vehicle's icon.
class RouteBadge extends StatelessWidget {
  const RouteBadge({super.key, required this.route});

  final TransitRoute? route;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = parseRouteColor(route?.color) ?? theme.colorScheme.primary;
    final text = routeTextColor(route, color);
    final name = route?.name ?? '';
    return Container(
      height: 22,
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
            Text(
              name,
              style: theme.textTheme.labelLarge?.copyWith(
                color: text,
                fontWeight: FontWeight.w600,
                height: 1,
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
          ),
          child: _StopText(
            visit: visit,
            time: i == last ? visit.arrival : visit.departure,
            live: visit.predicted,
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

  /// The leg of the departure ride [ride] (by its index in [ConnectionChain.rides])
  /// takes, when its stops are known.
  TransitLeg? _taken(int ride) =>
      widget.chain.rides[ride].legTaken(widget.fetched.elementAtOrNull(ride));

  @override
  void didUpdateWidget(JourneyItinerary oldWidget) {
    super.didUpdateWidget(oldWidget);
    final chain = widget.chain;
    if (!identical(oldWidget.chain.journey, chain.journey)) _expanded.clear();
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
      padding: const EdgeInsets.only(bottom: 16),
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
        rail: _Rail(marker: _Marker.end, below: connector(legs.firstOrNull)),
        child: _EndText(
          name: widget.start,
          time: 'Leave ${clock(context, chain.departure, utcOffset: offset)}',
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
              start: widget.start,
              destination: widget.destination,
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
          final wait = choice.made ? choice.waitSeconds : 0;
          // Whether there's a wait doesn't hang on the departure chosen of this ride,
          // so choosing one never adds or drops a row above its departures.
          final waits = change || _mayWait(choice);
          if (waits) {
            rows.add(
              _RailRow(
                key: ValueKey(('wait', i)),
                rail: const _Rail.through(_Line.thin),
                child: _WaitText(
                  change: change ? leg.boarding.stop.name : null,
                  seconds: wait,
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
        rail: _Rail(marker: _Marker.end, above: connector(legs.lastOrNull)),
        child: _EndText(
          name: widget.destination,
          time: arrival == null
              ? 'No connection'
              : 'Arrive ${clock(context, arrival, utcOffset: offset)}',
          prefix: 'Arrive ',
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

    rows.add(
      _RailRow(
        key: ValueKey(('title', index)),
        rail: _Rail(
          above: above,
          below: _Line.bar,
          marker: _Marker.vehicle,
          belowColor: first,
          icon: vehicleIcon(route?.kind ?? VehicleKind.other),
        ),
        child: _RideTitle(
          ride: ride,
          route: route,
          headsign: choice.headsign,
          seconds: leaves != null && arrives != null
              ? arrives.difference(leaves).inSeconds
              : null,
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
        ),
        child: _StopText(
          visit: ride.boarding,
          time: leaves,
          live: live.$1,
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
                ),
                child: _StopText(
                  visit: visit,
                  time: visit.departure,
                  live: visit.predicted,
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
        ),
        child: _StopText(
          visit: ride.alighting,
          time: arrives,
          live: live.$2,
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

/// Whether the rider may wait for [choice]'s ride, whichever of its departures they
/// take: as the plan has it, or, from when they're ready, for any they can make.
bool _mayWait(RideChoice choice) {
  final ready = choice.ready;
  final departures = choice.ride.departures;
  if (ready == null || departures.isEmpty) return choice.waitSeconds >= 60;
  return [
    for (final (i, d) in departures.indexed)
      if (choice.catchable[i]) d.departure.difference(ready).inSeconds >= 60,
  ].any((w) => w);
}

/// [text], a clock time at the end of a row, after [prefix], in a space as wide as
/// any clock time there, so what's beside it lays out the same whatever the time; a
/// change cross-fades.
class _TimeSlot extends StatelessWidget {
  const _TimeSlot(this.text, {this.prefix = '', this.style});

  final String text;
  final String prefix;
  final TextStyle? style;

  static const _figures = [FontFeature.tabularFigures()];

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
    return Stack(
      alignment: AlignmentDirectional.centerEnd,
      children: [
        for (final hour in const [10, 22])
          ExcludeSemantics(
            child: Visibility(
              visible: false,
              maintainSize: true,
              maintainAnimation: true,
              maintainState: true,
              child: Text('$prefix${widest(hour)}', style: style),
            ),
          ),
        FadeSwap(
          value: text,
          alignment: AlignmentDirectional.centerEnd,
          child: Text(text, style: style),
        ),
      ],
    );
  }
}

class _EndText extends StatelessWidget {
  const _EndText({
    required this.name,
    required this.time,
    required this.prefix,
  });

  final String name;
  final String time;

  /// What [time] says before its clock time.
  final String prefix;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Text(
              name,
              style: theme.textTheme.titleSmall,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          const SizedBox(width: 8),
          _TimeSlot(time, prefix: prefix, style: theme.textTheme.bodyMedium),
        ],
      ),
    );
  }
}

/// The change or wait before a ride, on one line: its height is the same whatever the
/// wait, so the rows below it stay where they are.
class _WaitText extends StatelessWidget {
  const _WaitText({required this.change, required this.seconds});

  /// The stop where the rider changes vehicles without walking, if they do.
  final String? change;
  final int seconds;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodyMedium?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final wait = seconds >= 60 ? formatDuration(seconds.toDouble()) : null;
    final change = this.change;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        children: [
          Icon(
            change != null ? Icons.sync_alt : Icons.hourglass_empty,
            size: 16,
            color: theme.colorScheme.onSurfaceVariant,
          ),
          const SizedBox(width: 6),
          if (change != null)
            Flexible(
              child: Text(
                'Change at $change',
                style: muted,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
          Flexible(
            flex: 0,
            child: FadeSwap(
              value: wait,
              child: Text(
                switch ((change, wait)) {
                  (null, null) => 'No wait',
                  (null, final wait?) => 'Wait $wait',
                  (_, null) => '',
                  (_, final wait?) => ', wait $wait',
                },
                style: muted,
                maxLines: 1,
                softWrap: false,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A walk or ride on the streets, its steps a tap away, and any moved stop at its end.
class _StreetRow extends StatefulWidget {
  const _StreetRow({
    super.key,
    required this.leg,
    required this.units,
    required this.start,
    required this.destination,
  });

  final StreetLeg leg;

  /// Metric or imperial, for the distances on foot.
  final UnitSystem units;
  final String start;
  final String destination;

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
    final leg = widget.leg;
    final walk = leg.walk;
    final steps = walk?.maneuvers ?? const [];
    final to = leg.to?.name ?? widget.destination;
    return _RailRow(
      rail: const _Rail.through(_Line.dotted),
      onTap: steps.isEmpty ? null : () => setState(() => _open = !_open),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // A moved stop is said first, where the rider reads before setting out.
            for (final notice in leg.stopMoved) ...[
              StopMovedBanner(notice: notice),
              const SizedBox(height: 6),
            ],
            Row(
              children: [
                Icon(
                  streetIcon(leg.mode),
                  size: 18,
                  color: scheme.onSurfaceVariant,
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(
                          text:
                              '${streetVerb(leg.mode)} '
                              '${formatDuration(leg.duration.toDouble())}',
                          style: theme.textTheme.bodyMedium,
                        ),
                        TextSpan(
                          text: [
                            if (walk != null)
                              ' · ${formatDistance(walk.distance, units: widget.units)}',
                            ' to $to',
                          ].join(),
                        ),
                      ],
                    ),
                    style: muted,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                if (steps.isNotEmpty)
                  Icon(
                    _open ? Icons.expand_less : Icons.expand_more,
                    size: 20,
                    color: scheme.onSurfaceVariant,
                  ),
              ],
            ),
            if (leg.mode == 'wheelchair' &&
                leg.accessibility != Accessibility.unknown)
              AccessibilityLine(accessibility: leg.accessibility),
            if (_open)
              for (final m in steps)
                Padding(
                  padding: const EdgeInsets.only(top: 4, left: 24),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(
                        child: Text(
                          m.instruction,
                          style: theme.textTheme.bodySmall,
                        ),
                      ),
                      if (m.distance > 0)
                        Text(
                          formatDistance(m.distance, units: widget.units),
                          style: muted,
                        ),
                    ],
                  ),
                ),
          ],
        ),
      ),
    );
  }
}

class _RideTitle extends StatelessWidget {
  const _RideTitle({
    required this.ride,
    required this.route,
    required this.headsign,
    required this.seconds,
  });

  final TransitLeg ride;

  /// The route and headsign of the departure taken.
  final TransitRoute? route;
  final String? headsign;

  /// How long it rides, when that's known.
  final int? seconds;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final route = this.route;
    final seconds = this.seconds;
    return Padding(
      padding: const EdgeInsets.only(top: 2, bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // One line each, so a choice of departure above the ride's departures
          // never changes their height.
          AnimatedSize(
            duration: rowDuration,
            curve: rowCurve,
            alignment: AlignmentDirectional.centerStart,
            child: FadeSwap(
              value: route?.id,
              child: RouteBadge(route: route),
            ),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                FadeSwap(
                  value: headsign ?? route?.longName,
                  child: Text(
                    headsign ?? route?.longName ?? '',
                    style: theme.textTheme.titleSmall,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                FadeSwap(
                  value: route?.kind,
                  child: Text(
                    [
                      ?ride.agency,
                      (route?.kind ?? VehicleKind.other).label,
                    ].join(' · '),
                    style: muted,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ],
            ),
          ),
          if (seconds != null) ...[
            const SizedBox(width: 8),
            FadeSwap(
              value: seconds ~/ 60,
              alignment: AlignmentDirectional.centerEnd,
              child: Text(
                formatDuration(seconds.toDouble()),
                style: theme.textTheme.bodyMedium?.copyWith(
                  color: theme.colorScheme.primary,
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

class _StopText extends StatelessWidget {
  const _StopText({
    required this.visit,
    required this.time,
    required this.live,
    required this.skipped,
    this.strong = false,
  });

  final StopVisit visit;

  /// When it's left or reached, or null when that isn't known.
  final DateTime? time;
  final bool live;

  /// Whether the trip skips it.
  final bool skipped;

  /// Whether the rider gets on or off here, rather than passing it.
  final bool strong;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final style = strong
        ? theme.textTheme.bodyLarge
        : theme.textTheme.bodySmall;
    final stop = visit.stop;
    final name = [
      stop.name,
      if (stop.platform != null) '(platform ${stop.platform})',
    ].join(' ');
    return Padding(
      padding: EdgeInsets.symmetric(vertical: strong ? 2 : 1),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Text(
              name,
              style: style?.copyWith(
                decoration: skipped ? TextDecoration.lineThrough : null,
              ),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          if (stop.accessibility == Accessibility.inaccessible)
            Padding(
              padding: const EdgeInsets.only(left: 4, top: 2),
              child: Icon(
                Icons.not_accessible,
                size: 16,
                color: theme.colorScheme.error,
              ),
            ),
          if (time case final time?) ...[
            const SizedBox(width: 8),
            _TimeSlot(
              clock(context, time, utcOffset: visit.utcOffset),
              style: style?.copyWith(color: live ? liveColor(theme) : null),
            ),
          ],
        ],
      ),
    );
  }
}

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
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        children: [
          Text(
            count == 1 ? '1 stop before' : '$count stops before',
            style: muted,
          ),
          Icon(
            open ? Icons.expand_less : Icons.expand_more,
            size: 18,
            color: theme.colorScheme.onSurfaceVariant,
          ),
          const Spacer(),
          Text(minutes, style: muted),
        ],
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
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Stay on board', style: theme.textTheme.titleSmall),
          const SizedBox(height: 2),
          Wrap(
            spacing: 6,
            runSpacing: 2,
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
    final runs = {for (final d in widget.choice.ride.departures) _run(d)};
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
    final theme = Theme.of(context);
    final choice = widget.choice;
    final ride = choice.ride;
    final departures = ride.departures;
    // Each tile badges its route when they aren't all the ride's.
    final mixed = departures.any((d) => d.route != null);
    return Padding(
      padding: const EdgeInsets.only(top: 6, bottom: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (departures.length > 1) ...[
            Text(
              'Departures',
              style: theme.textTheme.labelMedium?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
            const SizedBox(height: 4),
            LayoutBuilder(
              builder: (context, constraints) => ValueListenableBuilder(
                valueListenable: _now,
                builder: (context, now, _) => SingleChildScrollView(
                  controller: _tiles,
                  scrollDirection: Axis.horizontal,
                  child: _CenterAtFirst(
                    controller: _tiles,
                    tile: choice.departure == null
                        ? null
                        : _keyOf(choice.departure!),
                    viewport: constraints.maxWidth,
                    child: IntrinsicHeight(
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        spacing: 6,
                        children: [
                          for (final (i, d) in departures.indexed)
                            ConnectionTile(
                              key: _keyOf(d),
                              departure: d,
                              route: mixed ? d.route ?? ride.route : null,
                              countdown: Countdown(d, now: now),
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
              ),
            ),
            const SizedBox(height: 6),
          ],
          FadeSwap(
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
/// departures are of several routes; its countdown, with the live mark when its time is
/// real-time, and the clock time it leaves at; and whether it's the suggested one or the
/// day's last. One that doesn't run is struck through with a label, and one the rider
/// can't make is dimmed with its clock time, and neither can be chosen.
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

  static const _minWidth = 64.0;
  static const _radius = 10.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final d = departure;
    final struck = !d.status.runs;
    final usable = !struck && catchable;
    final faded = scheme.onSurface.withValues(alpha: 0.38);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: scheme.onSurfaceVariant,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final time = clock(context, d.departure, utcOffset: utcOffset);
    // One the rider can't make, or past the hour, or once it has left, shows the clock
    // time in place of a countdown.
    final counts =
        !struck &&
        catchable &&
        !countdown.departed &&
        countdown.minutesUntil <= countdownMinutes;
    final (label, labelColor) = switch (d) {
      RideDeparture(status: DepartureStatus.canceled) => (
        'Canceled',
        scheme.onSurfaceVariant,
      ),
      RideDeparture(status: DepartureStatus.skipped) => (
        'Skips stop',
        scheme.onSurfaceVariant,
      ),
      _ when !catchable => ('Missed', faded),
      RideDeparture(suggested: true, lastOfDay: true) => (
        'Suggested · Last',
        scheme.primary,
      ),
      RideDeparture(suggested: true) => ('Suggested', scheme.primary),
      RideDeparture(lastOfDay: true) => ('Last', scheme.error),
      _ => (null, null),
    };
    final route = this.route;
    return Semantics(
      button: usable,
      selected: selected,
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
            onTap: usable ? onTap : null,
            child: ConstrainedBox(
              constraints: const BoxConstraints(minWidth: _minWidth),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  spacing: 2,
                  children: [
                    if (route != null)
                      AnimatedOpacity(
                        duration: rowDuration,
                        curve: rowCurve,
                        opacity: usable ? 1 : 0.45,
                        child: RouteBadge(route: route),
                      ),
                    FadeSwap(
                      value: counts ? null : (time, usable, struck),
                      child: counts
                          ? CountdownNumber(
                              countdown,
                              key: const ValueKey('countdown'),
                              alignment: AlignmentDirectional.centerStart,
                            )
                          : Text(
                              time,
                              key: const ValueKey('countdown'),
                              style: theme.textTheme.titleMedium?.copyWith(
                                fontWeight: FontWeight.w600,
                                color: usable ? null : faded,
                                decoration: struck
                                    ? TextDecoration.lineThrough
                                    : null,
                              ),
                            ),
                    ),
                    if (counts)
                      Text(
                        time,
                        key: const ValueKey('clock'),
                        style: muted?.copyWith(
                          decoration: struck
                              ? TextDecoration.lineThrough
                              : null,
                        ),
                      ),
                    if (label case final label?)
                      FadeSwap(
                        value: (label, labelColor),
                        child: Text(
                          label,
                          key: const ValueKey('label'),
                          style: theme.textTheme.labelSmall?.copyWith(
                            color: labelColor,
                            fontWeight: d.suggested && usable
                                ? FontWeight.w600
                                : null,
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
/// be at its stop, with the departures chosen before it.
class NoConnection extends StatelessWidget {
  const NoConnection({super.key, required this.choice});

  final RideChoice choice;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final ready = choice.ready;
    final stop = choice.ride.boarding;
    return Container(
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
      decoration: BoxDecoration(
        color: scheme.errorContainer,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.link_off, size: 20, color: scheme.onErrorContainer),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'No connection',
                  style: theme.textTheme.labelLarge?.copyWith(
                    color: scheme.onErrorContainer,
                  ),
                ),
                Text(
                  ready == null
                      ? "You can't make any of these departures."
                      : "You'd be at the stop at "
                            '${clock(context, ready, utcOffset: stop.utcOffset)}'
                            ', after all of these leave. An earlier departure '
                            'on a ride before may connect.',
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

/// How one departure of a ride is running, in one place: live or scheduled, its
/// accessibility, its vehicle, whether it's the night's last, and its alerts. When and
/// where it goes are the ride's rows'.
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
    final scheme = theme.colorScheme;
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: scheme.onSurfaceVariant,
    );
    final d = departure;
    final own = d == null || d.suggested;
    final status = d == null ? rideStatus(theme, ride) : _status(theme, d);
    final accessibility = d?.accessibility ?? ride.accessibility;
    final bikes = d?.bikesAllowed ?? ride.bikesAllowed;
    final vehicle = own ? ride.vehicle : null;
    final vehicleText = [
      if (own && vehicle?.label != null) 'Vehicle ${vehicle!.label}',
      if (!own && d.vehicleId != null) 'Vehicle ${d.vehicleId}',
      if (vehicle?.occupancy case final occupancy?) occupancyText(occupancy),
    ].join(' · ');
    final alerts = (own ? ride.alerts : d.alerts)
        .where((a) => a.effect != 'stop_moved')
        .toList();
    return Container(
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
      decoration: BoxDecoration(
        color: scheme.surfaceContainer,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        spacing: 3,
        children: [
          IconLine(
            icon: status.live ? Icons.sensors : Icons.schedule,
            text: status.text,
            color: status.color,
            style: theme.textTheme.bodySmall?.copyWith(color: status.color),
          ),
          if (d?.lastOfDay ?? false)
            IconLine(
              icon: Icons.bedtime_outlined,
              text: 'The last departure of the day between these stops',
              color: scheme.error,
              style: theme.textTheme.bodySmall?.copyWith(color: scheme.error),
              maxLines: 2,
            ),
          AccessibilityLine(accessibility: accessibility),
          if (access == TransitAccess.cycling && bikes != null)
            IconLine(
              icon: Icons.pedal_bike,
              text: bikes == 'yes' ? 'Bikes allowed' : 'No bikes',
              style: muted,
            ),
          if (vehicleText.isNotEmpty)
            IconLine(
              icon: Icons.directions_bus_filled_outlined,
              text: vehicleText,
              style: muted,
            ),
          for (final alert in alerts) _AlertLine(alert: alert),
        ],
      ),
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
          return (text: 'Live: on time', color: liveColor(theme), live: true);
        }
        return delay > 0
            ? (
                text: 'Live: $minutes min late',
                color: theme.colorScheme.error,
                live: true,
              )
            : (
                text: 'Live: $minutes min early',
                color: liveColor(theme),
                live: true,
              );
    }
  }
}

/// An alert's header, its description a tap away.
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
    return InkWell(
      onTap: description == null ? null : () => setState(() => _open = !_open),
      child: Padding(
        padding: const EdgeInsets.only(top: 1),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.only(top: 1, right: 4),
              child: Icon(Icons.warning_amber, size: 14, color: scheme.error),
            ),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    header,
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: scheme.onSurface,
                      fontWeight: FontWeight.w500,
                    ),
                    maxLines: _open ? null : 2,
                    overflow: _open ? null : TextOverflow.ellipsis,
                  ),
                  if (_open && description != null)
                    Padding(
                      padding: const EdgeInsets.only(top: 2),
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
            if (description != null)
              Icon(
                _open ? Icons.expand_less : Icons.expand_more,
                size: 16,
                color: scheme.onSurfaceVariant,
              ),
          ],
        ),
      ),
    );
  }
}
