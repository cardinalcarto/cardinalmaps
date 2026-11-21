/// The graph on a driving route's row: how fast the router expects the route to go,
/// along its time.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

/// How tall a route's graph is, in logical pixels, whatever it draws.
const double routeGraphHeight = 32;

/// How tall the plain bar is that stands for a route with no profile to draw.
const double routeBarHeight = 6;

/// The two tones a profile is filled in: [motorway] for freeways and [other] for
/// every other road.
typedef ProfileTones = ({Color motorway, Color other});

/// The profile's tones for [scheme], both of its primary color's hue. The freeway's is
/// the one that stands out from the sheet: the darker on a light sheet, the lighter on
/// a dark one, and well apart in lightness either way.
ProfileTones profileTones(ColorScheme scheme) {
  final primary = HSLColor.fromColor(scheme.primary);
  // Enough color to tell the tones from the greys around them, whatever the seed.
  final hue = primary.withSaturation(math.max(primary.saturation, 0.35));
  final dark = scheme.brightness == Brightness.dark;
  return (
    motorway: hue.withLightness(dark ? 0.80 : 0.30).toColor(),
    other: hue.withLightness(dark ? 0.48 : 0.64).toColor(),
  );
}

/// A route's speed profile, as wide as it's given and [routeGraphHeight] tall: steps
/// whose height is the speed of each of [runs] and whose width is its time, filled in
/// the freeway's tone or the other.
///
/// [topSpeed] is the speed that's drawn full height: the fastest run of any route in
/// the list, so the rows compare. With no runs it's a plain bar along the bottom.
///
/// It's a layer of its own, painted once: nothing about the sheet moving paints it
/// again.
class SpeedProfile extends StatelessWidget {
  const SpeedProfile({
    super.key,
    required this.runs,
    required this.topSpeed,
    required this.tones,
  });

  final List<DirectionsRun> runs;
  final double topSpeed;
  final ProfileTones tones;

  @override
  Widget build(BuildContext context) => RepaintBoundary(
    child: CustomPaint(
      size: const Size(double.infinity, routeGraphHeight),
      painter: SpeedProfilePainter(
        runs: runs,
        topSpeed: topSpeed,
        tones: tones,
      ),
    ),
  );
}

/// A plain bar along the bottom of a graph's space, for a route with nothing to draw
/// above it yet.
class RouteBar extends StatelessWidget {
  const RouteBar({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) => SizedBox(
    height: routeGraphHeight,
    child: Align(
      alignment: Alignment.bottomCenter,
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: color,
          borderRadius: BorderRadius.circular(routeBarHeight / 2),
        ),
        child: const SizedBox(height: routeBarHeight, width: double.infinity),
      ),
    ),
  );
}

/// One step of a profile, as it's drawn.
typedef _Step = ({double left, double right, double top, bool motorway});

/// Paints a [SpeedProfile].
class SpeedProfilePainter extends CustomPainter {
  const SpeedProfilePainter({
    required this.runs,
    required this.topSpeed,
    required this.tones,
  });

  final List<DirectionsRun> runs;
  final double topSpeed;
  final ProfileTones tones;

  /// How round the outer corners and the fillets at each step are, at most: less
  /// where a step is smaller than that.
  static const radius = 3.0;

  /// The least a step stands, so a crawl or a ferry still shows.
  static const _floor = 3.0;

  /// The steps for [runs] in [size], the runs that take no time left out. Its width
  /// is the runs' own total time, which differs a little from the route's.
  List<_Step> _steps(Size size) {
    final total = runs.fold(0.0, (sum, run) => sum + run.duration);
    if (total <= 0 || size.width <= 0) return const [];
    final steps = <_Step>[];
    var elapsed = 0.0;
    for (final run in runs) {
      if (run.duration <= 0) continue;
      final left = size.width * elapsed / total;
      elapsed += run.duration;
      final share = topSpeed > 0 ? (run.speed / topSpeed).clamp(0.0, 1.0) : 0.0;
      steps.add((
        left: left,
        right: size.width * elapsed / total,
        top: size.height - math.max(_floor, size.height * share),
        motorway: run.motorway,
      ));
    }
    return steps;
  }

  /// The outline of [steps]: up the start, along the top of each step with a fillet
  /// down or up to the next, and down the end.
  @visibleForTesting
  static Path outline(
    List<({double left, double right, double top, bool motorway})> steps,
    Size size,
  ) {
    final path = Path()..moveTo(0, size.height);
    double width(int i) => steps[i].right - steps[i].left;
    // The outer corner at the start.
    final first = steps.first;
    final start = math.min(
      radius,
      math.min(width(0) / 2, size.height - first.top),
    );
    path
      ..lineTo(0, first.top + start)
      ..quadraticBezierTo(0, first.top, start, first.top);
    for (var i = 1; i < steps.length; i++) {
      final from = steps[i - 1].top;
      final to = steps[i].top;
      final x = steps[i].left;
      final r = [
        radius,
        (to - from).abs() / 2,
        width(i - 1) / 2,
        width(i) / 2,
      ].reduce(math.min);
      final down = to > from ? 1.0 : -1.0;
      path
        ..lineTo(x - r, from)
        ..quadraticBezierTo(x, from, x, from + down * r)
        ..lineTo(x, to - down * r)
        ..quadraticBezierTo(x, to, x + r, to);
    }
    final last = steps.last;
    final end = math.min(
      radius,
      math.min(width(steps.length - 1) / 2, size.height - last.top),
    );
    return path
      ..lineTo(size.width - end, last.top)
      ..quadraticBezierTo(size.width, last.top, size.width, last.top + end)
      ..lineTo(size.width, size.height)
      ..close();
  }

  @override
  void paint(Canvas canvas, Size size) {
    final steps = _steps(size);
    if (steps.isEmpty) {
      // Nothing to make steps of: the plain bar.
      canvas.drawRRect(
        RRect.fromLTRBR(
          0,
          size.height - routeBarHeight,
          size.width,
          size.height,
          const Radius.circular(routeBarHeight / 2),
        ),
        Paint()..color = tones.other,
      );
      return;
    }
    canvas
      ..save()
      ..clipPath(outline(steps, size));
    // One rectangle for each stretch of a tone, so no seam shows within it.
    var from = 0;
    for (var i = 1; i <= steps.length; i++) {
      if (i < steps.length && steps[i].motorway == steps[from].motorway) {
        continue;
      }
      canvas.drawRect(
        Rect.fromLTRB(steps[from].left, 0, steps[i - 1].right, size.height),
        Paint()..color = steps[from].motorway ? tones.motorway : tones.other,
      );
      from = i;
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(SpeedProfilePainter oldDelegate) =>
      !identical(runs, oldDelegate.runs) ||
      topSpeed != oldDelegate.topSpeed ||
      tones != oldDelegate.tones;
}
