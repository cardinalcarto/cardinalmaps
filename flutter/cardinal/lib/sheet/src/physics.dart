part of '../sheet.dart';

/// How the sheet picks an anchor when let go, and how it gets there.
@immutable
class SheetPhysics {
  const SheetPhysics({
    this.deceleration = 4000,
    this.gravity = 1200,
    this.minFlingVelocity = 250,
    this.overdragResistance = 0.35,
    this.spring = const SpringDescription(mass: 1, stiffness: 700, damping: 47.6),
  });

  /// How fast a fling slows, in logical pixels per second squared: how far a fling
  /// would carry the sheet if nothing caught it.
  final double deceleration;

  /// The gravity of anchors that don't set their own (see [SheetAnchor.gravity]).
  final double gravity;

  /// Releases slower than this, in logical pixels per second, settle on the nearest
  /// anchor rather than counting as a fling.
  final double minFlingVelocity;

  /// How far the sheet follows a finger dragging it below its lowest anchor, as a
  /// fraction of the finger's travel.
  final double overdragResistance;

  /// The spring the sheet settles on, starting at the speed it was let go at. By default
  /// Material 3's standard spatial spring, with a damping ratio of 0.9: quick to arrive,
  /// with no long drift at the end and next to no overshoot.
  final SpringDescription spring;

  /// The anchor the sheet comes to rest at when let go at [extent] moving at
  /// [velocity] (logical pixels per second, positive upwards), as an index into
  /// [anchors] (sorted lowest first, and not empty).
  int target(List<ResolvedAnchor> anchors, double extent, double velocity) {
    assert(anchors.isNotEmpty);
    int nearestTo(double position) {
      var best = 0;
      for (var i = 1; i < anchors.length; i++) {
        if ((anchors[i].extent - position).abs() < (anchors[best].extent - position).abs()) {
          best = i;
        }
      }
      return best;
    }

    if (velocity.abs() < minFlingVelocity) return nearestTo(extent);

    // The anchors ahead of the fling, nearest first.
    final direction = velocity.sign;
    final ahead = [
      for (var i = 0; i < anchors.length; i++)
        if ((anchors[i].extent - extent) * direction > _epsilon) i,
    ];
    if (direction < 0) ahead.setAll(0, ahead.reversed.toList());
    if (ahead.isEmpty) return nearestTo(extent);

    // How far the fling would carry the sheet if nothing caught it.
    final speed2 = velocity * velocity;
    final reach = speed2 / (2 * deceleration);
    final landing = extent + direction * reach;
    for (var i = 0; i < ahead.length; i++) {
      final anchor = anchors[ahead[i]];
      final distance = (anchor.extent - extent).abs();
      if (distance >= reach) {
        // The fling runs out before this anchor: it always gets as far as the first,
        // and otherwise stops at whichever side of where it runs out is nearer.
        if (i == 0) return ahead[0];
        final previous = anchors[ahead[i - 1]];
        return (previous.extent - landing).abs() <= (anchor.extent - landing).abs()
            ? ahead[i - 1]
            : ahead[i];
      }
      final speedThere = math.sqrt(speed2 - 2 * deceleration * distance);
      if (speedThere < anchor.gravity) return ahead[i];
    }
    return ahead.last;
  }

  @override
  bool operator ==(Object other) =>
      other is SheetPhysics &&
      other.deceleration == deceleration &&
      other.gravity == gravity &&
      other.minFlingVelocity == minFlingVelocity &&
      other.overdragResistance == overdragResistance &&
      other.spring.mass == spring.mass &&
      other.spring.stiffness == spring.stiffness &&
      other.spring.damping == spring.damping;

  @override
  int get hashCode => Object.hash(
    deceleration,
    gravity,
    minFlingVelocity,
    overdragResistance,
    spring.mass,
    spring.stiffness,
    spring.damping,
  );
}

/// How close, in logical pixels, counts as at an anchor.
const double _epsilon = 0.5;
