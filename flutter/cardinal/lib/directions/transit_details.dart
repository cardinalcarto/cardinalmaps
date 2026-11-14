/// One transit journey in detail, top to bottom: each ride on a bar in its route's
/// color with its stops and times, the walks and waits between rides, and for each ride
/// the other departures of its route, so a rider can see what happens if they miss it.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import 'format.dart';
import 'transit_plan.dart';
import 'transit_sheet.dart';
import 'transit_timeline.dart';

/// The top of a journey's details: back to the overview, when it leaves and arrives,
/// and its timeline.
class JourneyHeader extends StatelessWidget {
  const JourneyHeader({
    super.key,
    required this.journey,
    required this.now,
    required this.miles,
    required this.onBack,
  });

  final Journey journey;
  final DateTime now;
  final bool miles;
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final offset = journeyUtcOffset(journey);
    final street = journey.streetMeters;
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
                child: Text('Trip details', style: theme.textTheme.titleLarge),
              ),
              Text(
                formatDuration(journey.duration.toDouble()),
                style: theme.textTheme.titleLarge?.copyWith(
                  color: theme.colorScheme.primary,
                ),
              ),
            ],
          ),
          Padding(
            padding: const EdgeInsets.only(left: 12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  crossAxisAlignment: CrossAxisAlignment.baseline,
                  textBaseline: TextBaseline.alphabetic,
                  children: [
                    GoIn(journey: journey, now: now),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(
                        '${clock(context, journey.departure, utcOffset: offset)}'
                        ' – '
                        '${clock(context, journey.arrival, utcOffset: offset)}',
                        style: theme.textTheme.bodyMedium,
                        textAlign: TextAlign.end,
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
                      '${formatDistance(street, miles: miles)} '
                          '${streetDistanceWord(journey)}',
                  ].join(' · '),
                  style: muted,
                ),
                const SizedBox(height: 8),
                JourneyTimeline(
                  journey: journey,
                  axis: TimelineAxis(journey.departure, journey.arrival),
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

class _RailPainter extends CustomPainter {
  _RailPainter(this.rail, this.scheme);

  final _Rail rail;
  final ColorScheme scheme;

  @override
  void paint(Canvas canvas, Size size) {
    final markerY = rail.marker == _Marker.none ? size.height / 2 : _markerY;
    _line(canvas, rail.above, rail.aboveColor, 0, markerY);
    _line(canvas, rail.below, rail.belowColor, markerY, size.height);
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

  void _line(Canvas canvas, _Line line, Color? color, double from, double to) {
    if (to <= from) return;
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
          Offset(_railX, from),
          Offset(_railX, to),
          Paint()
            ..color = scheme.outlineVariant
            ..strokeWidth = 2,
        );
      case _Line.bar:
        canvas.drawRect(
          Rect.fromLTRB(_railX - 3.5, from, _railX + 3.5, to),
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

/// A journey's legs from top to bottom, as slivers' worth of rows.
class JourneyItinerary extends StatefulWidget {
  const JourneyItinerary({
    super.key,
    required this.journey,
    required this.start,
    required this.destination,
    required this.miles,
    required this.access,
    required this.now,
  });

  final Journey journey;

  /// What the trip's ends are called.
  final String start;
  final String destination;
  final bool miles;
  final TransitAccess access;
  final DateTime now;

  @override
  State<JourneyItinerary> createState() => _JourneyItineraryState();
}

class _JourneyItineraryState extends State<JourneyItinerary> {
  /// The rides' segments whose stops between are showing, as (leg, segment).
  final _expanded = <(int, int)>{};

  @override
  void didUpdateWidget(JourneyItinerary oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.journey, widget.journey)) _expanded.clear();
  }

  @override
  Widget build(BuildContext context) {
    final rows = _rows(context);
    return SliverPadding(
      padding: const EdgeInsets.only(bottom: 16),
      sliver: SliverList.builder(
        itemCount: rows.length,
        itemBuilder: (context, i) => rows[i],
      ),
    );
  }

  List<Widget> _rows(BuildContext context) {
    final journey = widget.journey;
    final legs = journey.legs;
    final offset = journeyUtcOffset(journey);
    final rows = <Widget>[];

    _Line connector(JourneyLeg? leg) => switch (leg) {
      null => _Line.none,
      StreetLeg() when _isChange(leg) => _Line.thin,
      StreetLeg() => _Line.dotted,
      TransitLeg() => _Line.thin,
    };

    rows.add(
      _RailRow(
        rail: _Rail(marker: _Marker.end, below: connector(legs.firstOrNull)),
        child: _EndText(
          name: widget.start,
          time: 'Leave ${clock(context, journey.departure, utcOffset: offset)}',
        ),
      ),
    );

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
              miles: widget.miles,
              start: widget.start,
              destination: widget.destination,
            ),
          );
        case TransitLeg():
          final change =
              previous is TransitLeg ||
              (previous is StreetLeg && _isChange(previous));
          final before = previous == null
              ? _Line.thin
              : connector(previous) == _Line.dotted
              ? _Line.dotted
              : _Line.thin;
          if (change || leg.waitSeconds >= 60) {
            rows.add(
              _RailRow(
                rail: const _Rail.through(_Line.thin),
                child: _WaitText(
                  change: change ? leg.boarding.stop.name : null,
                  seconds: leg.waitSeconds,
                ),
              ),
            );
          }
          rows.addAll(
            _rideRows(
              context,
              i,
              leg,
              above: change || leg.waitSeconds >= 60 ? _Line.thin : before,
              below: connector(next),
            ),
          );
      }
    }

    rows.add(
      _RailRow(
        rail: _Rail(marker: _Marker.end, above: connector(legs.lastOrNull)),
        child: _EndText(
          name: widget.destination,
          time: 'Arrive ${clock(context, journey.arrival, utcOffset: offset)}',
        ),
      ),
    );
    return rows;
  }

  /// A ride's rows: its route, its stops with their times, its departures, and any
  /// trips it goes on as.
  List<Widget> _rideRows(
    BuildContext context,
    int index,
    TransitLeg ride, {
    required _Line above,
    required _Line below,
  }) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final rows = <Widget>[];
    final segments = ride.segments.length >= 2
        ? ride.segments
        : <RideSegment?>[null];
    Color colorOf(RideSegment? segment) =>
        parseRouteColor(
          (segment == null ? ride.route : segmentRoute(ride, segment))?.color,
        ) ??
        scheme.primary;
    final first = colorOf(segments.first);
    final last = colorOf(segments.last);

    rows.add(
      _RailRow(
        rail: _Rail(
          above: above,
          below: _Line.bar,
          marker: _Marker.vehicle,
          belowColor: first,
          icon: vehicleIcon(ride.route?.kind ?? VehicleKind.other),
        ),
        child: _RideTitle(ride: ride),
      ),
    );
    rows.add(
      _RailRow(
        rail: _Rail(
          above: _Line.bar,
          below: _Line.bar,
          marker: _Marker.stop,
          aboveColor: first,
          belowColor: first,
        ),
        child: _StopText(
          visit: ride.boarding,
          time: ride.boarding.departure,
          live: ride.boarding.predicted,
          strong: true,
        ),
      ),
    );
    rows.add(
      _RailRow(
        rail: _Rail.through(_Line.bar, color: first),
        child: DepartureRange(
          key: ValueKey(('departures', index)),
          ride: ride,
          access: widget.access,
          now: widget.now,
        ),
      ),
    );
    final lastStop = ride.stops.length - 1;
    for (final (s, segment) in segments.indexed) {
      final color = colorOf(segment);
      final from = segment?.firstStop.clamp(0, lastStop) ?? 0;
      final to = segment?.lastStop.clamp(0, lastStop) ?? lastStop;
      final between = ride.stops.sublist(
        (from + 1).clamp(0, lastStop),
        to.clamp(from + 1, lastStop + 1),
      );
      final key = (index, s);
      final open = _expanded.contains(key);
      if (between.isNotEmpty) {
        final minutes = ride.stops[to].arrival.difference(
          ride.stops[from].departure,
        );
        rows.add(
          _RailRow(
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
          for (final visit in between) {
            rows.add(
              _RailRow(
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
            rail: _Rail(
              above: _Line.bar,
              below: _Line.bar,
              marker: _Marker.seat,
              aboveColor: color,
              belowColor: nextColor,
              icon: Icons.airline_seat_recline_normal,
            ),
            child: _ContinueText(
              ride: ride,
              segment: nextSegment,
              at: ride.stops[nextSegment.firstStop.clamp(0, lastStop)],
            ),
          ),
        );
      }
    }
    rows.add(
      _RailRow(
        rail: _Rail(
          above: _Line.bar,
          below: below,
          marker: _Marker.stop,
          aboveColor: last,
        ),
        child: _StopText(
          visit: ride.alighting,
          time: ride.alighting.arrival,
          live: ride.alighting.predicted,
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

class _EndText extends StatelessWidget {
  const _EndText({required this.name, required this.time});

  final String name;
  final String time;

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
          Text(time, style: theme.textTheme.bodyMedium),
        ],
      ),
    );
  }
}

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
    final wait = seconds >= 60
        ? 'wait ${formatDuration(seconds.toDouble())}'
        : null;
    final text = change != null
        ? ['Change at $change', ?wait].join(', ')
        : 'Wait ${formatDuration(seconds.toDouble())}';
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
          Expanded(child: Text(text, style: muted)),
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
    required this.miles,
    required this.start,
    required this.destination,
  });

  final StreetLeg leg;
  final bool miles;
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
                              ' · ${formatDistance(walk.distance, miles: widget.miles)}',
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
                          formatDistance(m.distance, miles: widget.miles),
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
  const _RideTitle({required this.ride});

  final TransitLeg ride;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final headsign = ride.headsign;
    return Padding(
      padding: const EdgeInsets.only(top: 2, bottom: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          RouteBadge(route: ride.route),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  headsign ?? ride.route?.longName ?? '',
                  style: theme.textTheme.titleSmall,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                ),
                Text(
                  [
                    ?ride.agency,
                    (ride.route?.kind ?? VehicleKind.other).label,
                  ].join(' · '),
                  style: muted,
                ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          Text(
            formatDuration(ride.duration.toDouble()),
            style: theme.textTheme.bodyMedium?.copyWith(
              color: theme.colorScheme.primary,
            ),
          ),
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
    this.strong = false,
  });

  final StopVisit visit;
  final DateTime time;
  final bool live;

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
                decoration: visit.skipped ? TextDecoration.lineThrough : null,
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
          const SizedBox(width: 8),
          Text(
            clock(context, time, utcOffset: visit.utcOffset),
            style: style?.copyWith(color: live ? liveColor(theme) : null),
          ),
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

/// A ride's departures, as chips to compare, and one place that says how the chosen
/// one is running: live or scheduled, how accessible, and its alerts.
///
/// The journey's own departure is emphasized and chosen to start with. Departures the
/// rider can't make, and ones that don't run, are muted and can't be chosen.
class DepartureRange extends StatefulWidget {
  const DepartureRange({
    super.key,
    required this.ride,
    required this.access,
    required this.now,
  });

  final TransitLeg ride;
  final TransitAccess access;
  final DateTime now;

  @override
  State<DepartureRange> createState() => _DepartureRangeState();
}

class _DepartureRangeState extends State<DepartureRange> {
  /// The departure described below the chips, by index; null for the ride's own.
  int? _chosen;

  @override
  void didUpdateWidget(DepartureRange oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.ride, widget.ride)) _chosen = null;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final ride = widget.ride;
    final departures = ride.departures;
    final own = departures.indexWhere((d) => d.suggested);
    final chosen = _chosen ?? (own >= 0 ? own : null);
    final departure = chosen == null ? null : departures[chosen];
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
            SizedBox(
              height: 46,
              child: ListView.separated(
                scrollDirection: Axis.horizontal,
                itemCount: departures.length,
                separatorBuilder: (_, _) => const SizedBox(width: 6),
                itemBuilder: (context, i) => DepartureChip(
                  departure: departures[i],
                  route: ride.route,
                  own: own >= 0 ? departures[own] : null,
                  chosen: i == chosen,
                  utcOffset: ride.boarding.utcOffset,
                  onTap: () => setState(() => _chosen = i),
                ),
              ),
            ),
            const SizedBox(height: 6),
          ],
          VehicleStatus(
            ride: ride,
            departure: departure,
            access: widget.access,
          ),
        ],
      ),
    );
  }
}

