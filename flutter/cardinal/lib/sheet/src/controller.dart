part of '../sheet.dart';

enum _Phase {
  /// At rest, or not yet placed.
  idle,

  /// Following a finger (or a mouse wheel).
  held,

  /// Springing to an anchor.
  settling,

  /// Following a page transition.
  transition,
}

/// Where the sheet is, and the way to move it.
///
/// Its value is the sheet's extent: how far it's pulled up, in logical pixels from the
/// bottom of the screen. Listeners hear about every change, every frame the sheet moves.
class SheetController extends ChangeNotifier implements ValueListenable<double> {
  SheetController({required TickerProvider vsync, this._physics = const SheetPhysics()})
    : _spring = AnimationController.unbounded(vsync: vsync) {
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

  /// The anchor the sheet is resting at, or null while it's moving.
  SheetAnchor? get restingAt => _restingAt;
  SheetAnchor? _restingAt;

  /// The furthest the sheet can be pulled up at all.
  double _maxExtent = double.infinity;

  /// The lowest the sheet goes, even pulled past its lowest anchor: the keyboard's top.
  double _floor = 0;

  double get _min => _anchors.isEmpty ? 0 : _anchors.first.extent;
  double get _max => _anchors.isEmpty ? _maxExtent : _anchors.last.extent;

  /// Whether the sheet is at its highest anchor, where dragging scrolls its content.
  bool get isAtMax => extent >= _max - _epsilon;

  /// Whether a finger (or a mouse wheel) is moving the sheet.
  bool get isHeld => _phase == _Phase.held;

  /// Whether a page transition is moving the sheet.
  bool get isTransitioning => _phase == _Phase.transition;

  /// Springs the sheet to [anchor], one of the current page's anchors, keeping whatever
  /// speed it has.
  void animateTo(SheetAnchor anchor) {
    if (isTransitioning) return;
    final target = _anchors.firstWhereOrNull((a) => a.anchor == anchor);
    if (target == null) return;
    _settleOn(target, _phase == _Phase.settling ? _spring.velocity : 0);
  }

  /// Springs the sheet to the next anchor up, or back to the lowest from the highest.
  void cycle() {
    if (isTransitioning || _anchors.isEmpty) return;
    final next = _anchors.firstWhereOrNull((a) => a.extent > extent + _epsilon);
    _settleOn(next ?? _anchors.first, 0);
  }

  /// Stops the sheet where it is, for a finger to take hold of it.
  ///
  /// Only [release] with the same [holder] lets go of it, so whatever took hold last
  /// decides where it settles.
  void hold({Object? holder}) {
    if (isTransitioning) return;
    _spring.stop();
    _wheelSettle?.cancel();
    _phase = _Phase.held;
    _holder = holder;
    _restingAt = null;
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
      if (rest < 0) extent = math.max(math.min(_floor, extent), extent + rest * resistance);
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
  /// upwards), springing it to the anchor [SheetPhysics.target] picks.
  void release(double velocity, {Object? holder}) {
    if (!isHeldBy(holder)) return;
    _settle(velocity);
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
    _settleOn(_anchors[_physics.target(_anchors, extent, velocity)], velocity);
  }

  /// The anchor the sheet is springing to, and where it was when the spring started.
  ResolvedAnchor? _settlingTo;

  void _settleOn(ResolvedAnchor target, double velocity) {
    _wheelSettle?.cancel();
    if ((extent - target.extent).abs() < _epsilon && velocity.abs() < 1) {
      _spring.stop();
      _settlingTo = null;
      _rest(target);
      return;
    }
    _phase = _Phase.settling;
    _restingAt = null;
    _settlingTo = target;
    final simulation = SpringSimulation(
      _physics.spring,
      extent,
      target.extent,
      velocity,
      tolerance: const Tolerance(distance: 0.5, velocity: 10),
    );
    _spring.value = extent;
    _spring.animateWith(simulation).then((_) {
      _settlingTo = null;
      // Where the anchor is now, should it have moved on the last frame.
      _rest(_anchors.firstWhereOrNull((a) => a.anchor == target.anchor) ?? target);
    });
  }

  void _rest(ResolvedAnchor anchor) {
    _phase = _Phase.idle;
    _restingAt = anchor.anchor;
    _set(anchor.extent);
  }

  void _handleSpringTick() {
    if (_phase != _Phase.settling) return;
    _set(_spring.value.clamp(0.0, _maxExtent));
  }

  void _set(double extent) {
    if (extent == _extent) return;
    _extent = extent;
    notifyListeners();
  }

  /// Takes the current page's [anchors], following the one the sheet rests at if it moved.
  void _setAnchors(List<ResolvedAnchor> anchors, double maxExtent, {double floor = 0}) {
    _anchors = anchors;
    _maxExtent = maxExtent;
    _floor = floor;
    if (_phase == _Phase.settling) {
      // Springing to an anchor that moved (as when the keyboard hides): spring on to
      // where it is now, from where the sheet is and at its speed.
      final target = _settlingTo;
      final moved = _anchors.firstWhereOrNull((a) => a.anchor == target?.anchor);
      if (target != null && moved != null && (moved.extent - target.extent).abs() > _epsilon) {
        _settleOn(moved, _spring.velocity);
      }
      return;
    }
    if (_phase != _Phase.idle) return;
    final resting = _anchors.firstWhereOrNull((a) => a.anchor == _restingAt);
    if (resting != null) {
      _set(resting.extent);
    } else if (_anchors.isNotEmpty && _restingAt != null) {
      // The anchor it rested at is gone: find the nearest.
      _rest(_anchors[_physics.target(_anchors, extent, 0)]);
    }
  }

  /// Hands the sheet to a page transition, which moves it with [_follow].
  void _beginTransition() {
    _spring.stop();
    _wheelSettle?.cancel();
    _phase = _Phase.transition;
    _restingAt = null;
  }

  void _follow(double extent) {
    if (isTransitioning) _set(extent);
  }

  /// Takes the sheet back from a page transition, resting it at [anchor] if the current
  /// page has it, or settling on the nearest anchor otherwise.
  void _endTransition(SheetAnchor? anchor) {
    if (!isTransitioning) return;
    _phase = _Phase.idle;
    final target = _anchors.firstWhereOrNull((a) => a.anchor == anchor);
    if (target != null) {
      _rest(target);
    } else {
      _settle(0);
    }
  }

  /// Places the sheet at [anchor] before it has been shown.
  void _place(ResolvedAnchor anchor) {
    if (hasExtent || _phase != _Phase.idle) return;
    _rest(anchor);
  }

  /// The anchor nearest the sheet, for remembering where a page was left.
  SheetAnchor? get _nearestAnchor =>
      _restingAt ??
      (_anchors.isEmpty ? null : _anchors[_physics.target(_anchors, extent, 0)].anchor);

  @override
  void dispose() {
    _wheelSettle?.cancel();
    _spring.dispose();
    super.dispose();
  }
}
