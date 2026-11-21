/// Scrubbing a row's graph: a finger dragged sideways along it, or a screen reader
/// stepping along it, shows a hairline there, says in the row's headline what's there,
/// and tells the page where that is on the map.
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../ui/adaptive.dart';
import '../units.dart';
import 'scrub_track.dart';
import 'transit_plan.dart';
import 'transit_timeline.dart';

/// What a page does as the graphs of its rows are scrubbed. Each call names the row
/// by an object of its own, the same throughout one scrub.
class ScrubSink {
  const ScrubSink({
    required this.onStart,
    required this.onMove,
    required this.onEnd,
  });

  /// A scrub of a row started; [select] selects the row.
  final void Function(Object row, VoidCallback select) onStart;

  /// It's at [point] on the map, or somewhere the row can't place.
  final void Function(Object row, DirectionsPoint? point) onMove;

  /// It ended: the finger lifted, or the screen reader's step was a second ago.
  final void Function(Object row) onEnd;
}

/// What a scrub at a point across a graph is at: what the readout says, most important
/// first, and where it is on the map.
typedef ScrubSpot = ({List<String> readout, DirectionsPoint? point});

/// How a row's graph is scrubbed: where across it it's drawn, and what's at each point.
abstract class ScrubTrack {
  const ScrubTrack();

  /// Where the graph is drawn across [width]: the hairline is kept within it.
  ({double left, double right}) extent(double width);

  /// What's at [x] across [width], within the [extent].
  ScrubSpot at(double x, double width);
}

/// A street route's graph, as wide as [share] of the width it's given, its width the
/// time its runs take.
class StreetScrubTrack extends ScrubTrack {
  const StreetScrubTrack({
    required this.route,
    required this.share,
    required this.driving,
    required this.units,
  });

  final DirectionsRoute route;
  final double share;
  final bool driving;
  final UnitSystem units;

  @override
  ({double left, double right}) extent(double width) =>
      (left: 0, right: width * share);

  @override
  ScrubSpot at(double x, double width) {
    final drawn = width * share;
    final fraction = drawn > 0 ? (x / drawn).clamp(0.0, 1.0) : 0.0;
    return (
      readout: streetReadout(route, fraction, driving: driving, units: units),
      point: streetPointAt(route, fraction),
    );
  }
}

/// A journey's timeline on [axis]: its time is the clock, which [clock] writes.
class JourneyScrubTrack extends ScrubTrack {
  JourneyScrubTrack({
    required this.journey,
    required this.axis,
    required this.clock,
  });

  final Journey journey;
  final TimelineAxis axis;
  final String Function(DateTime time) clock;

  late final _pieces = timelinePieces(journey);
  List<PlacedPiece>? _placed;
  double _placedWidth = -1;

  List<PlacedPiece> _placedAt(double width) {
    if (width != _placedWidth) {
      _placed = placePieces(_pieces, axis, width);
      _placedWidth = width;
    }
    return _placed!;
  }

  @override
  ({double left, double right}) extent(double width) {
    final placed = _placedAt(width);
    if (placed.isEmpty) {
      return (
        left: axis.x(journey.departure, width),
        right: axis.x(journey.arrival, width),
      );
    }
    return (left: placed.first.left, right: placed.last.right);
  }

  @override
  ScrubSpot at(double x, double width) {
    final placed = _placedAt(width);
    final time = placed.isEmpty ? journey.departure : timelineTimeAt(placed, x);
    return (
      readout: [clock(time), journeyActivity(journey, time)],
      point: journeyPointAt(journey, time),
    );
  }
}

/// How many steps a screen reader takes from one end of a graph to the other.
const scrubSteps = 10;

/// How long after a screen reader's last step its scrub ends, as a finger's would on
/// lifting.
const scrubStepLinger = Duration(seconds: 1);

/// How tall the touch zone around a graph is, at the least.
const scrubZoneHeight = 44.0;

/// How wide the edge of the window is, on iOS, from which a drag is the back gesture.
const backSwipeEdge = 20.0;

/// A row's scrub: where the hairline is, what the readout says, and how much they
/// show. It notifies its listeners as either changes, for them to paint again.
class RowScrub extends ChangeNotifier {
  RowScrub._(this._state);

  final _RowScrubberState _state;

  double? _x;
  List<String> _readout = const [];

  /// Where the hairline is across the graph's zone, or null if it's never been put.
  double? get x => _x;

  /// What the readout says, most important first.
  List<String> get readout => _readout;

