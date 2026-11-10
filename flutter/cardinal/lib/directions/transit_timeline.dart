/// A transit journey drawn along a time axis: its rides as badges in their routes'
/// colors, and the walks and waits between them as short gaps.
///
/// Rows of the overview share one axis, so journeys line up by the time they happen.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import 'format.dart';
import 'transit_plan.dart';
import 'transit_sheet.dart';

/// What a stretch of a journey's timeline is.
enum TimelineKind { street, wait, ride }

/// A stretch of a journey's timeline: a walk or ride on the streets, a wait, or one
/// trip of a ride (a stay-seated ride has one per trip).
@immutable
class TimelinePiece {
  const TimelinePiece({
    required this.kind,
    required this.start,
    required this.end,
    this.mode,
    this.route,
    this.continues = false,
  });

  final TimelineKind kind;
  final DateTime start;
  final DateTime end;

  /// For a street piece: `walking`, `cycling`, or `wheelchair`.
  final String? mode;

  /// For a ride piece: its route.
  final TransitRoute? route;

  /// For a ride piece: whether the vehicle goes on as the next piece, without the rider
  /// getting off.
  final bool continues;

  int get seconds => end.difference(start).inSeconds;
}

/// [journey]'s timeline, from leaving the start to arriving: a wait fills any time
/// between one leg and the next.
List<TimelinePiece> timelinePieces(Journey journey) {
  final pieces = <TimelinePiece>[];
  var clock = journey.departure;
  void add(TimelinePiece piece) {
    if (piece.start.isAfter(clock)) {
      pieces.add(
        TimelinePiece(kind: TimelineKind.wait, start: clock, end: piece.start),
      );
    }
    if (!piece.end.isBefore(piece.start)) {
      pieces.add(piece);
      if (piece.end.isAfter(clock)) clock = piece.end;
    }
  }

  for (final leg in journey.legs) {
    switch (leg) {
      case StreetLeg():
        // A change at the same stop has nothing to draw but the wait.
        if (leg.duration > 0) {
          add(
            TimelinePiece(
              kind: TimelineKind.street,
              start: leg.departure,
              end: leg.arrival,
              mode: leg.mode,
            ),
          );
        }
      case TransitLeg():
        final segments = leg.segments;
        if (segments.length < 2) {
          add(
            TimelinePiece(
              kind: TimelineKind.ride,
              start: leg.departure,
              end: leg.arrival,
              route: leg.route,
            ),
          );
        } else {
          for (final (i, segment) in segments.indexed) {
            final last = leg.stops.length - 1;
            final first = leg.stops[segment.firstStop.clamp(0, last)];
            final end = leg.stops[segment.lastStop.clamp(0, last)];
            add(
              TimelinePiece(
                kind: TimelineKind.ride,
                start: i == 0 ? leg.departure : first.departure,
                end: i == segments.length - 1 ? leg.arrival : end.arrival,
                route: segmentRoute(leg, segment),
                continues: i < segments.length - 1,
              ),
            );
          }
        }
    }
  }
  if (journey.arrival.isAfter(clock)) {
    pieces.add(
      TimelinePiece(
        kind: TimelineKind.wait,
        start: clock,
        end: journey.arrival,
      ),
    );
  }
  return pieces;
}

/// A stay-seated ride's trip's route, in the ride's colors when it has none of its own.
TransitRoute? segmentRoute(TransitLeg ride, RideSegment segment) =>
    segment.route?.withColors(ride.route?.color, ride.route?.textColor) ??
    ride.route;

/// The span of time a timeline shows.
@immutable
class TimelineAxis {
  const TimelineAxis(this.start, this.end);

  /// From the earliest departure to the latest arrival of [journeys].
  factory TimelineAxis.of(Iterable<Journey> journeys) {
    DateTime? start, end;
    for (final j in journeys) {
      if (start == null || j.departure.isBefore(start)) start = j.departure;
      if (end == null || j.arrival.isAfter(end)) end = j.arrival;
    }
    final now = DateTime.fromMillisecondsSinceEpoch(0);
    return TimelineAxis(start ?? now, end ?? now);
  }

