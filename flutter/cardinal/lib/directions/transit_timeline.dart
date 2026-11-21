/// A transit journey drawn along a time axis: its rides as badges in their routes'
/// colors, and the walks and waits between them as short gaps.
///
/// Rows of the overview share one axis, so journeys line up by the time they happen.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import 'connections.dart';
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
    this.ride,
    this.continues = false,
  });

  final TimelineKind kind;
  final DateTime start;
  final DateTime end;

  /// For a street piece: `walking`, `cycling`, or `wheelchair`.
  final String? mode;

  /// For a ride piece: its route.
  final TransitRoute? route;

  /// For a ride piece: which of the journey's rides it's of, by index in
  /// [Journey.rides].
  final int? ride;

  /// For a ride piece: whether the vehicle goes on as the next piece, without the rider
  /// getting off.
  final bool continues;

  int get seconds => end.difference(start).inSeconds;
}

/// [journey]'s timeline, from leaving the start to arriving: a wait fills any time
/// between one leg and the next. With a [chain], it's timed as the departures chosen
/// have it, unless a ride has no connection.
List<TimelinePiece> timelinePieces(Journey journey, {ConnectionChain? chain}) {
  final times = chain?.legTimes;
  final arrival = times == null ? journey.arrival : chain!.arrival!;
  final pieces = <TimelinePiece>[];
  var clock = times == null ? journey.departure : chain!.departure;
  var ride = 0;
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

  for (final (l, leg) in journey.legs.indexed) {
    final start = times?[l].start ?? leg.departure;
    final end = times?[l].end ?? leg.arrival;
    switch (leg) {
      case StreetLeg():
        // A change at the same stop has nothing to draw but the wait.
        if (leg.duration > 0) {
          add(
            TimelinePiece(
              kind: TimelineKind.street,
              start: start,
              end: end,
              mode: leg.mode,
            ),
          );
        }
      case TransitLeg():
        final choice = times == null ? null : chain!.forLeg(l);
        final segments = leg.segments;
        final index = ride++;
        if (segments.length < 2 || (choice != null && !choice.suggested)) {
          // Another departure than the plan's is one trip, as far as is known.
          add(
            TimelinePiece(
              kind: TimelineKind.ride,
              start: start,
              end: end,
              route: choice?.route ?? leg.route,
              ride: index,
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
                ride: index,
                continues: i < segments.length - 1,
              ),
            );
          }
        }
    }
  }
  if (arrival.isAfter(clock)) {
    pieces.add(
      TimelinePiece(kind: TimelineKind.wait, start: clock, end: arrival),
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
/// marks ([TimelineAxis.ticks]) drawn faintly behind it if [ticks] is set. With a
/// [chain], it's timed as the departures chosen have it.
///
/// With [others], the other routes that can be taken for each ride ([otherRoutes]),
/// a line above it says "or" and badges them, over the start of each ride that has
/// some, as far as there's room ([placeOrLabels]).
class JourneyTimeline extends StatelessWidget {
  const JourneyTimeline({
    super.key,
    required this.journey,
    required this.axis,
    this.chain,
    this.others,
    this.ticks = false,
    this.height = 26,
  });

  final Journey journey;
  final TimelineAxis axis;
  final ConnectionChain? chain;
  final List<List<TransitRoute>>? others;
  final bool ticks;

  /// The height of the timeline itself, below any line of other routes.
  final double height;

  /// The height of the line of other routes, and the room under it.
  static const orHeight = 16.0;
  static const orGap = 4.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final pieces = timelinePieces(journey, chain: chain);
    final others = this.others;
    final hasOthers = others != null && others.any((o) => o.isNotEmpty);
    final scaler = MediaQuery.textScalerOf(context);
    return Semantics(
      label: timelineLabel(pieces, others: others),
      child: SizedBox(
        height: hasOthers ? height + orHeight + orGap : height,
        width: double.infinity,
        child: CustomPaint(
          painter: _TimelinePainter(
            pieces: pieces,
            others: hasOthers ? others : const [],
            barTop: hasOthers ? orHeight + orGap : 0,
            axis: axis,
            ticks: ticks,
            scheme: theme.colorScheme,
            textStyle: theme.textTheme.labelLarge!.copyWith(
              fontWeight: FontWeight.w600,
              height: 1,
            ),
            orStyle: theme.textTheme.labelMedium!.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
              height: 1,
            ),
            orBadgeStyle: theme.textTheme.labelMedium!.copyWith(
              fontWeight: FontWeight.w700,
              height: 1,
            ),
            textScaler: scaler,
          ),
        ),
      ),
    );
  }
}

