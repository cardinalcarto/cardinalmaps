/// How the camera follows the navigation chevron: the zoom and tilt for each way of
/// traveling, zooming out as you go faster, and where on screen the chevron sits.
library;

import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/painting.dart';
import 'package:routing/routing.dart';

/// How the camera follows the chevron while traveling one way.
///
/// The faster you go, the further ahead you need to see, so above a speed the camera
/// zooms out: a level for each doubling of speed, which keeps the same time's travel
/// ahead of the chevron on screen. Below it, the base zoom already shows more road than
/// that, and stays.
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

  /// The speed, in meters per second, above which the camera zooms out; infinite for
  /// a mode that never does.
  final double referenceSpeed;

  /// The furthest out speed takes the camera.
  final double minZoom;

  /// The zoom at [speed] meters per second, or [zoom] without one: above
  /// [referenceSpeed], out by log2(speed / [referenceSpeed]), so that speed × 2^zoom,
  /// and so how far ahead a second's travel reaches on screen, stays the same; never
  /// past [minZoom].
  double zoomAt(double? speed) {
    if (speed == null || !speed.isFinite || speed <= referenceSpeed) {
      return zoom;
    }
    final out = math.log(speed / referenceSpeed) / math.ln2;
    return math.max(minZoom, zoom - out);
  }
}

/// How far, in logical pixels, the chevron trails the end of the traveled line at
/// [speed] meters per second and [zoom].
///
/// The chevron glides to each fix over the second until the next, so it trails the fix
/// by a second's travel, while the traveled line ends at the fix; the chevron's disk
/// covers the stretch between, or the line shows the wrong color just behind it. In the
/// follow framing at 412 × 915 (pitch 60, the chevron 260 px below the top of the
/// padded view), Sparkle's puck measured that lag at about 1.45 × speed × 2^(zoom − 16)
/// pixels of the disk, which lies flat on the map.
///
/// The zoom is chosen for the road ahead, not for this: zooming out only ever shrinks
/// the lag, so it never makes the line's end show.
double chevronLag(double speed, double zoom) =>
    1.45 * speed * math.pow(2, zoom - 16);

/// The most [chevronLag] the disk hides: the line's end stays covered while the lag
/// plus half the line's 8 px width is at most the disk's radius, 40 by default.
const chevronLagLimit = 40.0 - 8 / 2;

/// How the camera follows each way of traveling. The base zooms and tilts are
/// Ferrostar's (`NavigationCamera.kt:17-26`, the same on iOS): driving z16 at 45°,
/// cycling z18 at 45°, walking z20 at 10°. Ferrostar doesn't zoom by speed; the app
/// does so you see far enough ahead.
FollowCamera followCamera(NavigationMode mode) => switch (mode) {
  NavigationMode.driving => _driving,
  NavigationMode.cycling => _cycling,
  NavigationMode.walking => _walking,
};

/// Holds z16 through town, up to 30 km/h, then a level out for each doubling of speed:
/// about z14.3 at 100 km/h, and z14 from 120 km/h. Each level out shows twice as far
/// ahead, so the road ahead takes about as long to reach at any speed above 30 km/h,
/// and at highway speeds the next exit comes into view well before you're on it. The
/// lag stays about 12 px above 30 km/h, and at z14 is only 13 px at 130 km/h.
const _driving = FollowCamera(
  zoom: 16,
  pitch: 45,
  referenceSpeed: 30 / 3.6,
  minZoom: 14,
);

/// Holds z18 up to 20 km/h, then eases out to z17 by 40 km/h: a fast descent shows a
/// little more of the street ahead without losing the detail of paths and lanes that
/// cycling needs. The lag reaches 32 px at 20 km/h and stays there to 40 km/h; past
/// about 45 km/h it outgrows the disk.
const _cycling = FollowCamera(
  zoom: 18,
  pitch: 45,
  referenceSpeed: 20 / 3.6,
  minZoom: 17,
);

/// Never zooms out: on foot the street around you matters more than what's ahead. The
/// lag outgrows the disk above about 5.6 km/h, a brisk walk.
const _walking = FollowCamera(
  zoom: 20,
  pitch: 10,
  referenceSpeed: double.infinity,
  minZoom: 20,
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