  final DateTime start;
  final DateTime end;

  int get seconds => math.max(1, end.difference(start).inSeconds);

  /// Times to mark along the axis, at a round interval that leaves at least
  /// [minSpacing] logical pixels between marks on a timeline [width] wide, and none
  /// closer to either end than [inset], where a mark's label wouldn't fit.
  List<DateTime> ticks(
    double width, {
    double minSpacing = 72,
    double inset = 28,
  }) {
    if (width <= 0) return const [];
    const steps = [5, 10, 15, 20, 30, 60, 120, 180, 360];
    final perMinute = width / (seconds / 60);
    final step =
        steps.firstWhere(
          (m) => m * perMinute >= minSpacing,
          orElse: () => steps.last,
        ) *
        60;
    final local = start.toLocal();
    final midnight = DateTime(local.year, local.month, local.day);
    final from = local.difference(midnight).inSeconds;
    var t = midnight.add(Duration(seconds: (from ~/ step + 1) * step));
    final out = <DateTime>[];
    while (t.isBefore(end)) {
      final at = x(t, width);
      if (at >= inset && at <= width - inset) out.add(t);
      t = t.add(Duration(seconds: step));
    }
    return out;
  }

  double x(DateTime t, double width) =>
      t.difference(start).inMilliseconds / 1000 / seconds * width;

  @override
  bool operator ==(Object other) =>
      other is TimelineAxis && other.start == start && other.end == end;

  @override
  int get hashCode => Object.hash(start, end);
}

/// Where a piece is drawn, in logical pixels from the timeline's left.
@immutable
class PlacedPiece {
  const PlacedPiece(this.piece, this.left, this.right);

  final TimelinePiece piece;
  final double left;
  final double right;

  double get width => right - left;
}

/// The narrowest each kind of piece is drawn, so a short ride's badge stays readable.
const timelineMinWidths = {
  TimelineKind.ride: 34.0,
  TimelineKind.street: 8.0,
  TimelineKind.wait: 2.0,
};

/// Places [pieces] on [axis] across [width]: at their times, but each at least as wide
/// as [timelineMinWidths] says, in order and without overlapping. Widening a piece
/// pushes the next ones right only until their own times catch up, and nothing passes
/// the right edge; if the minimums can't all fit, everything shrinks to fit.
List<PlacedPiece> placePieces(
  List<TimelinePiece> pieces,
  TimelineAxis axis,
  double width,
) {
  if (pieces.isEmpty || width <= 0) return const [];
  final min = [for (final p in pieces) timelineMinWidths[p.kind]!];
  final total = min.fold(0.0, (a, b) => a + b);
  if (total >= width) {
    // Too many pieces for their minimums: share the width by minimum.
    var left = 0.0;
    return [
      for (final (i, p) in pieces.indexed)
        PlacedPiece(p, left, left += min[i] / total * width),
    ];
  }
  final lefts = List<double>.filled(pieces.length, 0);
  final rights = List<double>.filled(pieces.length, 0);
  // Forward: from each piece's time, at least its minimum, after the last.
  var cursor = axis.x(pieces.first.start, width).clamp(0.0, width);
  for (final (i, p) in pieces.indexed) {
    lefts[i] = math.max(cursor, axis.x(p.start, width));
    rights[i] = math.max(lefts[i] + min[i], axis.x(p.end, width));
    cursor = rights[i];
  }
  // Backward: nothing past the right edge, keeping minimums, pushing earlier ones left.
  var limit = width;
  for (var i = pieces.length - 1; i >= 0; i--) {
    rights[i] = math.min(rights[i], limit);
    lefts[i] = math.min(lefts[i], rights[i] - min[i]);
    limit = lefts[i];
  }
  return [
    for (final (i, p) in pieces.indexed) PlacedPiece(p, lefts[i], rights[i]),
  ];
}

