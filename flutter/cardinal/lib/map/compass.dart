import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../ui/adaptive.dart';
import '../ui/glass.dart';
import '../ui/haptics.dart';
import 'map.dart';
import 'map_control.dart';

/// The map's compass, for [MapButtons] to show above its other buttons: it shows while
/// the map isn't north up, and a tap turns it back.
class CompassButton extends StatefulWidget {
  const CompassButton({super.key});

  @override
  State<CompassButton> createState() => _CompassButtonState();
}

class _CompassButtonState extends State<CompassButton> {
  SparkleMapController? _map;
  double _bearing = 0;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final map = MapScope.of(context);
    if (map == _map) return;
    _map?.cameraMoved.removeListener(_moved);
    _map = map..cameraMoved.addListener(_moved);
    _moved();
  }

  @override
  void dispose() {
    _map?.cameraMoved.removeListener(_moved);
    super.dispose();
  }

  void _moved() {
    final bearing = _map?.camera?.bearing ?? 0;
    if (bearing != _bearing) setState(() => _bearing = bearing);
  }

  @override
  Widget build(BuildContext context) =>
      Compass(bearing: _bearing, onPressed: () => _map?.turnNorth());
}

/// A compass needle for a map turned to [bearing] (degrees clockwise from north), which
/// slides in from the end and fades in while it isn't north up, and out once it is.
class Compass extends StatelessWidget {
  const Compass({super.key, required this.bearing, required this.onPressed});

  final double bearing;
  final VoidCallback onPressed;

  /// Turned less than this many degrees, the map is north up.
  static const double northUp = 0.05;

  static const _duration = Duration(milliseconds: 250);

  @override
  Widget build(BuildContext context) {
    final shown = bearing.abs() >= northUp;
    final cupertino = isCupertinoOf(context);
    // Points to north on screen: the other way from how the map turned.
    final needle = Transform.rotate(
      angle: -bearing * math.pi / 180,
      child: const CustomPaint(
        size: Size.square(28),
        painter: _NeedlePainter(),
      ),
    );
    void turn() {
      Haptics.selection();
      onPressed();
    }

    final button = cupertino
        ? AppGlassLayer(
            tintAmount: 0.8,
            child: GlassSurface(
              radius: MapControl.size / 2,
              floating: true,
              child: Semantics(
                button: true,
                label: 'Turn north up',
                excludeSemantics: true,
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: turn,
                  child: SizedBox.square(
                    dimension: MapControl.size,
                    child: Center(child: needle),
                  ),
                ),
              ),
            ),
          )
        : FloatingActionButton(
            heroTag: null,
            tooltip: 'Turn north up',
            shape: const CircleBorder(),
            onPressed: turn,
            child: needle,
          );
    return IgnorePointer(
      ignoring: !shown,
      child: ExcludeSemantics(
        excluding: !shown,
        child: AnimatedSlide(
          // In from the end of the screen, where the buttons are.
          offset: shown
              ? Offset.zero
              : Offset(
                  Directionality.of(context) == TextDirection.ltr ? 0.6 : -0.6,
                  0,
                ),
          duration: _duration,
          curve: Curves.easeOutCubic,
          child: AnimatedOpacity(
            opacity: shown ? 1 : 0,
            duration: _duration,
            curve: Curves.easeOut,
            child: button,
          ),
        ),
      ),
    );
  }
}

/// A two-tone needle pointing up: red to the north, a quieter tone to the south.
class _NeedlePainter extends CustomPainter {
  const _NeedlePainter();

  @override
  void paint(Canvas canvas, Size size) {
    final c = size.center(Offset.zero);
    final half = size.height / 2;
    final width = size.width * 0.18;
    final north = Path()
      ..moveTo(c.dx, c.dy - half)
      ..lineTo(c.dx + width, c.dy)
      ..lineTo(c.dx - width, c.dy)
      ..close();
    final south = Path()
      ..moveTo(c.dx, c.dy + half)
      ..lineTo(c.dx + width, c.dy)
      ..lineTo(c.dx - width, c.dy)
      ..close();
    canvas
      ..drawPath(north, Paint()..color = const Color(0xFFE5483B))
      ..drawPath(south, Paint()..color = const Color(0xFF9AA0A6));
  }

  @override
  bool shouldRepaint(_NeedlePainter oldDelegate) => false;
}
