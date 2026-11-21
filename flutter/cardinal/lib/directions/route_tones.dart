/// The tones a route's graph is drawn in: one for each of the three kinds of way a
/// route of its mode can be on, best first, and what they're called in the legend.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

/// The tones of a route's graph, all of its primary color's hue but [ferry] and
/// [ground].
///
/// [best], [middle] and [worst] are the mode's best kind of way to its worst: freeway,
/// main road and street for a drive. On a light sheet they go from dark to light; on a
/// dark one from light to dark, so the best is the one that stands out either way.
/// Without the kinds of way, a drive's freeways are [best] and its other roads
/// [worst].
@immutable
class ProfileTones {
  const ProfileTones({
    required this.best,
    required this.middle,
    required this.worst,
    required this.ferry,
    required this.ground,
  });

  final Color best;
  final Color middle;
  final Color worst;

  /// A ferry crossing: a quiet grey, not one of the ways at all.
  final Color ferry;

  /// The silhouette of the ground a ride or a walk goes over.
  final Color ground;

  /// The tone of [rank], 0 for the best kind of way and 2 for the worst.
  Color ofRank(int rank) => switch (rank) {
    0 => best,
    1 => middle,
    _ => worst,
  };

  /// The tone [run] is drawn in. With [detailed], by its kind of way, or as a ferry;
  /// otherwise [best] for a motorway and [worst] for anything else.
  Color of(DirectionsRun run, {required bool detailed}) {
    if (!detailed) return run.motorway ? best : worst;
    final kind = run.kind;
    if (kind == null) return run.ferry ? ferry : worst;
    return ofRank(WayKinds.rankOf(kind));
  }

  @override
  bool operator ==(Object other) =>
      other is ProfileTones &&
      best == other.best &&
      middle == other.middle &&
      worst == other.worst &&
      ferry == other.ferry &&
      ground == other.ground;

  @override
  int get hashCode => Object.hash(best, middle, worst, ferry, ground);
}

/// The graph's tones for [scheme], well apart in lightness: 0.28, 0.48 and 0.68 on a
/// light sheet, 0.82, 0.62 and 0.44 on a dark one.
ProfileTones profileTones(ColorScheme scheme) {
  final primary = HSLColor.fromColor(scheme.primary);
  // Enough color to tell the tones from the greys around them, whatever the seed.
  final hue = primary.withSaturation(math.max(primary.saturation, 0.35));
  final dark = scheme.brightness == Brightness.dark;
  Color tone(double light, double dim) =>
      hue.withLightness(dark ? dim : light).toColor();
  return ProfileTones(
    best: tone(0.28, 0.82),
    middle: tone(0.48, 0.62),
    worst: tone(0.68, 0.44),
    ferry: scheme.onSurfaceVariant.withValues(alpha: 0.4),
    ground: scheme.onSurfaceVariant.withValues(alpha: 0.22),
  );
}

/// Black or white, whichever reads better on [background].
Color labelColorOn(Color background) {
  final luminance = background.computeLuminance();
  // Where white's contrast with it and black's are equal.
  return luminance > 0.179 ? Colors.black : Colors.white;
}

/// The three kinds of way the routes of a mode are among, best first, and what the
/// legend calls each.
enum WayKinds {
  driving(
    [
      DirectionsRunKind.freeway,
      DirectionsRunKind.mainRoad,
      DirectionsRunKind.street,
    ],
    ['Freeway', 'Main roads', 'Streets'],
  ),
  cycling(
    [
      DirectionsRunKind.trail,
      DirectionsRunKind.bikeLane,
      DirectionsRunKind.sharedRoad,
    ],
    ['Trail', 'Bike lane', 'Shared road'],
  ),
  walking(
    [
      DirectionsRunKind.path,
      DirectionsRunKind.sidewalk,
      DirectionsRunKind.stairs,
    ],
    ['Path', 'Sidewalk', 'Stairs'],
  );

  const WayKinds(this.kinds, this.names);

  /// The kinds, best first.
  final List<DirectionsRunKind> kinds;

  /// What the legend calls each of [kinds].
  final List<String> names;

  /// Which of its mode's three [kind] is: 0 for the best, 2 for the worst.
  static int rankOf(DirectionsRunKind kind) => switch (kind) {
    DirectionsRunKind.freeway ||
    DirectionsRunKind.trail ||
    DirectionsRunKind.path => 0,
    DirectionsRunKind.mainRoad ||
    DirectionsRunKind.bikeLane ||
    DirectionsRunKind.sidewalk => 1,
    DirectionsRunKind.street ||
    DirectionsRunKind.sharedRoad ||
    DirectionsRunKind.stairs => 2,
  };

  /// The mode [kind] is a kind of way for.
  static WayKinds ofKind(DirectionsRunKind kind) =>
      values.firstWhere((family) => family.kinds.contains(kind));

  /// The kinds [route]'s runs are among, or null if the router didn't say what any
  /// of its roads are.
  static WayKinds? of(DirectionsRoute route) {
    for (final run in route.runs) {
      final kind = run.kind;
      if (kind != null) return ofKind(kind);
    }
    return null;
  }

  /// The kinds the first of [routes] that says is among, or null if none does.
  static WayKinds? ofAll(List<DirectionsRoute> routes) {
    for (final route in routes) {
      final kinds = of(route);
      if (kinds != null) return kinds;
    }
    return null;
  }
}

/// Whether the router said what [runs]' roads are: then each run's kind of way, and
/// whether it's tolled, are true to the stretch itself.
bool hasWayKinds(List<DirectionsRun> runs) =>
    runs.any((run) => run.kind != null);
