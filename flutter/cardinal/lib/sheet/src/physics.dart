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
  ///
  /// Only the anchors are weighed here. A page with ranges (see [SheetRange]) is
  /// decided by [decide], which glides a fling on into a range when every anchor up
  /// to the range's end lets it past, where this would stop it at the last of them.
  int target(List<ResolvedAnchor> anchors, double extent, double velocity) {
    assert(anchors.isNotEmpty);
    final caught = _catch(anchors, extent, velocity);
    return anchors.indexOf(caught.anchor!);
  }

  /// Where the anchors alone take a release: the nearest for a slow one, and for a
  /// fling the first ahead that catches it.
  SheetDecision _catch(
    List<ResolvedAnchor> anchors,
    double extent,
    double velocity,
  ) {
    ResolvedAnchor nearestTo(double position) {
      var best = anchors.first;
      for (final anchor in anchors.skip(1)) {
        if ((anchor.extent - position).abs() < (best.extent - position).abs()) {
          best = anchor;
        }
      }
      return best;
    }

    // How far the fling would carry the sheet if nothing caught it.
    final landing = this.landing(extent, velocity);
    if (velocity.abs() < minFlingVelocity) {
      return SheetDecision.anchor(
        SheetRule.slowToNearest,
        nearestTo(extent),
        landing: landing,
      );
    }

    final ahead = _ahead(anchors, extent, velocity.sign);
    if (ahead.isEmpty) {
      return SheetDecision.anchor(
        SheetRule.noAnchorAhead,
        nearestTo(extent),
        landing: landing,
      );
    }

    final reach = (landing - extent).abs();
    double? arrival;
    for (var i = 0; i < ahead.length; i++) {
      final anchor = ahead[i];
      final distance = (anchor.extent - extent).abs();
      if (distance >= reach) {
        // The fling runs out before this anchor: it always gets as far as the first,
        // and otherwise stops at whichever side of where it runs out is nearer.
        final previous = i == 0 ? null : ahead[i - 1];
        return SheetDecision.anchor(
          SheetRule.ranOutBeforeAnchor,
          previous != null &&
                  (previous.extent - landing).abs() <=
                      (anchor.extent - landing).abs()
              ? previous
              : anchor,
          landing: landing,
        );
      }
      arrival = speedAt(extent, velocity, anchor.extent);
      if (arrival < anchor.gravity) {
        return SheetDecision.anchor(
          SheetRule.caughtAtAnchor,
          anchor,
          landing: landing,
          arrival: arrival,
        );
      }
    }
    return SheetDecision.anchor(
      SheetRule.passedAllAnchors,
      ahead.last,
      landing: landing,
      arrival: arrival,
    );
  }

  /// The anchors ahead of a fling from [extent] in [direction] (1 up, -1 down),
  /// nearest first.
  List<ResolvedAnchor> _ahead(
    List<ResolvedAnchor> anchors,
    double extent,
    double direction,
  ) {
    final ahead = [
      for (final anchor in anchors)
        if ((anchor.extent - extent) * direction > _epsilon) anchor,
    ];
    return direction < 0 ? ahead.reversed.toList() : ahead;
  }

  /// Where the sheet glides to when let go at [extent] moving at [velocity] (logical
  /// pixels per second, positive upwards) within one of [ranges] or flung into one
  /// (see [SheetRange]), or null if it springs to the anchor [target] picks among
  /// [anchors] instead.
  ///
  /// A slow release within a range stays where it is. A fling within one glides on,
  /// slowing as [coast] has it, and stops where it runs out of speed, so long as
  /// that's within the same range. A fling that would carry it out of the range is for
  /// the anchors to catch, if there are any at or beyond that end; otherwise it stops
  /// at the end.
  ///
  /// A fling from outside every range, towards one, glides on into it if nothing
  /// catches it first. Each anchor on the way, up to and including one at the end of
  /// the range it comes to, catches it as [target] has it: by its gravity, or because
  /// the fling runs out before it. An end with no anchor at it has nothing to catch
  /// with. Past them all, the fling stops in the range where it runs out of speed, or
  /// at the range's far end if it would carry beyond that.
  double? glide(
    List<ResolvedAnchor> anchors,
    List<ResolvedRange> ranges,
    double extent,
    double velocity,
  ) {
    final decision = decide(anchors, ranges, extent, velocity);
    return decision.range == null ? null : decision.extent;
  }

  /// Where the sheet comes to rest when let go at [extent] moving at [velocity]
  /// (logical pixels per second, positive upwards) among [anchors] (sorted lowest
  /// first, and not empty) and [ranges], and the rule that decided: [glide] and
  /// [target] in one.
  SheetDecision decide(
    List<ResolvedAnchor> anchors,
    List<ResolvedRange> ranges,
    double extent,
    double velocity,
  ) {
    assert(anchors.isNotEmpty);
    final landing = this.landing(extent, velocity);
    final inside = ranges.firstWhereOrNull((r) => r.contains(extent));
    if (velocity.abs() < minFlingVelocity) {
      if (inside == null) return _catch(anchors, extent, velocity);
      return SheetDecision.range(
        SheetRule.slowInRange,
        inside,
        extent.clamp(inside.lower, inside.upper),
        landing: landing,
      );
    }
    final direction = velocity.sign;
    if (inside != null) {
      final within = ranges.firstWhereOrNull(
        (r) => r.contains(extent) && r.contains(landing),
      );
      if (within != null) {
        return SheetDecision.range(
          SheetRule.glideWithinRange,
          within,
          landing.clamp(within.lower, within.upper),
          landing: landing,
        );
      }
      // Out past an end: stop there, unless an anchor waits at or beyond it.
      final end = direction > 0 ? inside.upper : inside.lower;
      final caught = anchors.any(
        (a) => (a.extent - end) * direction > -_epsilon,
      );
      if (caught) return _catch(anchors, extent, velocity);
      return SheetDecision.range(
        SheetRule.rangeEnd,
        inside,
        end,
        landing: landing,
      );
    }

    // Outside every range: into the next one ahead, if the fling gets past its near
    // end and every anchor up to there.
    double nearEnd(ResolvedRange range) =>
        direction > 0 ? range.lower : range.upper;
    ResolvedRange? next;
    for (final range in ranges) {
      final distance = (nearEnd(range) - extent) * direction;
      if (distance <= 0) continue;
      if (next == null || distance < (nearEnd(next) - extent) * direction) {
        next = range;
      }
    }
    if (next == null) return _catch(anchors, extent, velocity);
    final near = nearEnd(next);
    if ((landing - near) * direction <= _epsilon) {
      return _catch(anchors, extent, velocity);
    }
    for (final anchor in _ahead(anchors, extent, direction)) {
      if ((anchor.extent - near) * direction > _epsilon) break;
      if (speedAt(extent, velocity, anchor.extent) < anchor.gravity) {
        return _catch(anchors, extent, velocity);
      }
    }
    return SheetDecision.range(
      SheetRule.glideIntoRange,
      next,
      landing.clamp(next.lower, next.upper),
      landing: landing,
      arrival: speedAt(extent, velocity, near),
    );
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

/// The rule that decides where a release takes the sheet (see [SheetPhysics.decide]).
enum SheetRule {
  /// A slow release outside every range: the nearest anchor.
  slowToNearest,

  /// A slow release within a range: where it was let go.
  slowInRange,

  /// A fling within a range that runs out of speed within it: where it runs out.
  glideWithinRange,

  /// A fling from outside every range that nothing catches up to and including the
  /// near end of the range ahead: where it runs out of speed in that range, or the
  /// range's far end.
  glideIntoRange,

  /// A fling out of a range past an end with no anchor at or beyond it: that end.
  rangeEnd,

  /// A fling with no anchor ahead of it: the nearest anchor.
  noAnchorAhead,

  /// A fling that runs out of speed before an anchor: that anchor, if it's the first
  /// ahead, or else whichever of it and the one before is nearer to where it runs out.
  ranOutBeforeAnchor,

  /// A fling that reaches an anchor slower than the anchor's gravity: that anchor.
  caughtAtAnchor,

  /// A fling that reaches every anchor ahead at its gravity or faster: the last.
  passedAllAnchors,
}

/// Where a release takes the sheet, and the rule that decided (see
/// [SheetPhysics.decide]).
@immutable
class SheetDecision {
  /// Rests at [anchor].
  const SheetDecision.anchor(
    this.rule,
    ResolvedAnchor this.anchor, {
    required this.landing,
    this.arrival,
  }) : range = null,
       _extent = null;

  /// Rests [extent] up within [range].
  const SheetDecision.range(
    this.rule,
    ResolvedRange this.range,
    double extent, {
    required this.landing,
    this.arrival,
  }) : anchor = null,
       _extent = extent;

  final SheetRule rule;

  /// The anchor the sheet rests at, or null if it rests within [range].
  final ResolvedAnchor? anchor;

  /// The range the sheet rests within, or null if it rests at [anchor].
  final ResolvedRange? range;

  /// Where the sheet rests, in logical pixels up.
  double get extent => anchor?.extent ?? _extent!;
  final double? _extent;

  /// Where the release would have carried the sheet if nothing caught it (see
  /// [SheetPhysics.landing]).
  final double landing;

  /// How fast, in logical pixels per second, the fling is still going where the rule
  /// weighed it: at the anchor that caught it, at the last anchor it passed, or at the
  /// near end of the range it glides into. Null where no speed was weighed.
  final double? arrival;
}