/// One departure of a ride's route, to compare with the journey's own.
class DepartureChip extends StatelessWidget {
  const DepartureChip({
    super.key,
    required this.departure,
    required this.route,
    required this.own,
    required this.chosen,
    required this.utcOffset,
    required this.onTap,
  });

  final RideDeparture departure;
  final TransitRoute? route;

  /// The journey's own departure, to say how much later this one is.
  final RideDeparture? own;
  final bool chosen;
  final int? utcOffset;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final d = departure;
    final usable = d.status.runs && !d.missed;
    final routeColor = parseRouteColor(route?.color) ?? scheme.primary;
    final Color background, foreground;
    if (d.suggested) {
      background = routeColor;
      foreground = routeTextColor(route, routeColor);
    } else if (usable) {
      background = scheme.surfaceContainerHighest;
      foreground = scheme.onSurface;
    } else {
      background = Colors.transparent;
      foreground = scheme.onSurface.withValues(alpha: 0.45);
    }
    final note = switch (d) {
      RideDeparture(status: DepartureStatus.canceled) => 'Canceled',
      RideDeparture(status: DepartureStatus.skipped) => 'Skips stop',
      RideDeparture(missed: true) => 'Missed',
      RideDeparture(lastOfDay: true) => 'Last',
      RideDeparture(suggested: true) => 'Planned',
      _ when own != null => _later(d.departure.difference(own!.departure)),
      _ => '',
    };
    final border = chosen && !d.suggested
        ? BorderSide(color: scheme.primary, width: 2)
        : usable || d.suggested
        ? BorderSide.none
        : BorderSide(color: scheme.outlineVariant);
    return Semantics(
      button: usable,
      selected: chosen,
      child: Material(
        color: background,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(10),
          side: border,
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: usable ? onTap : null,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    if (d.status == DepartureStatus.live) ...[
                      Icon(
                        Icons.sensors,
                        size: 13,
                        color: d.suggested ? foreground : liveColor(theme),
                      ),
                      const SizedBox(width: 2),
                    ],
                    Text(
                      clock(context, d.departure, utcOffset: utcOffset),
                      style: theme.textTheme.labelLarge?.copyWith(
                        color: foreground,
                        fontWeight: d.suggested ? FontWeight.w700 : null,
                        decoration: d.status.runs
                            ? null
                            : TextDecoration.lineThrough,
                      ),
                    ),
                  ],
                ),
                Text(
                  note,
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: d.lastOfDay && usable && !d.suggested
                        ? scheme.error
                        : foreground,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  static String _later(Duration difference) {
    final minutes = (difference.inSeconds / 60).round();
    if (minutes == 0) return 'Same time';
    return minutes > 0 ? '+$minutes min' : '−${-minutes} min';
  }
}

/// How one departure of a ride is running, in one place: live or scheduled, its
/// accessibility, its vehicle, whether it's the night's last, and its alerts.
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
    final routeName = ride.route?.name ?? '';
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
          if (!own)
            Text(
              [
                'Leaves ${clock(context, d.departure, utcOffset: ride.boarding.utcOffset)}',
                'arrives ${clock(context, d.arrival, utcOffset: ride.alighting.utcOffset)}',
              ].join(', '),
              style: theme.textTheme.labelLarge,
            ),
          if (!own && d.headsign != null)
            Text('To ${d.headsign}', style: muted),
          IconLine(
            icon: status.live ? Icons.sensors : Icons.schedule,
            text: status.text,
            color: status.color,
            style: theme.textTheme.bodySmall?.copyWith(color: status.color),
          ),
          if (d?.lastOfDay ?? false)
            IconLine(
              icon: Icons.bedtime_outlined,
              text: routeName.isEmpty
                  ? 'The last one of the day from this stop'
                  : 'The last $routeName of the day from this stop',
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
