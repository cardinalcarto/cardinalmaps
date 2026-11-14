/// How the camera follows the navigation chevron: the zoom and tilt for each way of
/// traveling, zooming out as you go faster, and where on screen the chevron sits.
library;

import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/painting.dart';
import 'package:routing/routing.dart';

/// How the camera follows the chevron while traveling one way.
@immutable
class FollowCamera {
  const FollowCamera({
    required this.zoom,
    required this.pitch,
    required this.referenceSpeed,
    required this.minZoom,
  });

  /// The zoom at and below [referenceSpeed].
  final double zoom;

  /// The tilt, in degrees from looking straight down.
  final double pitch;

  /// The speed, in meters per second, above which the camera zooms out.
  ///
  /// The chevron glides to each fix over the second until the next, so it trails the
  /// fix by a second's travel; the traveled line ends at the fix, and the chevron's disk
  /// must cover that stretch, or the line shows the wrong color just behind the
  /// chevron. In the follow framing at 412 × 915 (pitch 60, the chevron 260 px below
  /// the top of the padded view), Sparkle's puck measured that lag at about
  /// 1.45 × speed × 2^(zoom − 16) pixels of the disk, which lies flat on the map, speed
  /// in m/s. The line's end stays hidden while the lag plus half the line's 8 px width
  /// is at most the disk's radius, 40 by default: lag ≤ 36 px, so
  /// speed × 2^(zoom − 16) ≤ 36 / 1.45 ≈ 24.8 m/s. Each mode's reference speed is that
  /// at its base [zoom]: 24.8 / 2^([zoom] − 16).
  final double referenceSpeed;

  /// The furthest out speed takes the camera. Two levels out from [zoom], four times
  /// [referenceSpeed]: past what the mode itself reaches, so it only bounds the zoom when
  /// you travel by something faster than the trip's mode, such as walking directions
  /// followed from a bus.
  final double minZoom;

  /// The zoom at [speed] meters per second, or [zoom] without one: above
  /// [referenceSpeed], out by log2(speed / [referenceSpeed]), so that speed × 2^zoom,
  /// and so the chevron's lag on screen, stays the same; never past [minZoom].
  double zoomAt(double? speed) {
    if (speed == null || !speed.isFinite || speed <= referenceSpeed) {
      return zoom;
    }
    final out = math.log(speed / referenceSpeed) / math.ln2;
    return math.max(minZoom, zoom - out);
  }
}

/// The speed times 2^(zoom − 16), in meters per second, at which the chevron's lag on
/// screen just stays under its disk: see [FollowCamera.referenceSpeed].
const lagLimitSpeedAtZoom16 = 36 / 1.45;

/// How the camera follows each way of traveling. The base zooms and tilts are
/// Ferrostar's (`NavigationCamera.kt:17-26`, the same on iOS): driving z16 at 45°,
/// cycling z18 at 45°, walking z20 at 10°.
FollowCamera followCamera(NavigationMode mode) => switch (mode) {
  NavigationMode.driving => _driving,
  NavigationMode.cycling => _cycling,
  NavigationMode.walking => _walking,
};

/// About 24.8 m/s, or 89 km/h.
const _driving = FollowCamera(
  zoom: 16,
  pitch: 45,
  referenceSpeed: lagLimitSpeedAtZoom16,
  minZoom: 14,
);

/// About 6.2 m/s, or 22 km/h.
const _cycling = FollowCamera(
  zoom: 18,
  pitch: 45,
  referenceSpeed: lagLimitSpeedAtZoom16 / 4,
  minZoom: 16,
);

/// About 1.55 m/s, or 5.6 km/h.
const _walking = FollowCamera(
  zoom: 20,
  pitch: 10,
  referenceSpeed: lagLimitSpeedAtZoom16 / 16,
  minZoom: 18,
);

/// The least distance, in logical pixels, between the chevron and the bottom of what
/// the system's bars leave of the screen: Ferrostar's 24 dp
/// (`NavigationCamera.kt:102-125`).
const _chevronClearance = 24.0;

/// The padding [SparkleMapController.follow] takes, on top of the map's own [mapPadding],
/// to put the chevron where Ferrostar's Android camera does (spec §3, "Puck position",
/// `NavigationCamera.kt:87-125`), in a map of [size] with the system's bars over
/// [systemPadding]:
///
/// - the top half of the screen is padded off, so the chevron sits three quarters of
///   the way down, with the road ahead of it in view;
/// - in landscape (wider than tall), the start half is too, which puts it in the end
///   half, clear of the instructions and trip progress down the start side;
/// - and the bottom is padded enough to keep it at least 24 px above the system's bars.
EdgeInsets followPadding({
  required Size size,
  required EdgeInsets mapPadding,
  required EdgeInsets systemPadding,
  required TextDirection direction,
}) {
  final top = size.height / 2;
  final landscape = size.width > size.height;
  // The chevron is halfway between the top padding and the bottom one.
  final below = (size.height - top) / 2;
  final needed = _chevronClearance + systemPadding.bottom;
  final bottom = below < needed ? 2 * (needed - below) : 0.0;
  final start = landscape ? size.width / 2 : 0.0;
  final full = EdgeInsetsDirectional.only(
    top: top,
    bottom: bottom,
    start: start,
  ).resolve(direction);
  // The map's padding is already taken off.
  return EdgeInsets.fromLTRB(
    math.max(0, full.left - mapPadding.left),
    math.max(0, full.top - mapPadding.top),
    math.max(0, full.right - mapPadding.right),
    math.max(0, full.bottom - mapPadding.bottom),
  );
}

/// The longest the chevron glides from one fix to the next. Fixes come a second apart,
/// and one a little late still glides all the way; after a longer gap, such as a
/// tunnel, the chevron takes no longer than this to reach the fix, rather than crawl
/// there for as long as the gap was, trailing it all the while.
const maxGlide = Duration(milliseconds: 1500);

/// How long the chevron glides to a fix that came [sincePrevious] after the one before:
/// as long as the fixes are apart, up to [maxGlide], so it reaches each fix as the next
/// comes. The first fix, with none before it, is taken at once.
Duration glideDuration(Duration? sincePrevious) {
  if (sincePrevious == null || sincePrevious.isNegative) return Duration.zero;
  return sincePrevious > maxGlide ? maxGlide : sincePrevious;
}
