/// The graph on a ride's or a walk's row: the ground it goes over, along its time,
/// above a bar in the tones of the ways it takes.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import 'route_tones.dart';
import 'speed_profile.dart';

/// The heights that a list's elevation profiles span, in meters: [low] at the bottom
/// of each, [high] at the top.
typedef ElevationRange = ({double low, double high});

/// The least span of heights a profile is drawn to, in meters, so a flat route looks
/// flat rather than having its ripples stretched to the profile's full height.
const double elevationFloor = 50;

/// The heights the elevations of [routes] span, all to one scale so their hills
/// compare: from the lowest of any to the highest, and at least [elevationFloor].
/// Null if none of them has an elevation.
ElevationRange? elevationRangeOf(List<DirectionsRoute> routes) {
  var low = double.infinity;
  var high = double.negativeInfinity;
  for (final route in routes) {
    for (final height in route.elevation?.heights ?? const <double>[]) {
      low = math.min(low, height);
      high = math.max(high, height);
    }
  }
  if (low > high) return null;
  return (low: low, high: math.max(high, low + elevationFloor));
}

/// A ride's or a walk's graph, as wide as it's given and [routeGraphHeight] tall: the
/// silhouette of [elevation] along the route's time, drawn to [range], over a bar of
/// [runs] in their kinds of way's tones.
///
/// With no elevation the space above the bar stays empty. With no kinds of way the
/// bar is plain.
///
/// It's a layer of its own, painted once: nothing about the sheet moving paints it
/// again.
class ElevationProfile extends StatelessWidget {
  const ElevationProfile({
    super.key,
    required this.runs,
    required this.elevation,
    required this.distance,
    required this.range,
    required this.tones,
  });

  final List<DirectionsRun> runs;
  final DirectionsElevation? elevation;

  /// How far the route goes, in meters, which its last height is at.
  final double distance;

  /// The heights the list's profiles span, or null if none has an elevation.
  final ElevationRange? range;
  final ProfileTones tones;

  @override
  Widget build(BuildContext context) => RepaintBoundary(
    child: CustomPaint(
      size: const Size(double.infinity, routeGraphHeight),
      painter: ElevationProfilePainter(
        runs: runs,
        elevation: elevation,
        distance: distance,
        range: range,
        tones: tones,
      ),
    ),
  );
}

/// Paints an [ElevationProfile].
class ElevationProfilePainter extends CustomPainter {
  const ElevationProfilePainter({
    required this.runs,
    required this.elevation,
    required this.distance,
    required this.range,
    required this.tones,
  });

  final List<DirectionsRun> runs;
  final DirectionsElevation? elevation;
  final double distance;
  final ElevationRange? range;
  final ProfileTones tones;

  /// Between the silhouette and the bar.
  static const gap = 2.0;

  /// Between two stretches of the bar in different tones.
  static const seam = 1.0;

  /// The least the silhouette stands at the lowest height of the list, so it shows.
  static const stand = 2.0;

  /// How far apart the silhouette's points are, in logical pixels.
  static const spacing = 2.0;

  /// How tall the silhouette's space is, above the bar.
  static const bandHeight = routeGraphHeight - routeBarHeight - gap;

  /// The share of the route's distance gone at each of [count] + 1 points evenly
  /// apart along its time, from 0 at the start to 1 at the end. By distance alone if
  /// [runs] don't say how long each stretch takes.
  @visibleForTesting
  static List<double> distanceShares(List<DirectionsRun> runs, int count) {
    final time = runs.fold(0.0, (sum, run) => sum + run.duration);
    final length = runs.fold(0.0, (sum, run) => sum + run.distance);
    if (time <= 0 || length <= 0) {
      return [for (var i = 0; i <= count; i++) i / count];
    }
    final shares = <double>[];
    var run = 0;
    var timeBefore = 0.0;
    var distanceBefore = 0.0;
    for (var i = 0; i <= count; i++) {
      final at = time * i / count;
      while (run < runs.length - 1 && timeBefore + runs[run].duration < at) {
        timeBefore += runs[run].duration;
        distanceBefore += runs[run].distance;
        run++;
      }
      final current = runs[run];
      final into = current.duration > 0
          ? ((at - timeBefore) / current.duration).clamp(0.0, 1.0)
          : 1.0;
      shares.add(
        ((distanceBefore + into * current.distance) / length).clamp(0.0, 1.0),
      );
    }
    return shares;
  }

