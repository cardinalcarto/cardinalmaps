/// The graph on a driving route's row: how fast the router expects the route to go,
/// along its time.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import 'route_tones.dart';

/// How tall a route's graph is, in logical pixels, whatever it draws.
const double routeGraphHeight = 32;

/// How tall the plain bar is that stands for a route with no profile to draw.
const double routeBarHeight = 6;

/// What's written on a tolled stretch of a profile.
const String tollLabel = 'Toll';

/// The most "Toll" is scaled, as far as the tabs' labels are. Whether a stretch holds
/// it is judged at this scale, so that doesn't change with the text size.
const double tollLabelScale = 1.3;

/// Room around "Toll" within its stretch, on either side and above and below.
const EdgeInsets tollLabelPadding = EdgeInsets.symmetric(
  horizontal: 4,
  vertical: 1,
);

/// The style "Toll" is written in on a profile, at [context].
TextStyle tollLabelStyle(BuildContext context) => DefaultTextStyle.of(context)
    .style
    .merge(
      Theme.of(context).textTheme.labelSmall!
          .copyWith(fontWeight: FontWeight.w700),
    );

/// How much room "Toll" takes at [tollLabelScale], at [context].
Size tollLabelSize(BuildContext context) {
  final painter = TextPainter(
    text: TextSpan(text: tollLabel, style: tollLabelStyle(context)),
    textDirection: Directionality.of(context),
    textScaler: const TextScaler.linear(tollLabelScale),
    maxLines: 1,
  )..layout();
  final size = painter.size;
  painter.dispose();
  return size;
}

/// Where "Toll" is written on a profile: centered in [rect], on [run]'s tone.
typedef TollLabel = ({Rect rect, DirectionsRun run});

/// One step of a profile, as it's drawn.
typedef ProfileStep = ({double left, double right, double top, Color tone});

/// How far from the top of a profile [size] tall a run at [speed] stands, when
/// [topSpeed] stands full height.
double _stepTop(double speed, double topSpeed, Size size) {
  final share = topSpeed > 0 ? (speed / topSpeed).clamp(0.0, 1.0) : 0.0;
  return size.height - math.max(SpeedProfilePainter.floor, size.height * share);
}

/// The steps of a profile of [runs] in [size], the runs that take no time left out,
/// each in its tone from [tones]. Its width is the runs' own total time, which differs
/// a little from the route's.
List<ProfileStep> profileSteps(
  List<DirectionsRun> runs,
  double topSpeed,
  Size size,
  ProfileTones tones,
) {
  final total = runs.fold(0.0, (sum, run) => sum + run.duration);
  if (total <= 0 || size.width <= 0) return const [];
  final detailed = hasWayKinds(runs);
  final steps = <ProfileStep>[];
  var elapsed = 0.0;
  for (final run in runs) {
    if (run.duration <= 0) continue;
    final left = size.width * elapsed / total;
    elapsed += run.duration;
    steps.add((
      left: left,
      right: size.width * elapsed / total,
      top: _stepTop(run.speed, topSpeed, size),
      tone: tones.of(run, detailed: detailed),
    ));
  }
  return steps;
}

/// Where "Toll" goes on a profile of [runs] in [size], [label] being the room the
/// word takes at [tollLabelScale]: on each stretch of tolled runs, not ferries, that
/// holds it with [tollLabelPadding] around it, between the stretch's lowest top and
/// the bottom.
///
/// None unless the router said what the runs' roads are, since only then is a run
/// tolled just where the road is; otherwise a stretch could be drawn far longer than
/// the toll.
List<TollLabel> tollLabels(
  List<DirectionsRun> runs, {
  required double topSpeed,
  required Size size,
  required Size label,
}) {
  final total = runs.fold(0.0, (sum, run) => sum + run.duration);
  if (total <= 0 || size.width <= 0 || !hasWayKinds(runs)) return const [];
  final labels = <TollLabel>[];
  double x(double seconds) => size.width * seconds / total;

  void place(List<DirectionsRun> stretch, double from, double to) {
    if (stretch.isEmpty) return;
    final top = stretch
        .map((run) => _stepTop(run.speed, topSpeed, size))
        .reduce(math.max);
    final rect = Rect.fromLTRB(
      x(from) + tollLabelPadding.left,
      top + tollLabelPadding.top,
      x(to) - tollLabelPadding.right,
      size.height - tollLabelPadding.bottom,
    );
    if (rect.width < label.width || rect.height < label.height) return;
    // The run at the middle of the stretch, for the tone the word is written on.
    final middle = (from + to) / 2;
    var at = from;
    var under = stretch.first;
    for (final run in stretch) {
      under = run;
      at += run.duration;
      if (at >= middle) break;
    }
    labels.add((rect: rect, run: under));
  }

  var stretch = <DirectionsRun>[];
  var start = 0.0;
  var elapsed = 0.0;
  for (final run in runs) {
    if (run.duration <= 0) continue;
    if (run.toll && !run.ferry) {
      if (stretch.isEmpty) start = elapsed;
      stretch.add(run);
    } else {
      place(stretch, start, elapsed);
      stretch = [];
    }
    elapsed += run.duration;
  }
  place(stretch, start, elapsed);
  return labels;
}

/// A route's speed profile, as wide as it's given and [routeGraphHeight] tall: steps
/// whose height is the speed of each of [runs] and whose width is its time, each
/// filled in its kind of road's tone (with no kinds, the freeway's or the other's),
/// and "Toll" written where [tollLabels] puts it.
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
    this.tollLabels = const [],
  });

  final List<DirectionsRun> runs;
  final double topSpeed;
  final ProfileTones tones;

  /// Where "Toll" is written: [tollLabels] for the width the profile is given.
  final List<TollLabel> tollLabels;

  @override
  Widget build(BuildContext context) {
    final paint = CustomPaint(
      size: const Size(double.infinity, routeGraphHeight),
      painter: SpeedProfilePainter(
        runs: runs,
        topSpeed: topSpeed,
        tones: tones,
      ),
    );
    if (tollLabels.isEmpty) return RepaintBoundary(child: paint);
    final style = tollLabelStyle(context);
    final scaler = MediaQuery.textScalerOf(context)
        .clamp(maxScaleFactor: tollLabelScale);
    return RepaintBoundary(
      child: SizedBox(
        height: routeGraphHeight,
        child: Stack(
          children: [
            Positioned.fill(child: paint),
            for (final label in tollLabels)
              Positioned.fromRect(
                rect: label.rect,
                child: Center(
                  child: Text(
                    tollLabel,
                    style: style.copyWith(
                      color: labelColorOn(tones.of(label.run, detailed: true)),
                    ),
                    textScaler: scaler,
                    maxLines: 1,
                    softWrap: false,
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
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
  static const floor = 3.0;

  /// The outline of [steps]: up the start, along the top of each step with a fillet
  /// down or up to the next, and down the end.
  @visibleForTesting
  static Path outline(List<ProfileStep> steps, Size size) {
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
    final steps = profileSteps(runs, topSpeed, size, tones);
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
        Paint()..color = tones.worst,
      );
      return;
    }
    canvas
      ..save()
      ..clipPath(outline(steps, size));
    // One rectangle for each stretch of a tone, so no seam shows within it.
    var from = 0;
    for (var i = 1; i <= steps.length; i++) {
      if (i < steps.length && steps[i].tone == steps[from].tone) continue;
      canvas.drawRect(
        Rect.fromLTRB(steps[from].left, 0, steps[i - 1].right, size.height),
        Paint()..color = steps[from].tone,
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