  /// How much the hairline and readout show, and the text they replace doesn't.
  Animation<double> get shown => _state._shownCurve;

  /// Whether a finger or a screen reader is scrubbing it.
  bool get active => _state._active;

  ScrubTrack get track => _state.widget.track;

  void _set(double x, List<String> readout) {
    _x = x;
    _readout = readout;
    notifyListeners();
  }
}

/// Gives the row it builds a [RowScrub], for its graph ([ScrubGraph]) and the text
/// its readout takes the place of ([ScrubReadout]); with no [sink], none, and the
/// row doesn't scrub.
///
/// A scrub selects the row ([onSelect]) as it starts, through the [sink].
class RowScrubber extends StatefulWidget {
  const RowScrubber({
    super.key,
    required this.track,
    required this.sink,
    required this.onSelect,
    required this.builder,
  });

  final ScrubTrack track;
  final ScrubSink? sink;
  final VoidCallback? onSelect;
  final Widget Function(BuildContext context, RowScrub? scrub) builder;

  @override
  State<RowScrubber> createState() => _RowScrubberState();
}

class _RowScrubberState extends State<RowScrubber>
    with SingleTickerProviderStateMixin {
  late final _shown = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 200),
  );
  late final _shownCurve = CurvedAnimation(
    parent: _shown,
    curve: Curves.easeOut,
  );
  late final _scrub = RowScrub._(this);
  bool _active = false;

  /// The sink the scrub under way started with, which hears it end whatever the row
  /// is given since.
  ScrubSink? _sink;

  /// Where a screen reader has stepped to, of [scrubSteps].
  int _step = 0;
  Timer? _stepEnd;

  /// The width of the graph's zone, as last laid out.
  double _width = 0;

  @override
  void didUpdateWidget(RowScrubber oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!_active) return;
    if (widget.sink != _sink) {
      // The row no longer scrubs: on its way out, say.
      _end();
    } else if (_scrub.x case final x?) {
      // A newer plan's times, say: what's at the hairline, now.
      _move(x);
    }
  }

  @override
  void dispose() {
    _stepEnd?.cancel();
    if (_active) _sink?.onEnd(this);
    _scrub.dispose();
    _shownCurve.dispose();
    _shown.dispose();
    super.dispose();
  }

  void _start(double x) {
    final sink = _sink = widget.sink;
    if (sink == null) return;
    _active = true;
    sink.onStart(this, widget.onSelect ?? () {});
    _shown.forward();
    _move(x);
  }

  void _move(double x) {
    if (!_active) return;
    final extent = widget.track.extent(_width);
    final at = x
        .clamp(extent.left, math.max(extent.left, extent.right))
        .toDouble();
    final spot = widget.track.at(at, _width);
    _scrub._set(at, spot.readout);
    _sink?.onMove(this, spot.point);
  }

  void _end() {
    _stepEnd?.cancel();
    _stepEnd = null;
    if (!_active) return;
    _active = false;
    _sink?.onEnd(this);
    _sink = null;
    _shown.reverse();
  }

  /// Where the screen reader's step [step] is across the graph.
  double _stepX(int step) {
    final extent = widget.track.extent(_width);
    return extent.left + (extent.right - extent.left) * step / scrubSteps;
  }

  /// A screen reader's step along the graph, by [by]: a scrub there that ends
  /// [scrubStepLinger] after the last.
  void _stepBy(int by) {
    // For the slider's value and actions to be those of the new step.
    setState(() => _step = (_step + by).clamp(0, scrubSteps));
    final x = _stepX(_step);
    if (_active) {
      _move(x);
    } else {
      _start(x);
    }
    _stepEnd?.cancel();
    _stepEnd = Timer(scrubStepLinger, _end);
  }

  @override
  Widget build(BuildContext context) =>
      widget.builder(context, widget.sink == null ? null : _scrub);
}

/// A drag that scrubs: sideways, from a pointer that didn't go down where the system
/// or iOS takes a drag from the window's edge for going back.
class _ScrubDrag extends HorizontalDragGestureRecognizer {
  _ScrubDrag({super.debugOwner});

  bool Function(Offset position) allows = _always;

  static bool _always(Offset position) => true;

  @override
  bool isPointerAllowed(PointerEvent event) =>
      allows(event.position) && super.isPointerAllowed(event);
}