  /// The height of [elevation] at each of [shares] of a route [distance] long: the
  /// highest of the heights between it and its neighbors' midpoints, so a peak
  /// narrower than the points' spacing still shows.
  @visibleForTesting
  static List<double> resample(
    DirectionsElevation elevation,
    double distance,
    List<double> shares,
  ) {
    final heights = elevation.heights;
    final last = heights.length - 1;
    // Where along the route each height is: every interval, and the last at the end.
    final end = last <= 0
        ? 0.0
        : distance.clamp(
            (last - 1) * elevation.interval,
            last * elevation.interval,
          );
    double at(int k) => k == last ? end : k * elevation.interval;
    double heightAt(double meters) {
      if (last <= 0) return heights.isEmpty ? 0 : heights.first;
      final k = math.min((meters / elevation.interval).floor(), last - 1);
      final from = at(k);
      final span = at(k + 1) - from;
      final t = span > 0 ? ((meters - from) / span).clamp(0.0, 1.0) : 0.0;
      return heights[k] + (heights[k + 1] - heights[k]) * t;
    }

    final samples = <double>[];
    var k = 0;
    for (var i = 0; i < shares.length; i++) {
      final meters = shares[i] * end;
      final from = i == 0 ? meters : (shares[i - 1] + shares[i]) / 2 * end;
      final to = i == shares.length - 1
          ? meters
          : (shares[i] + shares[i + 1]) / 2 * end;
      var highest = heightAt(meters);
      while (k <= last && at(k) < from) {
        k++;
      }
      for (var j = k; j <= last && at(j) <= to; j++) {
        highest = math.max(highest, heights[j]);
      }
      samples.add(highest);
    }
    return samples;
  }

  /// The silhouette of [samples], evenly apart across [width], drawn to [range] in
  /// the [bandHeight] above the bar.
  @visibleForTesting
  static Path silhouette(
    List<double> samples,
    ElevationRange range,
    double width,
  ) {
    final span = range.high - range.low;
    double y(double height) {
      final share = span > 0
          ? ((height - range.low) / span).clamp(0.0, 1.0)
          : 0.0;
      return bandHeight - (stand + share * (bandHeight - stand));
    }

    final path = Path()..moveTo(0, bandHeight);
    final count = samples.length - 1;
    for (var i = 0; i <= count; i++) {
      path.lineTo(count > 0 ? width * i / count : 0, y(samples[i]));
    }
    return path
      ..lineTo(width, bandHeight)
      ..close();
  }

  /// The stretches of the bar for [runs] across [width], each a run of one tone, with
  /// a [seam] between stretches. One plain stretch if the runs have no kinds of way.
  @visibleForTesting
  static List<({double left, double right, Color tone})> barStretches(
    List<DirectionsRun> runs,
    double width,
    ProfileTones tones,
  ) {
    final time = runs.fold(0.0, (sum, run) => sum + run.duration);
    if (!hasWayKinds(runs) || time <= 0) {
      return [(left: 0, right: width, tone: tones.worst)];
    }
    final stretches = <({double left, double right, Color tone})>[];
    var elapsed = 0.0;
    for (final run in runs) {
      if (run.duration <= 0) continue;
      final left = width * elapsed / time;
      elapsed += run.duration;
      final right = width * elapsed / time;
      final tone = tones.of(run, detailed: true);
      if (stretches.isNotEmpty && stretches.last.tone == tone) {
        final last = stretches.removeLast();
        stretches.add((left: last.left, right: right, tone: tone));
      } else {
        stretches.add((left: left, right: right, tone: tone));
      }
    }
    return [
      for (var i = 0; i < stretches.length; i++)
        (
          left: stretches[i].left + (i > 0 ? seam / 2 : 0),
          right: stretches[i].right - (i < stretches.length - 1 ? seam / 2 : 0),
          tone: stretches[i].tone,
        ),
    ];
  }

  @override
  void paint(Canvas canvas, Size size) {
    if (size.width <= 0) return;
    final elevation = this.elevation;
    final range = this.range;
    if (elevation != null && range != null && elevation.heights.isNotEmpty) {
      final count = math.max(1, (size.width / spacing).ceil());
      final samples = resample(
        elevation,
        distance,
        distanceShares(runs, count),
      );
      canvas.drawPath(
        silhouette(samples, range, size.width),
        Paint()..color = tones.ground,
      );
    }
    final bar = Rect.fromLTRB(
      0,
      size.height - routeBarHeight,
      size.width,
      size.height,
    );
    canvas
      ..save()
      ..clipRRect(
        RRect.fromRectAndRadius(bar, const Radius.circular(routeBarHeight / 2)),
      );
    for (final stretch in barStretches(runs, size.width, tones)) {
      canvas.drawRect(
        Rect.fromLTRB(stretch.left, bar.top, stretch.right, bar.bottom),
        Paint()..color = stretch.tone,
      );
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(ElevationProfilePainter oldDelegate) =>
      !identical(runs, oldDelegate.runs) ||
      !identical(elevation, oldDelegate.elevation) ||
      distance != oldDelegate.distance ||
      range != oldDelegate.range ||
      tones != oldDelegate.tones;
}
