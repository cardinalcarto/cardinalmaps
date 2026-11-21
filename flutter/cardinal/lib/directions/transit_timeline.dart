/// A transit journey drawn along a time axis: its rides as badges in their routes'
/// colors, and the walks and waits between them as short gaps.
///
/// Rows of the overview share one axis, so journeys line up by the time they happen.
library;

import 'dart:math' as math;
import 'dart:ui' show lerpDouble;

import 'package:material_ui/material_ui.dart';

import 'animated_rows.dart';
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
/// Its pieces sit at their times on [axis], which rows of a list share. With a
/// [chain], it's timed as the departures chosen have it, and when the chain changes it
/// eases there: each piece moves and recolors to where it now is, the labels
/// cross-fade, and pieces that come or go fade in or out.
///
/// With [others], the other routes that can be taken for each ride ([otherRoutes]),
/// a line under it says "or" and badges them, each from the left of the ride that has
/// some, as far as there's room ([placeOrLabels]). Whether it has that line is the
/// journey's alone, so its height doesn't change with a choice; with [reserveOthers]
/// it keeps the line's room whether or not there's anything to say on it, as rows of
/// one height do.
///
/// The timeline and that line are as tall as [height] and [orHeight] say, or as their
/// labels need at the system's text size if that's more, so no label is cut short. A
/// row of a list says where they go in it: the timeline in the middle of [room], and
/// the line [othersGap] under that, [othersHeight] tall.
class JourneyTimeline extends StatefulWidget {
  const JourneyTimeline({
    super.key,
    required this.journey,
    required this.axis,
    this.chain,
    this.others,
    this.reserveOthers = false,
    this.eases,
    this.height = 26,
    this.room,
    this.othersGap = orGap,
    this.othersHeight,
  });

  final Journey journey;
  final TimelineAxis axis;
  final ConnectionChain? chain;
  final List<List<TransitRoute>>? others;

  /// Whether to keep the room of the line of other routes with none to show.
  final bool reserveOthers;

  /// Whether it eases to what it's given next, rather than just showing it: for a
  /// timeline that goes on being the same journey's, as its times or its axis
  /// change. If null, it does while it has a [chain].
  final bool? eases;

  /// The height of the timeline itself, above any line of other routes, at the
  /// default text size.
  final double height;

  /// The room the timeline itself is in the middle of, if that's more than it's
  /// tall: a row's place for its graph.
  final double? room;

  /// The room between the timeline's and the line of other routes.
  final double othersGap;

  /// How tall the line of other routes and its badges are, or null for
  /// [orHeightOf]: a line of a row's text.
  final double? othersHeight;

  /// The height of the line of other routes at the default text size, and the room
  /// over it.
  static const orHeight = 16.0;
  static const orGap = 4.0;

  /// How tall a timeline [height] tall at the default text size is at [context]'s.
  static double barHeightOf(BuildContext context, {double height = 26}) {
    final size = Theme.of(context).textTheme.labelLarge!.fontSize!;
    return math.max(height, MediaQuery.textScalerOf(context).scale(size) + 6);
  }

  /// How tall the line of other routes is at [context]'s text size.
  static double orHeightOf(BuildContext context) {
    final size = Theme.of(context).textTheme.labelMedium!.fontSize!;
    return math.max(orHeight, MediaQuery.textScalerOf(context).scale(size) + 4);
  }

  @override
  State<JourneyTimeline> createState() => JourneyTimelineState();
}

/// What a timeline painted last, to ease on from.
class _Frame {
  List<DrawnPiece>? pieces;
  double width = 0;
}

