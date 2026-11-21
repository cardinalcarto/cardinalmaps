part of '../sheet.dart';

enum _Phase {
  /// At rest, or not yet placed.
  idle,

  /// Following a finger (or a mouse wheel).
  held,

  /// Springing to an anchor.
  settling,

  /// Gliding to a stop within a range (see [SheetRange]).
  gliding,

  /// Coasting with a fling to the anchor it's caught at, for the spring to take over
  /// there.
  coasting,

  /// Following a page transition.
  transition,
}

/// Where the sheet is, and the way to move it.
///
/// Its value is the sheet's extent: how far it's pulled up, in logical pixels from the
/// bottom of the screen. Listeners hear about every change, every frame the sheet moves.
class SheetController extends ChangeNotifier
    implements ValueListenable<double> {
  SheetController({
    required TickerProvider vsync,
    this._physics = const SheetPhysics(),
  }) : _spring = AnimationController.unbounded(vsync: vsync) {
    _spring.addListener(_handleSpringTick);
  }

  final AnimationController _spring;
  Timer? _wheelSettle;

  SheetPhysics get physics => _physics;
  SheetPhysics _physics;
  set physics(SheetPhysics value) {
    if (value == _physics) return;
    _physics = value;
    // Gravity may have changed; the scaffold places the anchors again.
    notifyListeners();
  }

  _Phase _phase = _Phase.idle;

  @override
  double get value => extent;

  /// How far the sheet is pulled up, in logical pixels.
  double get extent => _extent ?? 0;
  double? _extent;

  /// Whether the sheet has been placed yet.
  bool get hasExtent => _extent != null;

  /// The current page's anchors, lowest first.
  List<ResolvedAnchor> get anchors => _anchors;
  List<ResolvedAnchor> _anchors = const [];

  /// The current page's ranges (see [SheetRange]), lowest first.
  List<ResolvedRange> get ranges => _ranges;
  List<ResolvedRange> _ranges = const [];

  /// The anchor the sheet is resting at, or null while it's moving or resting within a
  /// range.
  SheetAnchor? get restingAt => switch (_rest) {
    _AtAnchor(:final anchor) => anchor,
    _ => null,
  };

  /// The range the sheet is resting within, away from its ends, or null while it's
  /// moving or resting at an anchor.
  SheetRange? get restingIn => switch (_rest) {
    _InRange(:final range) => range,
    _ => null,
  };

  /// Whether the sheet is at rest, at an anchor or within a range.
  bool get isResting => _rest != null;

  _Rest? get _rest => _resting;
  _Rest? _resting;
  set _rest(_Rest? value) {
    _resting = value;
    _collapsed.value = value is _AtAnchor && value.anchor is _CollapsedAnchor;
  }

  /// Whether the sheet is resting out of sight at [SheetAnchor.collapsed]. It stops
  /// being collapsed as soon as it starts moving again.
  bool get isCollapsed => _collapsed.value;

  /// [isCollapsed], to listen to.
  ValueListenable<bool> get collapsed => _collapsed;
  final _collapsed = ValueNotifier<bool>(false);

  /// The anchor the current page's sheet first moves to (see
  /// [SheetContent.initialAnchor]), for [expand].
  SheetAnchor? _initial;

  /// Brings a collapsed sheet back up to the current page's initial anchor. Does
  /// nothing unless it's collapsed.
  void expand() {
    if (!isCollapsed) return;
    final initial = _initial;
    if (initial != null) animateTo(initial);
  }

  /// The furthest the sheet can be pulled up at all.
  double _maxExtent = double.infinity;

  /// The lowest the sheet goes, even pulled past its lowest anchor: the keyboard's top.
  double _floor = 0;

  /// The lowest the sheet rests: its lowest anchor, or the bottom of a range below it.
  double get _min {
    final range = _ranges.firstOrNull?.lower;
    if (_anchors.isEmpty) return range ?? 0;
    return range == null
        ? _anchors.first.extent
        : math.min(range, _anchors.first.extent);
  }

  /// The highest the sheet goes: its highest anchor, or the top of a range above it.
  double get _max {
    final range = _ranges.map((r) => r.upper).maxOrNull;
    if (_anchors.isEmpty) return range ?? _maxExtent;
    return range == null
        ? _anchors.last.extent
        : math.max(range, _anchors.last.extent);
  }

  /// Whether dragging moves the sheet: see [SheetContent.draggable].
  bool get isDraggable => _draggable;
  bool _draggable = true;

  /// Whether the sheet is as high as it goes, where dragging scrolls its content.
  bool get isAtMax => extent >= _max - _epsilon;

  /// Whether a finger (or a mouse wheel) is moving the sheet.
  bool get isHeld => _phase == _Phase.held;

  /// Whether a page transition is moving the sheet.
  bool get isTransitioning => _phase == _Phase.transition;

  /// Springs the sheet to [anchor], one of the current page's anchors or an end of one
  /// of its ranges, keeping whatever speed it has. If a page transition has the sheet,
  /// or the current page doesn't have that anchor yet, as when it's about to add it,
  /// once that's over.
  void animateTo(SheetAnchor anchor) {
    _awaited = null;
    final target = _placeOf(anchor);
    if (isTransitioning || target == null) {
      _awaited = anchor;
      return;
    }
    _springTo(
      target.$1,
      target.$2,
      _phase == _Phase.settling ? _spring.velocity : 0,
    );
  }

  /// The anchor to spring to as soon as the current page has it.
  SheetAnchor? _awaited;

  /// Where [anchor] is on the current page, as a rest and its extent: one of its anchors,
  /// or else an end of one of its ranges.
  (_Rest, double)? _placeOf(SheetAnchor? anchor) {
    if (anchor == null) return null;
    if (_anchors.firstWhereOrNull((a) => a.anchor == anchor) case final a?) {
      return (_AtAnchor(a.anchor), a.extent);
    }
    for (final range in _ranges) {
      if (range.range.from != anchor && range.range.to != anchor) continue;
      final end = range.extentOf(anchor);
      return (_InRange(range.range, range.fractionOf(end)), end);
    }
    return null;
  }

  /// Springs the sheet to the next stop up, an anchor or the top of a range, or back to
  /// the lowest from the highest.
  void cycle() {
    if (isTransitioning || _anchors.isEmpty || !_draggable) return;
    // Never down out of sight: that's for a drag.
    final stops = <(_Rest, double)>[
      for (final a in _anchors)
        if (a.anchor is! _CollapsedAnchor) (_AtAnchor(a.anchor), a.extent),
      for (final r in _ranges)
        if (!_anchors.any((a) => (a.extent - r.upper).abs() < _epsilon))
          (_InRange(r.range, 1), r.upper),
    ]..sort((a, b) => a.$2.compareTo(b.$2));
    final next =
        stops.firstWhereOrNull((s) => s.$2 > extent + _epsilon) ??
        stops.firstWhere((s) => s.$1 is _AtAnchor);
    _springTo(next.$1, next.$2, 0);
  }

  /// Stops the sheet where it is, for a finger to take hold of it.
  ///
  /// Only [release] with the same [holder] lets go of it, so whatever took hold last
  /// decides where it settles.
  void hold({Object? holder}) {
    if (isTransitioning || !_draggable) return;
    _awaited = null;
    _spring.stop();
    _wheelSettle?.cancel();
    _phase = _Phase.held;
    _holder = holder;
    _carryOn = null;
    _rest = null;
  }

  Object? _holder;

  /// Whether [holder] is holding the sheet.
  bool isHeldBy(Object? holder) => isHeld && _holder == holder;

  /// Moves a held sheet by [delta] logical pixels (positive upwards), as far as its
  /// anchors allow, and returns how much of [delta] it took: the rest is for the content
  /// to scroll by. Below the lowest anchor the sheet follows with resistance, down to
  /// the keyboard's top while it shows.
  double dragBy(double delta) {
    if (!isHeld || _anchors.isEmpty) return 0;
    var extent = this.extent;
    var rest = delta;
    final resistance = _physics.overdragResistance;
    if (rest < 0) {
      final free = math.max(0.0, extent - _min);
      final step = math.max(rest, -free);
      extent += step;
      rest -= step;
      if (rest < 0) {
        extent = math.max(math.min(_floor, extent), extent + rest * resistance);
      }
      rest = 0;
    } else {
      if (extent < _min) {
        final step = math.min(rest, (_min - extent) / resistance);
        extent += step * resistance;
        rest -= step;
      }
      final step = math.min(rest, math.max(0.0, _max - extent));
      extent += step;
      rest -= step;
    }
    _set(extent);
    return delta - rest;
  }

  /// Lets go of a sheet [holder] is holding, moving at [velocity] (logical pixels per second, positive
  /// upwards), springing it to the anchor [SheetPhysics.target] picks, or gliding within
  /// a range (see [SheetRange]).
  ///
  /// If the fling would carry the sheet past its highest anchor from within a range that
  /// [SheetRange.carriesIntoScroll], it glides there instead and calls [carryOn] with the
  /// speed it has left, for the content to scroll on with.
  void release(
    double velocity, {
    Object? holder,
    ValueChanged<double>? carryOn,
  }) {
    if (!isHeldBy(holder)) return;
    if (carryOn != null && _carryIntoScroll(velocity, carryOn)) return;
    _settle(velocity);
  }

  /// Called with the speed left when a glide reaches the highest anchor.
  ValueChanged<double>? _carryOn;

  /// Glides the sheet up to its highest anchor at [velocity], then hands what's left of
  /// its speed to [carryOn], if a range lets it and the fling would carry it that far.
  bool _carryIntoScroll(double velocity, ValueChanged<double> carryOn) {
    if (velocity < _physics.minFlingVelocity || _anchors.isEmpty) return false;
    final top = _max;
    final range = _ranges.firstWhereOrNull(
      (r) =>
          r.range.carriesIntoScroll &&
          r.contains(extent) &&
          (r.upper - top).abs() < _epsilon,
    );
    if (range == null) return false;
    if (_physics.landing(extent, velocity) <= top + _epsilon) {
      return false;
    }
    _wheelSettle?.cancel();
    _spring.stop();
    _settlingTo = null;
    _phase = _Phase.gliding;
    _rest = null;
    _glidingIn = range.range;
    _carryOn = carryOn;
    _spring.value = extent;
    _spring
        .animateWith(_physics.coast(extent, velocity))
        .then((_) => _endGlide());
    return true;
  }

  /// Moves the sheet by [delta] for a mouse wheel, settling once the wheel stops.
  void _nudge(double delta) {
    if (isTransitioning || _anchors.isEmpty) return;
    hold();
    _set((extent + delta).clamp(_min, _max));
    _wheelSettle = Timer(const Duration(milliseconds: 150), () {
      if (isHeld) _settle(0);
    });
  }

  void _settle(double velocity) {
    if (_anchors.isEmpty) {
      _phase = _Phase.idle;
      return;
    }
    // Pulled down past the bottom of a range, with no anchor from here up to it: back
    // up to it, as to an anchor there, unless an anchor below it (collapsed, say) is
    // nearer or catches the fling.
    final bottom = _ranges.firstOrNull;
    if (bottom != null &&
        extent < bottom.lower - _epsilon &&
        !_anchors.any(
          (a) =>
              a.extent > extent + _epsilon &&
              a.extent < bottom.lower + _epsilon,
        )) {
      final end = bottom.fromIsUpper ? bottom.range.to : bottom.range.from;
      final stops = [
        for (final a in _anchors)
          if (a.extent < bottom.lower - _epsilon) a,
        ResolvedAnchor(end, bottom.lower, end.gravity ?? _physics.gravity),
      ];
      final i = _physics.target(stops, extent, velocity);
      if (i == stops.length - 1) {
        _springTo(_InRange(bottom.range, 0), bottom.lower, velocity);
      } else {
        _catchOn(stops[i], velocity);
      }
      return;
    }
    if (_physics.glide(_anchors, _ranges, extent, velocity)
        case final landing?) {
      _glideTo(landing, velocity);
      return;
    }
    _catchOn(_anchors[_physics.target(_anchors, extent, velocity)], velocity);
  }

  /// Settles on [target], let go at [velocity]. A fling that would carry the sheet past
  /// it coasts there, and the spring takes over at the anchor with the speed it has
  /// left, running on past it as far as that momentum takes it before coming back; a
  /// slower one springs from here, easing in.
  void _catchOn(ResolvedAnchor target, double velocity) {
    final toward = target.extent - extent;
    final passes =
        velocity.abs() >= _physics.minFlingVelocity &&
        toward.abs() > _epsilon &&
        velocity.sign == toward.sign &&
        (_physics.landing(extent, velocity) - target.extent) * toward.sign >
            _epsilon;
    if (passes) {
      _coastTo(_AtAnchor(target.anchor), target.extent, velocity);
    } else {
      _settleOn(target, velocity);
    }
  }

  /// Where the sheet is coasting to, and that rest's extent.
  _Rest? _coastingTo;
  double _coastingExtent = 0;

  /// Which way the sheet is coasting: 1 up, -1 down.
  double _coastingDirection = 0;

  /// Coasts the sheet from where it is at [velocity] to [to], to spring on from there to
  /// rest at [target].
  void _coastTo(_Rest target, double to, double velocity) {
    _wheelSettle?.cancel();
    _spring.stop();
    _settlingTo = null;
    _glidingIn = null;
    _carryOn = null;
    _phase = _Phase.coasting;
    _rest = null;
    _coastingTo = target;
    _coastingExtent = to;
    _coastingDirection = velocity.sign;
    _spring.value = extent;
    _spring.animateWith(_physics.coast(extent, velocity)).then((_) {
      // Run out of speed just short of it.
      if (_phase == _Phase.coasting) _arrive(0);
    });
  }

  /// Hands a coasting sheet, at [velocity], to the spring at where it was coasting to.
  void _arrive(double velocity) {
    final target = _coastingTo;
    _coastingTo = null;
    _spring.stop();
    if (target == null) {
      _settle(velocity);
      return;
    }
    _set(_coastingExtent);
    _springTo(target, _coastingExtent, velocity);
  }

  /// The range a glide from the sheet's extent to [landing] stays within.
  ResolvedRange? _rangeFor(double landing) => _ranges.firstWhereOrNull(
    (r) => r.contains(extent) && r.contains(landing),
  );

  /// Glides the sheet from where it is at [velocity] to a stop at [landing], within one
  /// of the current ranges, slowing as [SheetPhysics.coast] has it.
  void _glideTo(double landing, double velocity) {
    _wheelSettle?.cancel();
    _spring.stop();
    _settlingTo = null;
    final range = _rangeFor(landing);
    if (range == null) {
      _settleOn(
        _anchors[_physics.target(_anchors, extent, velocity)],
        velocity,
      );
      return;
    }
    if (velocity.abs() < _physics.minFlingVelocity ||
        (landing - extent).abs() < _epsilon) {
      _restWithin(range, extent);
      return;
    }
    _phase = _Phase.gliding;
    _rest = null;
    _glidingIn = range.range;
    _carryOn = null;
    _spring.value = extent;
    _spring
        .animateWith(_physics.coast(extent, velocity))
        .then((_) => _endGlide());
  }

  /// Rests the sheet where its glide stopped.
  void _endGlide() {
    final glidingIn = _glidingIn;
    _glidingIn = null;
    _carryOn = null;
    final now = _ranges.firstWhereOrNull((r) => r.range == glidingIn);
    if (now != null) {
      _restWithin(now, extent);
    } else {
      _settle(0);
    }
  }

  /// The range the sheet is gliding within.
  SheetRange? _glidingIn;

  /// Rests the sheet at [extent] within [range], or at the anchor there if it's at one
  /// of its ends.
  void _restWithin(ResolvedRange range, double extent) {
    final extentInRange = extent.clamp(range.lower, range.upper);
    final anchor = _anchors.firstWhereOrNull(
      (a) => (a.extent - extentInRange).abs() < _epsilon,
    );
    if (anchor != null) {
      _restAt(anchor);
    } else {
      _restOn(
        _InRange(range.range, range.fractionOf(extentInRange)),
        extentInRange,
      );
    }
  }

  /// Where the sheet is springing to, and the extent the spring heads for.
  _Rest? _settlingTo;
  double _settlingExtent = 0;

  void _settleOn(ResolvedAnchor target, double velocity) =>
      _springTo(_AtAnchor(target.anchor), target.extent, velocity);

  /// Springs the sheet to rest at [target], [to] up.
  void _springTo(_Rest target, double to, double velocity) {
    _wheelSettle?.cancel();
    _glidingIn = null;
    _carryOn = null;
    if ((extent - to).abs() < _epsilon && velocity.abs() < 1) {
      _spring.stop();
      _settlingTo = null;
      _restOn(target, to);
      return;
    }
    _phase = _Phase.settling;
    _rest = null;
    _settlingTo = target;
    _settlingExtent = to;
    final simulation = SpringSimulation(
      _physics.spring,
      extent,
      to,
      velocity,
      tolerance: const Tolerance(distance: 0.5, velocity: 10),
    );
    _spring.value = extent;
    _spring.animateWith(simulation).then((_) {
      _settlingTo = null;
      // Where it is now, should it have moved on the last frame.
      _restOn(target, target.extentIn(_anchors, _ranges) ?? to);
    });
  }

  void _restAt(ResolvedAnchor anchor) =>
      _restOn(_AtAnchor(anchor.anchor), anchor.extent);

  void _restOn(_Rest rest, double extent) {
    _phase = _Phase.idle;
    _rest = rest;
    _set(extent);
  }

  void _handleSpringTick() {
    switch (_phase) {
      case _Phase.settling:
        _set(_spring.value.clamp(0.0, _maxExtent));
      case _Phase.coasting:
        final value = _spring.value;
        if ((value - _coastingExtent) * _coastingDirection >= 0) {
          _arrive(_spring.velocity);
        } else {
          _set(value.clamp(0.0, _maxExtent));
        }
      case _Phase.gliding:
        // Within the range, should its ends have moved since the glide began.
        final range = _ranges.firstWhereOrNull((r) => r.range == _glidingIn);
        final value = _spring.value;
        if (range == null) {
          _set(value);
          break;
        }
        final clamped = value.clamp(range.lower, range.upper);
        _set(clamped);
        final carryOn = _carryOn;
        if (carryOn != null && value >= _max - _epsilon) {
          // At the top: the content scrolls on from here.
          final velocity = _spring.velocity;
          _spring.stop();
          _glidingIn = null;
          _carryOn = null;
          _restWithin(range, range.upper);
          carryOn(velocity);
        } else if (value != clamped) {
          // Carried out to an end with no anchor past it. At the top of the screen it
          // stops; anywhere else it runs on past the end with the speed it has, and
          // springs back, as content scrolled past its end does.
          final velocity = _spring.velocity;
          _spring.stop();
          if (clamped >= _maxExtent - _epsilon) {
            _endGlide();
          } else {
            _glidingIn = null;
            final upper = clamped == range.upper;
            _springTo(_InRange(range.range, upper ? 1 : 0), clamped, velocity);
          }
        }
      case _Phase.idle || _Phase.held || _Phase.transition:
        break;
    }
  }

  void _set(double extent) {
    if (extent == _extent) return;
    _extent = extent;
    notifyListeners();
  }

  /// Takes the current page's [anchors] and [ranges], following where the sheet rests
  /// if it moved.
  void _setAnchors(
    List<ResolvedAnchor> anchors,
    double maxExtent, {
    List<ResolvedRange> ranges = const [],
    double floor = 0,
    bool draggable = true,
    SheetAnchor? initial,
  }) {
    _draggable = draggable;
    _initial = initial;
    _anchors = anchors;
    _ranges = ranges;
    _maxExtent = maxExtent;
    _floor = floor;
    final awaited = _placeOf(_awaited);
    if (awaited != null && !isTransitioning) {
      _awaited = null;
      _springTo(
        awaited.$1,
        awaited.$2,
        _phase == _Phase.settling ? _spring.velocity : 0,
      );
      return;
    }
    if (_phase == _Phase.settling) {
      // Springing to an anchor that moved (as when the keyboard hides): spring on to
      // where it is now, from where the sheet is and at its speed.
      final target = _settlingTo;
      final moved = target?.extentIn(_anchors, _ranges);
      if (target != null && moved == null && _anchors.isNotEmpty) {
        // It's gone: spring to the nearest instead.
        _settleOn(
          _anchors[_physics.target(_anchors, extent, 0)],
          _spring.velocity,
        );
      } else if (target != null &&
          moved != null &&
          (moved - _settlingExtent).abs() > _epsilon) {
        _springTo(target, moved, _spring.velocity);
      }
      return;
    }
    if (_phase == _Phase.coasting) {
      // Coasting to an anchor that moved: on to where it is now, or settle from here
      // if it's gone.
      final now = _coastingTo?.extentIn(_anchors, _ranges);
      if (now != null) {
        _coastingExtent = now;
      } else {
        final velocity = _spring.velocity;
        _spring.stop();
        _coastingTo = null;
        _settle(velocity);
      }
      return;
    }
    if (_phase == _Phase.gliding) {
      // Gliding within a range that's gone: settle from here instead.
      if (!_ranges.any((r) => r.range == _glidingIn)) {
        final velocity = _spring.velocity;
        _spring.stop();
        _glidingIn = null;
        _carryOn = null;
        _settle(velocity);
      }
      return;
    }
    if (_phase != _Phase.idle) return;
    final rest = _rest;
    if (rest == null) return;
    if (rest.extentIn(_anchors, _ranges) case final resting?) {
      _set(resting);
    } else if (_anchors.isNotEmpty) {
      // Where it rested is gone: spring to the nearest anchor.
      _settleOn(_anchors[_physics.target(_anchors, extent, 0)], 0);
    }
  }

  /// Hands the sheet to a page transition, which moves it with [_follow].
  void _beginTransition() {
    _spring.stop();
    _wheelSettle?.cancel();
    _glidingIn = null;
    _carryOn = null;
    _phase = _Phase.transition;
    _rest = null;
  }

  void _follow(double extent) {
    if (isTransitioning) _set(extent);
  }

  /// Takes the sheet back from a page transition, resting it at [rest] if the current
  /// page has it, or settling on the nearest anchor otherwise.
  void _endTransition(_Rest? rest) {
    if (!isTransitioning) return;
    _phase = _Phase.idle;
    final awaited = _placeOf(_awaited);
    _awaited = null;
    final target = rest?.extentIn(_anchors, _ranges);
    if (awaited != null) {
      _springTo(awaited.$1, awaited.$2, 0);
    } else if (rest != null && target != null) {
      _restOn(rest, target);
    } else {
      _settle(0);
    }
  }

  /// Places the sheet at [rest], [extent] up, before it has been shown.
  void _place(_Rest rest, double extent) {
    if (hasExtent || _phase != _Phase.idle) return;
    _restOn(rest, extent);
  }

  /// Where the sheet would rest among a page's [anchors] and [ranges] from where it is
  /// now: within a range it's in, or else at the nearest anchor. For remembering where a
  /// page was left.
  _Rest? _restFor(List<ResolvedAnchor> anchors, List<ResolvedRange> ranges) {
    if (anchors.isEmpty) return null;
    final nearest = anchors[_physics.target(anchors, extent, 0)];
    if ((nearest.extent - extent).abs() >= _epsilon) {
      final range = ranges.firstWhereOrNull((r) => r.contains(extent));
      if (range != null) return _InRange(range.range, range.fractionOf(extent));
    }
    return _AtAnchor(nearest.anchor);
  }

  /// Where the sheet rests, or would rest from where it is, on the current page.
  _Rest? get _currentRest => _rest ?? _restFor(_anchors, _ranges);

  @override
  void dispose() {
    _wheelSettle?.cancel();
    _spring.dispose();
    _collapsed.dispose();
    super.dispose();
  }
}