/// The text color to draw on a route's color: its own when the feed gives one.
Color routeTextColor(TransitRoute? route, Color background) =>
    parseRouteColor(route?.textColor) ??
    (background.computeLuminance() > 0.4 ? Colors.black : Colors.white);

/// A journey's timeline: ride badges in route colors, dotted walks, and thin waits.
///
/// Its pieces sit at their times on [axis], which rows of a list share, with the axis'
/// marks ([TimelineAxis.ticks]) drawn faintly behind it if [ticks] is set.
class JourneyTimeline extends StatelessWidget {
  const JourneyTimeline({
    super.key,
    required this.journey,
    required this.axis,
    this.ticks = false,
    this.height = 26,
  });

  final Journey journey;
  final TimelineAxis axis;
  final bool ticks;
  final double height;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final pieces = timelinePieces(journey);
    return Semantics(
      label: timelineLabel(pieces),
      child: SizedBox(
        height: height,
        width: double.infinity,
        child: CustomPaint(
          painter: _TimelinePainter(
            pieces: pieces,
            axis: axis,
            ticks: ticks,
            scheme: theme.colorScheme,
            textStyle: theme.textTheme.labelLarge!.copyWith(
              fontWeight: FontWeight.w600,
              height: 1,
            ),
            textScaler: MediaQuery.textScalerOf(context),
          ),
        ),
      ),
    );
  }
}

/// What a timeline says, for screen readers: its rides in order.
String timelineLabel(List<TimelinePiece> pieces) {
  final rides = [
    for (final p in pieces)
      if (p.kind == TimelineKind.ride) _badgeText(p.route),
  ];
  return rides.isEmpty ? 'No rides' : rides.join(', then ');
}

String _badgeText(TransitRoute? route) {
  final name = route?.shortName ?? route?.longName;
  return name == null || name.isEmpty
      ? (route?.kind ?? VehicleKind.other).label
      : name;
}

class _TimelinePainter extends CustomPainter {
  _TimelinePainter({
    required this.pieces,
    required this.axis,
    required this.ticks,
    required this.scheme,
    required this.textStyle,
    required this.textScaler,
  });

  final List<TimelinePiece> pieces;
  final TimelineAxis axis;
  final bool ticks;
  final ColorScheme scheme;
  final TextStyle textStyle;
  final TextScaler textScaler;

  @override
  void paint(Canvas canvas, Size size) {
    final mid = size.height / 2;
    if (ticks) {
      final tick = Paint()
        ..color = scheme.outlineVariant.withValues(alpha: 0.5)
        ..strokeWidth = 1;
      for (final t in axis.ticks(size.width)) {
        final x = axis.x(t, size.width).roundToDouble() + 0.5;
        canvas.drawLine(Offset(x, -12), Offset(x, size.height + 12), tick);
      }
    }
    final placed = placePieces(pieces, axis, size.width);
    for (final (i, p) in placed.indexed) {
      switch (p.piece.kind) {
        case TimelineKind.wait:
          final paint = Paint()
            ..color = scheme.outlineVariant
            ..strokeWidth = 2
            ..strokeCap = StrokeCap.round;
          if (p.width >= 4) {
            canvas.drawLine(
              Offset(p.left + 2, mid),
              Offset(p.right - 2, mid),
              paint,
            );
          }
        case TimelineKind.street:
          _street(canvas, p, mid);
        case TimelineKind.ride:
          final before = i > 0 ? placed[i - 1].piece : null;
          _ride(
            canvas,
            p,
            size.height,
            joined: before?.kind == TimelineKind.ride && before!.continues,
          );
      }
    }
  }