/// What a timeline says, for screen readers: its rides in order, each with the [others]
/// that can be taken for it.
String timelineLabel(
  List<TimelinePiece> pieces, {
  List<List<TransitRoute>>? others,
}) {
  final rides = [
    for (final (i, p) in pieces.indexed)
      if (p.kind == TimelineKind.ride)
        [
          _badgeText(p.route),
          // Once a ride, at its first piece.
          if (_startsRide(pieces, i))
            for (final route in _othersOf(others, p.ride)) _badgeText(route),
        ].join(' or '),
  ];
  return rides.isEmpty ? 'No rides' : rides.join(', then ');
}

/// The routes in [others] for ride [ride], if there are any.
List<TransitRoute> _othersOf(List<List<TransitRoute>>? others, int? ride) =>
    others == null || ride == null || ride >= others.length
    ? const []
    : others[ride];

/// Whether piece [i] is the first of its ride.
bool _startsRide(List<TimelinePiece> pieces, int i) =>
    pieces[i].ride != null &&
    !pieces.take(i).any((p) => p.ride == pieces[i].ride);

/// Where "or" labels go along a timeline [width] wide, each from its [anchors] (the
/// left of the ride it's for), in order: as near its anchor as it can be without
/// overlapping the one before ([gap] apart) or passing the right edge, with as many of
/// its badges as fit. [widths] has each label's width with 1, 2, … badges.
///
/// Returns each label's left and how many badges it shows; none for a label with no
/// room, which isn't drawn.
List<({double left, int count})> placeOrLabels(
  List<double> anchors,
  List<List<double>> widths,
  double width, {
  double gap = 8,
}) {
  final out = <({double left, int count})>[];
  var cursor = 0.0;
  for (final (i, anchor) in anchors.indexed) {
    var placed = (left: 0.0, count: 0);
    for (var count = widths[i].length; count >= 1; count--) {
      final w = widths[i][count - 1];
      var left = math.max(anchor, cursor);
      if (left + w > width) left = width - w;
      if (left >= cursor && left >= 0) {
        placed = (left: left, count: count);
        cursor = left + w + gap;
        break;
      }
    }
    out.add(placed);
  }
  return out;
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
    required this.others,
    required this.barTop,
    required this.axis,
    required this.ticks,
    required this.scheme,
    required this.textStyle,
    required this.orStyle,
    required this.orBadgeStyle,
    required this.textScaler,
  });

  final List<TimelinePiece> pieces;

  /// The other routes for each ride, or empty for no line of them.
  final List<List<TransitRoute>> others;

  /// Where the timeline itself starts, below any line of other routes.
  final double barTop;
  final TimelineAxis axis;
  final bool ticks;
  final ColorScheme scheme;
  final TextStyle textStyle;
  final TextStyle orStyle;
  final TextStyle orBadgeStyle;
  final TextScaler textScaler;

  @override
  void paint(Canvas canvas, Size size) {
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
    if (others.isNotEmpty) _others(canvas, placed, size.width);
    canvas.save();
    canvas.translate(0, barTop);
    _pieces(canvas, placed, Size(size.width, size.height - barTop));
    canvas.restore();
  }

  void _pieces(Canvas canvas, List<PlacedPiece> placed, Size size) {
    final mid = size.height / 2;
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

  /// The line of other routes: "or" and their badges, over the start of each ride
  /// that has some.
  void _others(Canvas canvas, List<PlacedPiece> placed, double width) {
    const space = 4.0, padding = 4.0, badgeGap = 3.0;
    const height = JourneyTimeline.orHeight;
    TextPainter text(String label, TextStyle style) => TextPainter(
      text: TextSpan(text: label, style: style),
      textDirection: TextDirection.ltr,
      textScaler: textScaler,
      maxLines: 1,
      ellipsis: '…',
    )..layout(maxWidth: 56);
    final or = text('or', orStyle);
    (Color, TextPainter) badge(TransitRoute route) {
      final color = parseRouteColor(route.color) ?? scheme.primary;
      return (
        color,
        text(
          _badgeText(route),
          orBadgeStyle.copyWith(color: routeTextColor(route, color)),
        ),
      );
    }

    // Each ride's anchor, and its badges' colors and labels.
    final rides = <(double, List<(Color, TextPainter)>)>[];
    for (final (i, p) in placed.indexed) {
      final routes = _othersOf(others, p.piece.ride);
      if (routes.isEmpty || !_startsRide(pieces, i)) continue;
      rides.add((p.left + 1, [for (final route in routes) badge(route)]));
    }
    double badgeWidth(TextPainter label) => label.width + 2 * padding;
    final widths = [
      for (final (_, badges) in rides)
        [
          for (var n = 1; n <= badges.length; n++)
            or.width +
                space +
                badges.take(n).fold(0.0, (w, b) => w + badgeWidth(b.$2)) +
                (n - 1) * badgeGap,
        ],
    ];
    final spots = placeOrLabels([for (final (a, _) in rides) a], widths, width);
    for (final (i, (_, badges)) in rides.indexed) {
      final spot = spots[i];
      if (spot.count == 0) continue;
      var x = spot.left;
      or.paint(canvas, Offset(x, (height - or.height) / 2));
      x += or.width + space;
      for (final (color, label) in badges.take(spot.count)) {
        final rect = RRect.fromRectAndRadius(
          Rect.fromLTWH(x, 0, badgeWidth(label), height),
          const Radius.circular(4),
        );
        canvas.drawRRect(rect, Paint()..color = color);
        if (_lowContrast(color)) {
          canvas.drawRRect(
            rect,
            Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = 1
              ..color = scheme.outline,
          );
        }
        label.paint(canvas, Offset(x + padding, (height - label.height) / 2));
        x = rect.right + badgeGap;
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
      old.orStyle != orStyle ||
      old.barTop != barTop ||
      !_sameOthers(old.others, others) ||
      old.textScaler != textScaler ||
      old.ticks != ticks;

  static bool _sameOthers(
    List<List<TransitRoute>> a,
    List<List<TransitRoute>> b,
  ) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i].length != b[i].length) return false;
      for (var j = 0; j < a[i].length; j++) {
        if (a[i][j].id != b[i][j].id) return false;
      }
    }
    return true;
  }

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