class JourneyTimelineState extends State<JourneyTimeline>
    with SingleTickerProviderStateMixin {
  late final _t = AnimationController(
    vsync: this,
    duration: rowDuration,
    value: 1,
  );
  late final _curve = CurvedAnimation(parent: _t, curve: rowCurve);
  final _frame = _Frame();
  late var _pieces = timelinePieces(widget.journey, chain: widget.chain);

  /// What it eases from, as it was painted, [_fromWidth] wide.
  List<DrawnPiece>? _from;
  double _fromWidth = 0;
  List<List<TransitRoute>> _fromOthers = const [];

  /// The pieces as last painted, for tests.
  @visibleForTesting
  List<DrawnPiece> get drawn => _frame.pieces ?? const [];

  List<List<TransitRoute>> _othersOf(JourneyTimeline w) {
    final others = w.others;
    return others != null && others.any((o) => o.isNotEmpty)
        ? others
        : const [];
  }

  @override
  void didUpdateWidget(JourneyTimeline oldWidget) {
    super.didUpdateWidget(oldWidget);
    final pieces = timelinePieces(widget.journey, chain: widget.chain);
    final changed =
        !_samePieces(_pieces, pieces) ||
        oldWidget.axis != widget.axis ||
        !_sameOthers(_othersOf(oldWidget), _othersOf(widget));
    _pieces = pieces;
    if (!changed) return;
    final painted = _frame.pieces;
    // A timeline that follows choices eases between them; one that doesn't, such as
    // an overview row given another journey, just changes.
    final eases =
        widget.eases ?? (widget.chain != null && oldWidget.chain != null);
    if (!eases || painted == null) {
      _from = null;
      _t.value = 1;
      return;
    }
    _from = painted;
    _fromWidth = _frame.width;
    _fromOthers = _othersOf(oldWidget);
    _t.forward(from: 0);
  }

  @override
  void dispose() {
    _curve.dispose();
    _t.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final others = _othersOf(widget);
    final hasOthers = others.isNotEmpty || widget.reserveOthers;
    final scaler = MediaQuery.textScalerOf(context);
    final bar = JourneyTimeline.barHeightOf(context, height: widget.height);
    final room = math.max(bar, widget.room ?? 0.0);
    final orHeight = widget.othersHeight ?? JourneyTimeline.orHeightOf(context);
    final orTop = room + widget.othersGap;
    return Semantics(
      label: timelineLabel(_pieces, others: widget.others),
      child: SizedBox(
        height: hasOthers ? orTop + orHeight : room,
        width: double.infinity,
        child: CustomPaint(
          painter: _TimelinePainter(
            pieces: _pieces,
            others: others,
            from: _from,
            fromWidth: _fromWidth,
            fromOthers: _fromOthers,
            t: _curve,
            frame: _frame,
            barTop: (room - bar) / 2,
            barHeight: bar,
            orTop: orTop,
            orHeight: orHeight,
            axis: widget.axis,
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

/// A piece of a timeline as it's drawn: where, in what color and how opaque, which
/// can be part way from how another timeline drew it.
@immutable
class DrawnPiece {
  const DrawnPiece({
    required this.key,
    required this.piece,
    required this.left,
    required this.right,
    required this.color,
    this.joined = false,
    this.opacity = 1,
    this.fromRoute,
    this.labelT = 1,
  });

  /// Which piece of its journey it is, whatever the departures chosen: a ride's trip
  /// by the ride and its place in it, a walk by its place, and a wait by what follows.
  final Object key;
  final TimelinePiece piece;
  final double left;
  final double right;

  /// A ride's fill.
  final Color color;

  /// Whether it's a ride that the one before goes on as.
  final bool joined;
  final double opacity;

  /// For a ride whose route changes, the route whose label fades out as [labelT] goes
  /// from 0 to 1, and the piece's own fades in.
  final TransitRoute? fromRoute;
  final double labelT;

  double get width => right - left;
}

/// Each of [pieces]'s keys ([DrawnPiece.key]).
List<Object> _pieceKeys(List<TimelinePiece> pieces) {
  final keys = List<Object>.filled(pieces.length, '');
  final trips = <int?, int>{};
  var streets = 0;
  for (final (i, p) in pieces.indexed) {
    keys[i] = switch (p.kind) {
      TimelineKind.ride => (
        'ride',
        p.ride,
        trips[p.ride] = (trips[p.ride] ?? -1) + 1,
      ),
      TimelineKind.street => ('street', streets++),
      TimelineKind.wait => '',
    };
  }
  Object after = 'end';
  for (var i = pieces.length - 1; i >= 0; i--) {
    if (pieces[i].kind == TimelineKind.wait) {
      keys[i] = ('wait', after);
    } else {
      after = keys[i];
    }
  }
  return keys;
}

/// [pieces] as drawn on [axis] across [width], rides in their routes' colors or
/// [primary].
List<DrawnPiece> drawnPieces(
  List<TimelinePiece> pieces,
  TimelineAxis axis,
  double width,
  Color primary,
) {
  final placed = placePieces(pieces, axis, width);
  final keys = _pieceKeys(pieces);
  return [
    for (final (i, p) in placed.indexed)
      DrawnPiece(
        key: keys[i],
        piece: p.piece,
        left: p.left,
        right: p.right,
        color: parseRouteColor(p.piece.route?.color) ?? primary,
        joined:
            p.piece.kind == TimelineKind.ride &&
            i > 0 &&
            placed[i - 1].piece.kind == TimelineKind.ride &&
            placed[i - 1].piece.continues,
      ),
  ];
}

/// [t] of the way from [from], drawn [fromWidth] wide, to [to], drawn [width] wide:
/// each piece of both moves and recolors, and one of either alone fades.
List<DrawnPiece> lerpDrawn(
  List<DrawnPiece> from,
  double fromWidth,
  List<DrawnPiece> to,
  double t,
  double width,
) {
  final scale = fromWidth > 0 ? width / fromWidth : 1.0;
  final byKey = {for (final f in from) f.key: f};
  bool alike(DrawnPiece a, DrawnPiece b) =>
      a.piece.kind == b.piece.kind &&
      a.piece.continues == b.piece.continues &&
      a.joined == b.joined;
  final matched = <DrawnPiece>{};
  final coming = <DrawnPiece>[];
  for (final p in to) {
    final f = byKey[p.key];
    if (f != null && alike(f, p)) {
      matched.add(f);
      final relabels =
          routeBadgeText(f.piece.route) != routeBadgeText(p.piece.route);
      coming.add(
        DrawnPiece(
          key: p.key,
          piece: p.piece,
          left: lerpDouble(f.left * scale, p.left, t)!,
          right: lerpDouble(f.right * scale, p.right, t)!,
          color: Color.lerp(f.color, p.color, t)!,
          joined: p.joined,
          opacity: lerpDouble(f.opacity, 1, t)!,
          fromRoute: relabels ? f.piece.route : null,
          labelT: t,
        ),
      );
    } else {
      coming.add(
        DrawnPiece(
          key: p.key,
          piece: p.piece,
          left: p.left,
          right: p.right,
          color: p.color,
          joined: p.joined,
          opacity: t,
        ),
      );
    }
  }
  return [
    // Those going, under those coming.
    for (final f in from)
      if (!matched.contains(f) && f.opacity * (1 - t) > 0.01)
        DrawnPiece(
          key: f.key,
          piece: f.piece,
          left: f.left * scale,
          right: f.right * scale,
          color: f.color,
          joined: f.joined,
          opacity: f.opacity * (1 - t),
        ),
    ...coming,
  ];
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
          routeBadgeText(p.route),
          // Once a ride, at its first piece.
          if (_startsRide(pieces, i))
            for (final route in _othersOf(others, p.ride))
              routeBadgeText(route),
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

/// What a ride's badge calls its [route]: its short name, else its long name, else
/// the kind of vehicle it runs.
String routeBadgeText(TransitRoute? route) {
  final name = route?.shortName ?? route?.longName;
  return name == null || name.isEmpty
      ? (route?.kind ?? VehicleKind.other).label
      : name;
}

class _TimelinePainter extends CustomPainter {
  _TimelinePainter({
    required this.pieces,
    required this.others,
    required this.from,
    required this.fromWidth,
    required this.fromOthers,
    required this.t,
    required this.frame,
    required this.barTop,
    required this.barHeight,
    required this.orTop,
    required this.orHeight,
    required this.axis,
    required this.scheme,
    required this.textStyle,
    required this.orStyle,
    required this.orBadgeStyle,
    required this.textScaler,
  }) : super(repaint: t);

  final List<TimelinePiece> pieces;

  /// The other routes for each ride, or empty for no line of them.
  final List<List<TransitRoute>> others;

  /// What it eases from, as drawn [fromWidth] wide, with its other routes, [t] of the
  /// way.
  final List<DrawnPiece>? from;
  final double fromWidth;
  final List<List<TransitRoute>> fromOthers;
  final Animation<double> t;

  /// Where it notes what it painted.
  final _Frame frame;

  /// Where the timeline itself starts, and how tall it is.
  final double barTop;
  final double barHeight;

  /// Where the line of other routes starts, under the timeline, and how tall it is.
  final double orTop;
  final double orHeight;
  final TimelineAxis axis;
  final ColorScheme scheme;
  final TextStyle textStyle;
  final TextStyle orStyle;
  final TextStyle orBadgeStyle;
  final TextScaler textScaler;

  @override
  void paint(Canvas canvas, Size size) {
    final target = drawnPieces(pieces, axis, size.width, scheme.primary);
    final from = this.from;
    final t = this.t.value;
    final drawn = from == null || t >= 1
        ? target
        : lerpDrawn(from, fromWidth, target, t, size.width);
    frame
      ..pieces = drawn
      ..width = size.width;
    // Each ride's left, where its "or" goes.
    final anchors = <int, double>{
      for (final p in drawn)
        if (p.key case ('ride', final int ride, 0)) ride: p.left + 1,
    };
    final crossing = from != null && t < 1 && !_sameOthers(fromOthers, others);
    canvas.save();
    canvas.translate(0, orTop);
    final line = Size(size.width, orHeight);
    if (crossing && fromOthers.isNotEmpty) {
      _faded(canvas, line, 1 - t, () {
        _others(canvas, anchors, fromOthers, size.width);
      });
    }
    if (others.isNotEmpty) {
      _faded(canvas, line, crossing ? t : 1, () {
        _others(canvas, anchors, others, size.width);
      });
    }
    canvas.restore();
    canvas.save();
    canvas.translate(0, barTop);
    final bar = Size(size.width, barHeight);
    for (final p in drawn) {
      _faded(canvas, bar, p.opacity, () => _piece(canvas, p, bar));
    }
    canvas.restore();
  }

  /// Paints what [draw] does at [opacity].
  void _faded(Canvas canvas, Size size, double opacity, VoidCallback draw) {
    if (opacity >= 1) return draw();
    if (opacity <= 0) return;
    canvas.saveLayer(
      (Offset.zero & size).inflate(4),
      Paint()..color = Color.fromRGBO(0, 0, 0, opacity),
    );
    draw();
    canvas.restore();
  }

  void _piece(Canvas canvas, DrawnPiece p, Size size) {
    final mid = size.height / 2;
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
        _ride(canvas, p, size.height);
    }
  }

  /// The line of other routes: "or" and their badges, under the start of each ride
  /// that has some, at [anchors].
  void _others(
    Canvas canvas,
    Map<int, double> anchors,
    List<List<TransitRoute>> others,
    double width,
  ) {
    const space = 4.0, padding = 4.0, badgeGap = 3.0;
    final height = orHeight;
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
          routeBadgeText(route),
          orBadgeStyle.copyWith(color: routeTextColor(route, color)),
        ),
      );
    }

    // Each ride's anchor, and its badges' colors and labels.
    final rides = <(double, List<(Color, TextPainter)>)>[];
    for (final (ride, anchor) in anchors.entries.map((e) => (e.key, e.value))) {
      final routes = _othersOf(others, ride);
      if (routes.isEmpty) continue;
      rides.add((anchor, [for (final route in routes) badge(route)]));
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

  void _street(Canvas canvas, DrawnPiece p, double mid) {
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

  void _ride(Canvas canvas, DrawnPiece p, double height) {
    final route = p.piece.route;
    final color = p.color;
    final joined = p.joined;
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
    // Each label in its own route's text color, the one going fading out.
    Color textOf(TransitRoute? route) =>
        routeTextColor(route, parseRouteColor(route?.color) ?? scheme.primary);
    final fromRoute = p.fromRoute;
    if (fromRoute != null && p.labelT < 1) {
      _label(
        canvas,
        body,
        fromRoute,
        textOf(fromRoute).withValues(alpha: 1 - p.labelT),
      );
      _label(canvas, body, route, textOf(route).withValues(alpha: p.labelT));
    } else {
      _label(canvas, body, route, textOf(route));
    }
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
    final name = routeBadgeText(route);
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
      !identical(old.from, from) ||
      old.axis != axis ||
      old.scheme != scheme ||
      old.textStyle != textStyle ||
      old.orStyle != orStyle ||
      old.barTop != barTop ||
      old.barHeight != barHeight ||
      old.orTop != orTop ||
      old.orHeight != orHeight ||
      !_sameOthers(old.others, others) ||
      old.textScaler != textScaler;
}

bool _sameOthers(List<List<TransitRoute>> a, List<List<TransitRoute>> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i].length != b[i].length) return false;
    for (var j = 0; j < a[i].length; j++) {
      if (a[i][j].id != b[i][j].id) return false;
    }
  }
  return true;
}

bool _samePieces(List<TimelinePiece> a, List<TimelinePiece> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i].kind != b[i].kind ||
        a[i].start != b[i].start ||
        a[i].end != b[i].end ||
        a[i].continues != b[i].continues ||
        a[i].route?.id != b[i].route?.id ||
        a[i].route?.name != b[i].route?.name ||
        a[i].route?.color != b[i].route?.color) {
      return false;
    }
  }
  return true;
}

/// Times along the overview's axis, above its rows. When the axis changes, as it
/// does when the journeys are planned again, the times slide to where they now are,
/// in step with the rows' timelines.
class TimelineTicks extends ImplicitlyAnimatedWidget {
  const TimelineTicks({super.key, required this.axis})
    : super(duration: rowDuration, curve: rowCurve);

  final TimelineAxis axis;

  /// How tall the times are at [context]'s text size.
  static double heightOf(BuildContext context) {
    final size = Theme.of(context).textTheme.labelSmall!.fontSize!;
    return math.max(16, MediaQuery.textScalerOf(context).scale(size) + 4);
  }

  @override
  ImplicitlyAnimatedWidgetState<TimelineTicks> createState() =>
      _TimelineTicksState();
}

class _TimelineTicksState extends AnimatedWidgetBaseState<TimelineTicks> {
  /// The ends of the axis, in milliseconds since the epoch.
  Tween<double>? _start;
  Tween<double>? _end;

  @override
  void forEachTween(TweenVisitor<dynamic> visitor) {
    Tween<double> tween(dynamic value) => Tween<double>(begin: value as double);
    _start = visitor(
      _start,
      widget.axis.start.millisecondsSinceEpoch.toDouble(),
      tween,
    ) as Tween<double>?;
    _end = visitor(
      _end,
      widget.axis.end.millisecondsSinceEpoch.toDouble(),
      tween,
    ) as Tween<double>?;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final style = theme.textTheme.labelSmall?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
      height: 1,
    );
    DateTime at(Tween<double>? tween, DateTime otherwise) => tween == null
        ? otherwise
        : DateTime.fromMillisecondsSinceEpoch(
            tween.evaluate(animation).round(),
          );
    final axis = TimelineAxis(
      at(_start, widget.axis.start),
      at(_end, widget.axis.end),
    );
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        return SizedBox(
          height: TimelineTicks.heightOf(context),
          child: Stack(
            clipBehavior: Clip.none,
            children: [
              for (final t in axis.ticks(width))
                Positioned(
                  key: ValueKey(t),
                  left: axis.x(t, width) - 60,
                  width: 120,
                  top: 0,
                  bottom: 0,
                  // A time too wide for the room between two shrinks.
                  child: FittedBox(
                    fit: BoxFit.scaleDown,
                    child: Text(clock(context, t), style: style, maxLines: 1),
                  ),
                ),
            ],
          ),
        );
      },
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