  void _street(Canvas canvas, PlacedPiece p, double mid) {
    final paint = Paint()..color = scheme.onSurfaceVariant;
    final width = p.width - 4;
    if (width <= 0) return;
    if (p.piece.mode == 'cycling') {
      // Dashes for a bike.
      paint
        ..strokeWidth = 3
        ..strokeCap = StrokeCap.round;
      final count = math.max(1, (width / 9).floor());
      final step = width / count;
      for (var k = 0; k < count; k++) {
        final x = p.left + 2 + k * step + step / 2;
        canvas.drawLine(Offset(x - 2, mid), Offset(x + 2, mid), paint);
      }
    } else {
      // Dots for a walk or a wheelchair.
      final count = math.max(1, (width / 6).floor());
      final step = width / count;
      for (var k = 0; k < count; k++) {
        canvas.drawCircle(
          Offset(p.left + 2 + k * step + step / 2, mid),
          1.6,
          paint,
        );
      }
    }
  }

  void _ride(
    Canvas canvas,
    PlacedPiece p,
    double height, {
    required bool joined,
  }) {
    final route = p.piece.route;
    final color = parseRouteColor(route?.color) ?? scheme.primary;
    final text = routeTextColor(route, color);
    final mid = height / 2;
    final notch = height * 0.25;
    const r = 6.0;
    const radius = Radius.circular(r);
    // A hairline gap between pieces. Where the vehicle goes on as another route, this
    // piece points into the next one's notch.
    final left = joined ? p.left - notch + 1 : p.left + 1;
    final right = p.right - 1;
    if (right - left < 2 * r) return;
    final path = Path()..moveTo(joined ? left : left + r, 0);
    if (p.piece.continues) {
      path
        ..lineTo(right - notch, 0)
        ..lineTo(right, mid)
        ..lineTo(right - notch, height);
    } else {
      path
        ..lineTo(right - r, 0)
        ..arcToPoint(Offset(right, r), radius: radius)
        ..lineTo(right, height - r)
        ..arcToPoint(Offset(right - r, height), radius: radius);
    }
    if (joined) {
      path
        ..lineTo(left, height)
        ..lineTo(left + notch, mid);
    } else {
      path
        ..lineTo(left + r, height)
        ..arcToPoint(Offset(left, height - r), radius: radius)
        ..lineTo(left, r)
        ..arcToPoint(Offset(left + r, 0), radius: radius);
    }
    path.close();
    canvas.drawPath(path, Paint()..color = color);
    if (_lowContrast(color)) {
      canvas.drawPath(
        path,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1
          ..color = scheme.outline,
      );
    }
    final body = Rect.fromLTRB(
      joined ? left + notch : left,
      0,
      p.piece.continues ? right - notch : right,
      height,
    );
    _label(canvas, body, route, text);
  }

  /// Whether [color] would blend into the sheet behind it.
  bool _lowContrast(Color color) {
    final a = color.computeLuminance(), b = scheme.surface.computeLuminance();
    final ratio = (math.max(a, b) + 0.05) / (math.min(a, b) + 0.05);
    return ratio < 1.6;
  }

  void _label(Canvas canvas, Rect rect, TransitRoute? route, Color color) {
    final room = rect.width - 6;
    if (room < 10) return;
    TextPainter text(String label) => TextPainter(
      text: TextSpan(
        text: label,
        style: textStyle.copyWith(color: color),
      ),
      textDirection: TextDirection.ltr,
      textScaler: textScaler,
      maxLines: 1,
    )..layout();
    final name = _badgeText(route);
    var painter = text(name);
    // "E Line" or "1 Line" says "E" or "1" where it must, as its signs do.
    if (painter.width > room && name.endsWith(' Line') && name.length > 5) {
      painter = text(name.substring(0, name.length - 5));
    }
    if (painter.width > room) {
      // Too narrow for the name: the vehicle's icon, if that fits.
      final icon = vehicleIcon(route?.kind ?? VehicleKind.other);
      final size = math.min(16.0, rect.height - 6);
      if (room + 4 < size) return;
      final glyph = TextPainter(
        text: TextSpan(
          text: String.fromCharCode(icon.codePoint),
          style: TextStyle(
            fontFamily: icon.fontFamily,
            package: icon.fontPackage,
            fontSize: size,
            color: color,
            height: 1,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      glyph.paint(
        canvas,
        rect.center - Offset(glyph.width / 2, glyph.height / 2),
      );
      return;
    }
    painter.paint(
      canvas,
      rect.center - Offset(painter.width / 2, painter.height / 2),
    );
  }

  @override
  bool shouldRepaint(_TimelinePainter old) =>
      !identical(old.pieces, pieces) && !_samePieces(old.pieces, pieces) ||
      old.axis != axis ||
      old.scheme != scheme ||
      old.textStyle != textStyle ||
      old.textScaler != textScaler ||
      old.ticks != ticks;

  static bool _samePieces(List<TimelinePiece> a, List<TimelinePiece> b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i].kind != b[i].kind ||
          a[i].start != b[i].start ||
          a[i].end != b[i].end ||
          a[i].route?.name != b[i].route?.name) {
        return false;
      }
    }
    return true;
  }
}