/// A row's graph, [child], where it's scrubbed: in a zone [scrubZoneHeight] tall at
/// the least, [padding] around the graph, which it catches sideways drags in, with
/// the hairline over the graph. To screen readers it's a slider, [label], whose value
/// is the readout where it's stepped to.
///
/// A drag that starts sideways scrubs; one that starts up or down is the sheet's, and
/// a tap the row's. Once a scrub starts it has the finger until it lifts.
class ScrubGraph extends StatelessWidget {
  const ScrubGraph({
    super.key,
    required this.scrub,
    required this.label,
    required this.child,
    this.padding = EdgeInsets.zero,
  });

  final RowScrub? scrub;
  final String label;
  final EdgeInsets padding;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final scrub = this.scrub;
    if (scrub == null) return Padding(padding: padding, child: child);
    final state = scrub._state;
    final color = Theme.of(context).colorScheme.primary;
    final window = MediaQuery.sizeOf(context);
    final gestures = MediaQuery.systemGestureInsetsOf(context);
    final rtl = Directionality.of(context) == TextDirection.rtl;
    final back = isCupertinoOf(context) ? backSwipeEdge : 0.0;
    final left = math.max(gestures.left, rtl ? 0.0 : back);
    final right = math.max(gestures.right, rtl ? back : 0.0);
    bool allows(Offset position) =>
        position.dx >= left && position.dx <= window.width - right;
    return LayoutBuilder(
      builder: (context, constraints) {
        final width = constraints.maxWidth;
        state._width = width;
        final extent = scrub.track.extent(width);
        // At least as wide as a finger, within the row.
        final zoneWidth = math.min(
          width,
          math.max(scrubZoneHeight, extent.right - extent.left),
        );
        final zoneLeft = extent.left.clamp(0.0, width - zoneWidth);
        String valueAt(int step) =>
            scrub.track.at(state._stepX(step), width).readout.join(', ');
        return Semantics(
          container: true,
          slider: true,
          label: label,
          value: valueAt(state._step),
          increasedValue: state._step < scrubSteps
              ? valueAt(state._step + 1)
              : null,
          decreasedValue: state._step > 0 ? valueAt(state._step - 1) : null,
          onIncrease: state._step < scrubSteps ? () => state._stepBy(1) : null,
          onDecrease: state._step > 0 ? () => state._stepBy(-1) : null,
          child: ExcludeSemantics(
            child: Stack(
              children: [
                Padding(padding: padding, child: child),
                Positioned(
                  left: padding.left,
                  right: padding.right,
                  top: padding.top,
                  bottom: padding.bottom,
                  child: IgnorePointer(
                    child: RepaintBoundary(
                      child: CustomPaint(
                        painter: _HairlinePainter(
                          scrub,
                          color: color,
                          offset: -padding.left,
                        ),
                      ),
                    ),
                  ),
                ),
                Positioned(
                  left: zoneLeft,
                  width: zoneWidth,
                  top: 0,
                  bottom: 0,
                  child: RawGestureDetector(
                    behavior: HitTestBehavior.translucent,
                    gestures: {
                      _ScrubDrag:
                          GestureRecognizerFactoryWithHandlers<_ScrubDrag>(
                            () => _ScrubDrag(debugOwner: this),
                            (drag) => drag
                              ..allows = allows
                              ..onStart = (details) {
                                state._start(
                                  details.localPosition.dx + zoneLeft,
                                );
                              }
                              ..onUpdate = (details) {
                                state._move(
                                  details.localPosition.dx + zoneLeft,
                                );
                              }
                              ..onEnd = (_) {
                                state._end();
                              }
                              ..onCancel = state._end,
                          ),
                    },
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// Paints a row's hairline where its scrub is, as far as it shows.
class _HairlinePainter extends CustomPainter {
  _HairlinePainter(this.scrub, {required this.color, required this.offset})
    : super(repaint: Listenable.merge([scrub, scrub.shown]));

  final RowScrub scrub;
  final Color color;

  /// Where the graph is across the zone the scrub's position is in.
  final double offset;

  static const width = 2.0;

  @override
  void paint(Canvas canvas, Size size) {
    final x = scrub.x;
    final shown = scrub.shown.value;
    if (x == null || shown <= 0) return;
    final at = (x + offset).clamp(width / 2, size.width - width / 2);
    canvas.drawLine(
      Offset(at, 0),
      Offset(at, size.height),
      Paint()
        ..color = color.withValues(alpha: color.a * shown)
        ..strokeWidth = width,
    );
  }

  @override
  bool shouldRepaint(_HairlinePainter oldDelegate) =>
      scrub != oldDelegate.scrub ||
      color != oldDelegate.color ||
      offset != oldDelegate.offset;
}

/// [child], the text of a row that a scrub's readout replaces: while the row's
/// scrubbed, the readout fades in over it in [style] as it fades out, and back. The
/// readout takes [child]'s room and no more, so nothing moves; what it can't fit on
/// its line it leaves off, from the end. Where [child] has [lines] lines, more than
/// one, the readout wraps onto as many, from the top.
class ScrubReadout extends StatelessWidget {
  const ScrubReadout({
    super.key,
    required this.scrub,
    required this.style,
    required this.child,
    this.alignment = TextAlign.start,
    this.lines = 1,
  });

  final RowScrub? scrub;
  final TextStyle style;
  final TextAlign alignment;
  final int lines;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final scrub = this.scrub;
    if (scrub == null) return child;
    // As wide as the room [child] is given, whatever it fills of it.
    return Stack(
      fit: StackFit.passthrough,
      children: [
        FadeTransition(opacity: ReverseAnimation(scrub.shown), child: child),
        Positioned.fill(
          child: IgnorePointer(
            child: ExcludeSemantics(
              child: FadeTransition(
                opacity: scrub.shown,
                child: RepaintBoundary(
                  child: CustomPaint(
                    painter: ReadoutPainter(
                      scrub,
                      style: DefaultTextStyle.of(context).style.merge(style),
                      textScaler: MediaQuery.textScalerOf(context),
                      direction: Directionality.of(context),
                      alignment: alignment,
                      lines: lines,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// The parts of a readout that fit [width] in [style], joined: as many as fit from
/// the first, and at least the first, cut with an ellipsis if even it doesn't fit.
String fittedReadout(
  List<String> parts,
  double width, {
  required TextStyle style,
  required TextScaler textScaler,
  required TextDirection direction,
}) {
  for (var count = parts.length; count > 1; count--) {
    final text = parts.take(count).join(' · ');
    final painter = TextPainter(
      text: TextSpan(text: text, style: style),
      textDirection: direction,
      textScaler: textScaler,
      maxLines: 1,
    )..layout();
    final fits = painter.width <= width;
    painter.dispose();
    if (fits) return text;
  }
  return parts.firstOrNull ?? '';
}

/// Paints a scrub's readout (see [ScrubReadout]).
class ReadoutPainter extends CustomPainter {
  ReadoutPainter(
    this.scrub, {
    required this.style,
    required this.textScaler,
    required this.direction,
    required this.alignment,
    this.lines = 1,
  }) : super(repaint: scrub);

  final RowScrub scrub;
  final TextStyle style;
  final TextScaler textScaler;
  final TextDirection direction;
  final TextAlign alignment;

  /// How many lines it has: one, in the middle of its room, which holds the parts
  /// that fit; or more, from the top, which all the parts wrap onto.
  final int lines;

  /// The text last laid out, and for what, so a readout that says the same isn't
  /// laid out again.
  TextPainter? _painter;
  (List<String>, double)? _for;

  /// What it last painted, for tests.
  @visibleForTesting
  String? painted;

  @override
  void paint(Canvas canvas, Size size) {
    final parts = scrub.readout;
    if (parts.isEmpty) return;
    if (_for case (final p, final w) when p == parts && w == size.width) {
    } else {
      final text = lines > 1
          ? parts.join(' · ')
          : fittedReadout(
              parts,
              size.width,
              style: style,
              textScaler: textScaler,
              direction: direction,
            );
      _painter?.dispose();
      _painter = TextPainter(
        text: TextSpan(text: text, style: style),
        textDirection: direction,
        textScaler: textScaler,
        textAlign: alignment,
        maxLines: lines,
        ellipsis: '…',
      )..layout(minWidth: size.width, maxWidth: size.width);
      _for = (parts, size.width);
      painted = text;
    }
    final painter = _painter!;
    painter.paint(
      canvas,
      Offset(0, lines > 1 ? 0 : (size.height - painter.height) / 2),
    );
  }

  @override
  bool shouldRepaint(ReadoutPainter oldDelegate) =>
      scrub != oldDelegate.scrub ||
      style != oldDelegate.style ||
      textScaler != oldDelegate.textScaler ||
      direction != oldDelegate.direction ||
      alignment != oldDelegate.alignment ||
      lines != oldDelegate.lines;
}
