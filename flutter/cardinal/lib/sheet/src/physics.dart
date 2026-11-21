part of '../sheet.dart';

/// How the sheet picks an anchor when let go, and how it gets there.
@immutable
class SheetPhysics {
  const SheetPhysics({
    this.platform,
    this.gravity = 4800,
    this.minFlingVelocity = 250,
    this.overdragResistance = 0.35,
    this.spring = const SpringDescription(
      mass: 1,
      stiffness: 700,
      damping: 47.6,
    ),
  });

  /// Whose scrolling a fling of the sheet slows as (see [coast]); null follows
  /// [defaultTargetPlatform].
  final TargetPlatform? platform;

  /// The gravity of anchors that don't set their own (see [SheetAnchor.gravity]).
  final double gravity;

  /// Releases slower than this, in logical pixels per second, settle on the nearest
  /// anchor rather than counting as a fling.
  final double minFlingVelocity;

  /// How far the sheet follows a finger dragging it below its lowest anchor, as a
  /// fraction of the finger's travel.
  final double overdragResistance;

  /// The spring the sheet settles on. By default Material 3's standard spatial spring,
  /// with a damping ratio of 0.9: quick to arrive, with no long drift at the end.
  ///
  /// A fling that would carry the sheet past the anchor it settles on coasts there first
  /// (see [coast]), and the spring takes over at the anchor with the speed it has left:
  /// it runs on past it as far as that momentum takes it, and comes back. A slower
  /// release springs from where it was let go, and eases in.
  final SpringDescription spring;

  /// How a fling slows the sheet from [extent] at [velocity] (logical pixels per second,
  /// positive upwards), as the platform's scrolling slows a fling of content: so a fling
  /// of the sheet feels like one of its content, and one carries on into the other (see
  /// [SheetRange.carriesIntoScroll]) at the same speed.
  ///
  /// On iOS and macOS, speed decays exponentially, as `UIScrollView`'s normal
  /// deceleration rate, 0.998 a millisecond, has it; elsewhere, as Android's spline-based
  /// fling. Both are what [BouncingScrollPhysics] and [ClampingScrollPhysics] use.
  Simulation coast(double extent, double velocity) =>
      switch (platform ?? defaultTargetPlatform) {
        TargetPlatform.iOS || TargetPlatform.macOS => FrictionSimulation(
          0.135,
          extent,
          velocity,
          tolerance: _coastTolerance,
        ),
        _ => ClampingScrollSimulation(
          position: extent,
          velocity: velocity,
          tolerance: _coastTolerance,
        ),
      };

  /// Where a fling from [extent] at [velocity] would carry the sheet if nothing caught it.
  double landing(double extent, double velocity) {
    if (velocity == 0) return extent;
    final coast = this.coast(extent, velocity);
    return coast is FrictionSimulation
        ? coast.finalX
        : coast.x(double.infinity);
  }

  /// How fast a fling from [extent] at [velocity] is still going when it reaches [at],
  /// which must be within its reach (see [landing]).
  double speedAt(double extent, double velocity, double at) =>
      coast(extent, velocity).dx(_timeAt(extent, velocity, at)).abs();

  /// When a fling from [extent] at [velocity] reaches [at], in seconds.
  double _timeAt(double extent, double velocity, double at) {
    final coast = this.coast(extent, velocity);
    final direction = velocity.sign;
    bool reached(double t) => (coast.x(t) - at) * direction >= 0;
    var hi = 1 / 64;
    while (!reached(hi) && hi < 64) {
      hi *= 2;
    }
    var lo = 0.0;
    for (var i = 0; i < 40; i++) {
      final mid = (lo + hi) / 2;
      if (reached(mid)) {
        hi = mid;
      } else {
        lo = mid;
      }
    }
    return hi;
  }

  /// The anchor the sheet comes to rest at when let go at [extent] moving at
  /// [velocity] (logical pixels per second, positive upwards), as an index into
  /// [anchors] (sorted lowest first, and not empty).
  int target(List<ResolvedAnchor> anchors, double extent, double velocity) {
    assert(anchors.isNotEmpty);
    int nearestTo(double position) {
      var best = 0;
      for (var i = 1; i < anchors.length; i++) {
        if ((anchors[i].extent - position).abs() <
            (anchors[best].extent - position).abs()) {
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
    final landing = this.landing(extent, velocity);
    final reach = (landing - extent).abs();
    for (var i = 0; i < ahead.length; i++) {
      final anchor = anchors[ahead[i]];
      final distance = (anchor.extent - extent).abs();
      if (distance >= reach) {
        // The fling runs out before this anchor: it always gets as far as the first,
        // and otherwise stops at whichever side of where it runs out is nearer.
        if (i == 0) return ahead[0];
        final previous = anchors[ahead[i - 1]];
        return (previous.extent - landing).abs() <=
                (anchor.extent - landing).abs()
            ? ahead[i - 1]
            : ahead[i];
      }
      if (speedAt(extent, velocity, anchor.extent) < anchor.gravity) {
        return ahead[i];
      }
    }
    return ahead.last;
  }

  /// Where the sheet glides to when let go at [extent] moving at [velocity] (logical
  /// pixels per second, positive upwards) within one of [ranges] (see [SheetRange]), or
  /// null if it springs to the anchor [target] picks among [anchors] instead.
  ///
  /// A slow release stays where it is. A fling glides on, slowing as [coast] has it,
  /// and stops where it runs out of speed, so long as that's within the same range. A
  /// fling that would carry it out of the range is for the anchors to catch, if there
  /// are any at or beyond that end; otherwise it stops at the end.
  double? glide(
    List<ResolvedAnchor> anchors,
    List<ResolvedRange> ranges,
    double extent,
    double velocity,
  ) {
    if (velocity.abs() < minFlingVelocity) {
      final range = ranges.firstWhereOrNull((r) => r.contains(extent));
      return range == null ? null : extent.clamp(range.lower, range.upper);
    }
    final direction = velocity.sign;
    final landing = this.landing(extent, velocity);
    final within = ranges.firstWhereOrNull(
      (r) => r.contains(extent) && r.contains(landing),
    );
    if (within != null) return landing.clamp(within.lower, within.upper);
    // Out past an end: stop there, unless an anchor waits at or beyond it.
    final range = ranges.firstWhereOrNull((r) => r.contains(extent));
    if (range == null) return null;
    final end = direction > 0 ? range.upper : range.lower;
    final caught = anchors.any((a) => (a.extent - end) * direction > -_epsilon);
    return caught ? null : end;
  }

  @override
  bool operator ==(Object other) =>
      other is SheetPhysics &&
      other.platform == platform &&
      other.gravity == gravity &&
      other.minFlingVelocity == minFlingVelocity &&
      other.overdragResistance == overdragResistance &&
      other.spring.mass == spring.mass &&
      other.spring.stiffness == spring.stiffness &&
      other.spring.damping == spring.damping;

  @override
  int get hashCode => Object.hash(
    platform,
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

/// Coasting stops once it's this slow, in logical pixels per second, short of where it
/// would end by a few pixels: as scrolling stops, about a physical pixel a frame.
const Tolerance _coastTolerance = Tolerance(distance: 0.5, velocity: 10);