/// Times along the overview's axis, above its rows.
class TimelineTicks extends StatelessWidget {
  const TimelineTicks({super.key, required this.axis});

  final TimelineAxis axis;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final style = theme.textTheme.labelSmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        return SizedBox(
          height: 16,
          child: Stack(
            clipBehavior: Clip.none,
            children: [
              for (final t in axis.ticks(width))
                Positioned(
                  left: axis.x(t, width) - 40,
                  width: 80,
                  child: Text(
                    clock(context, t),
                    textAlign: TextAlign.center,
                    style: style,
                    maxLines: 1,
                  ),
                ),
            ],
          ),
        );
      },
    );
  }
}

/// A journey in the overview: its timeline on the shared axis, when to go, and how long
/// it takes.
class JourneyRow extends StatelessWidget {
  const JourneyRow({
    super.key,
    required this.journey,
    required this.axis,
    required this.now,
    required this.onTap,
  });

  final Journey journey;
  final TimelineAxis axis;
  final DateTime now;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: scheme.onSurfaceVariant,
    );
    final offset = journeyUtcOffset(journey);
    final moved = journey.legs.whereType<StreetLeg>().any(
      (leg) => leg.stopMoved.isNotEmpty,
    );
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 10, 16, 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            JourneyTimeline(journey: journey, axis: axis, ticks: true),
            const SizedBox(height: 6),
            Row(
              crossAxisAlignment: CrossAxisAlignment.baseline,
              textBaseline: TextBaseline.alphabetic,
              children: [
                Expanded(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        goInSpan(theme, journey, now),
                        TextSpan(
                          text:
                              '   ${clock(context, journey.departure, utcOffset: offset)}'
                              ' – '
                              '${clock(context, journey.arrival, utcOffset: offset)}',
                          style: muted,
                        ),
                      ],
                    ),
                    style: theme.textTheme.titleSmall,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
                if (moved) ...[
                  Icon(Icons.wrong_location, size: 16, color: scheme.error),
                  const SizedBox(width: 8),
                ],
                Text(
                  formatDuration(journey.duration.toDouble()),
                  style: theme.textTheme.titleSmall,
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// "Go in 5 min", with the minutes in the live color when the first ride's times are
/// live.
class GoIn extends StatelessWidget {
  const GoIn({super.key, required this.journey, required this.now, this.style});

  final Journey journey;
  final DateTime now;
  final TextStyle? style;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Text.rich(
      goInSpan(theme, journey, now),
      style: style ?? theme.textTheme.titleSmall,
      maxLines: 1,
      softWrap: false,
    );
  }
}

/// "Go in 5 min" or "Go now", from [now], with the time in the live color when the
/// first ride's times are live.
TextSpan goInSpan(ThemeData theme, Journey journey, DateTime now) {
  final live = journey.rides.firstOrNull?.realtime ?? false;
  final minutes = (journey.departure.difference(now).inSeconds / 60).floor();
  final liveStyle = live ? TextStyle(color: liveColor(theme)) : null;
  return TextSpan(
    children: minutes <= 0
        ? [TextSpan(text: 'Go now', style: liveStyle)]
        : [
            const TextSpan(text: 'Go in '),
            TextSpan(text: formatDuration(minutes * 60.0), style: liveStyle),
          ],
  );
}
