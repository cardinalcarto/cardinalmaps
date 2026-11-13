/// Drawing on the map with a finger: a page freezes the map, so nothing reaches it, or
/// takes the strokes drawn over it, while two fingers still pinch and pan the map.
///
/// [MapDrawing] is the state, in [MapScope], and [MapDrawingLayer] is what lies over the
/// map, under the sheet, taking the touches while a page holds it. The map doesn't know
/// about it: the layer sits on top, and while strokes are taken lets touches through
/// with the map's gestures cut down to [MapGestures.pinch].
library;

import 'dart:async';

import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import 'map.dart';

/// Whether a page holds the map still, and what it does with strokes drawn over it.
///
/// While a page holds it, the map's buttons fade away. Without strokes being taken,
/// nothing reaches the map, no pan, zoom, rotation or tap with any number of fingers.
/// While the page takes strokes, a finger down alone draws, its path handed to the page
/// when it lifts; two fingers pinch to zoom and pan the map, which stays as flat and
/// turned as it was, and a second finger landing while a stroke is drawn throws the
/// stroke away. One finger never pans it.
class MapDrawing extends ChangeNotifier {
  Object? _holder;
  ValueChanged<List<Offset>>? _onStroke;

  /// Whether a page holds the map still.
  bool get frozen => _holder != null;

  /// What takes strokes drawn over the map, in logical pixels within it, or null if
  /// they aren't being taken.
  ValueChanged<List<Offset>>? get onStroke => _onStroke;

  /// Holds the map still for [holder], taking strokes with [onStroke] if given. A page
  /// calls it again to start or stop taking strokes.
  void hold(Object holder, {ValueChanged<List<Offset>>? onStroke}) {
    if (_holder == holder && _onStroke == onStroke) return;
    _holder = holder;
    _onStroke = onStroke;
    notifyListeners();
  }

  /// Lets the map go, if [holder] holds it.
  void release(Object holder) {
    if (_holder != holder) return;
    _holder = null;
    _onStroke = null;
    notifyListeners();
  }
}

/// Lies over the map, with the map's buttons ([child]) in it: while [MapDrawing] is
/// frozen it hides them and takes every touch, drawing strokes if they're being taken.
class MapDrawingLayer extends StatelessWidget {
  const MapDrawingLayer({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final drawing = MapScope.drawingOf(context);
    return ListenableBuilder(
      listenable: drawing,
      builder: (context, _) {
        final frozen = drawing.frozen;
        return Stack(
          fit: StackFit.expand,
          children: [
            IgnorePointer(
              ignoring: frozen,
              child: AnimatedOpacity(
                opacity: frozen ? 0 : 1,
                duration: const Duration(milliseconds: 200),
                child: child,
              ),
            ),
            if (frozen) _StrokeSurface(onStroke: drawing.onStroke),
          ],
        );
      },
    );
  }
}

/// While [onStroke] isn't set, takes every touch, so none reach the map. While it is,
/// draws the stroke of a finger down alone, fading it away once the finger lifts or a
/// second lands, and lets touches through to the map, which only pinches and pans
/// ([MapGestures.pinch]).
class _StrokeSurface extends StatefulWidget {
  const _StrokeSurface({required this.onStroke});

  final ValueChanged<List<Offset>>? onStroke;

  @override
  State<_StrokeSurface> createState() => _StrokeSurfaceState();
}

class _StrokeSurfaceState extends State<_StrokeSurface>
    with SingleTickerProviderStateMixin {
  /// The finger drawing, while it's down.
  int? _pointer;

  /// Every finger down.
  final _down = <int>{};
  final _points = <Offset>[];
  final _changed = ValueNotifier(0);

  /// Fades a finished stroke out, or one thrown away.
  late final _fade = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 300),
    value: 0,
  );

  late final SparkleMapController _map;

  @override
  void initState() {
    super.initState();
    _map = MapScope.of(context);
    _setGestures();
  }

  /// Two fingers move the map while strokes are taken; one doesn't.
  void _setGestures() => _map.gestures = widget.onStroke == null
      ? MapGestures.all
      : MapGestures.pinch;

  @override
  void didUpdateWidget(_StrokeSurface oldWidget) {
    super.didUpdateWidget(oldWidget);
    _setGestures();
    // Strokes stopped being taken: drop one being drawn.
    if (widget.onStroke == null && _pointer != null) {
      _pointer = null;
      _points.clear();
      _changed.value++;
    }
  }

  @override
  void dispose() {
    _map.gestures = MapGestures.all;
    _fade.dispose();
    _changed.dispose();
    super.dispose();
  }

  void _onDown(PointerDownEvent e) {
    _down.add(e.pointer);
    if (_pointer != null) {
      // A second finger: it's moving the map, not drawing.
      _pointer = null;
      unawaited(_fade.reverse());
      return;
    }
    if (widget.onStroke == null || _down.length > 1) return;
    _pointer = e.pointer;
    _fade.value = 1;
    _points
      ..clear()
      ..add(e.localPosition);
    _changed.value++;
  }

  void _move(PointerMoveEvent e) {
    if (e.pointer != _pointer) return;
    _points.add(e.localPosition);
    _changed.value++;
  }

  void _up(PointerUpEvent e) {
    _down.remove(e.pointer);
    if (e.pointer != _pointer) return;
    _pointer = null;
    final stroke = List.of(_points);
    unawaited(_fade.reverse());
    widget.onStroke?.call(stroke);
  }

  void _cancel(PointerCancelEvent e) {
    _down.remove(e.pointer);
    if (e.pointer != _pointer) return;
    _pointer = null;
    unawaited(_fade.reverse());
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final drawing = widget.onStroke != null;
    return Listener(
      // Frozen, every touch stops here; drawing, they go on to the map, too.
      behavior: drawing ? HitTestBehavior.translucent : HitTestBehavior.opaque,
      onPointerDown: _onDown,
      onPointerMove: _move,
      onPointerUp: _up,
      onPointerCancel: _cancel,
      // The stroke itself takes no touches, so they can go through it.
      child: IgnorePointer(
        child: CustomPaint(
          size: Size.infinite,
          painter: _StrokePainter(
            points: _points,
            repaint: Listenable.merge([_changed, _fade]),
            fade: _fade,
            color: areaColor,
            halo: colors.surface,
          ),
        ),
      ),
    );
  }
}

/// The color areas are drawn in, on the screen and on the map.
const areaColor = Color(0xFF3B81DE);

class _StrokePainter extends CustomPainter {
  _StrokePainter({
    required this.points,
    required Listenable repaint,
    required this.fade,
    required this.color,
    required this.halo,
  }) : super(repaint: repaint);

  final List<Offset> points;
  final Animation<double> fade;
  final Color color;

  /// Under the stroke, so it reads over any map.
  final Color halo;

  @override
  void paint(Canvas canvas, Size size) {
    if (points.isEmpty || fade.value == 0) return;
    final path = Path()..moveTo(points.first.dx, points.first.dy);
    for (final p in points.skip(1)) {
      path.lineTo(p.dx, p.dy);
    }
    Paint line(Color color, double width) => Paint()
      ..color = color.withValues(alpha: color.a * fade.value)
      ..style = PaintingStyle.stroke
      ..strokeWidth = width
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    canvas
      ..drawPath(path, line(halo.withValues(alpha: 0.7), 7))
      ..drawPath(path, line(color, 4));
  }

  @override
  bool shouldRepaint(_StrokePainter oldDelegate) =>
      oldDelegate.color != color || oldDelegate.halo != halo;
}