/// A journey in the overview: its timeline on the shared axis, with the other routes
/// that can be taken for its rides above it, when to go, and how long it takes.
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
            JourneyTimeline(
              journey: journey,
              axis: axis,
              others: otherRoutes(journey),
              ticks: true,
            ),
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
  const GoIn({
    super.key,
    required this.journey,
    required this.now,
    this.leave,
    this.live,
    this.style,
  });

  final Journey journey;
  final DateTime now;

  /// When to leave, and whether the first ride's times are live, when not as the plan
  /// has them: with another departure of its first ride chosen.
  final DateTime? leave;
  final bool? live;
  final TextStyle? style;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Text.rich(
      goInSpan(theme, journey, now, leave: leave, live: live),
      style: style ?? theme.textTheme.titleSmall,
      maxLines: 1,
      softWrap: false,
    );
  }
}

/// "Go in 5 min" or "Go now", from [now], with the time in the live color when the
/// first ride's times are live: as the plan has them, unless [leave] and [live] say.
TextSpan goInSpan(
  ThemeData theme,
  Journey journey,
  DateTime now, {
  DateTime? leave,
  bool? live,
}) {
  live ??= journey.rides.firstOrNull?.realtime ?? false;
  final minutes = ((leave ?? journey.departure).difference(now).inSeconds / 60)
      .floor();
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
