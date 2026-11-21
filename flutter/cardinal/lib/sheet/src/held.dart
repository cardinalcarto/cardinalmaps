part of '../sheet.dart';

/// Marks its child as a part of a [SheetContent] that can be held in view, by [id]
/// (see [SheetContent.held]). Until it's held, it only shows its child.
///
/// While held, the child is painted over the content rather than in the slot, which
/// keeps its place and size in the content: the child stays this widget's child, so
/// its state and any gesture in progress carry on as it's taken up and put back. The
/// child is a layer of its own, and following the sheet or the scroll neither builds
/// it, lays it out nor paints it again.
///
/// A slot's height shouldn't depend on how far the sheet is pulled up, as for a
/// [SheetSection].
class SheetSlot extends StatefulWidget {
  const SheetSlot({super.key, required this.id, required this.child});

  final Object id;
  final Widget child;

  /// How far the child of the slot around [context] is painted from its slot, in
  /// logical pixels: positive while it's held up from a slot below, negative while
  /// it's held down from one above, and 0 in place. For a shadow that fades in as the
  /// content starts to pass behind it, say. Always 0 outside a slot.
  static ValueListenable<double> displacementOf(BuildContext context) =>
      context.getInheritedWidgetOfExactType<_SheetSlotScope>()?.displacement ??
      const AlwaysStoppedAnimation<double>(0);

  @override
  State<SheetSlot> createState() => _SheetSlotState();
}

class _SheetSlotState extends State<SheetSlot> {
  final _displacement = ValueNotifier<double>(0);

  @override
  void dispose() {
    _displacement.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => _SheetSlotBox(
    id: widget.id,
    entry: _SheetContentScope.of(context),
    displacement: _displacement,
    // A layer of its own, for the content to paint wherever the child is held.
    child: RepaintBoundary(
      child: _SheetSlotScope(displacement: _displacement, child: widget.child),
    ),
  );
}

class _SheetSlotScope extends InheritedWidget {
  const _SheetSlotScope({required this.displacement, required super.child});

  final ValueListenable<double> displacement;

  @override
  bool updateShouldNotify(_SheetSlotScope oldWidget) =>
      displacement != oldWidget.displacement;
}

class _SheetSlotBox extends SingleChildRenderObjectWidget {
  const _SheetSlotBox({
    required this.id,
    required this.entry,
    required this.displacement,
    required super.child,
  });

  final Object id;
  final _SheetEntry entry;
  final ValueNotifier<double> displacement;

  @override
  _RenderSheetSlot createRenderObject(BuildContext context) =>
      _RenderSheetSlot(id, entry, displacement);

  @override
  void updateRenderObject(BuildContext context, _RenderSheetSlot renderObject) {
    renderObject
      ..id = id
      ..entry = entry
      ..displacement = displacement;
  }
}

/// Where a slot sits in its content.
@immutable
class _SlotPlace {
  const _SlotPlace(this.span, this.top, this.left, this.width);

  /// Measures [slot] in [viewport], after it's laid out.
  factory _SlotPlace.of(_RenderSheetViewport viewport, _RenderSheetSlot slot) {
    // As far down the content as the slivers put it, whatever is pinned over it.
    RenderObject child = slot;
    RenderBox? outermost;
    var within = 0.0;
    while (child.parent != viewport) {
      final parent = child.parent!;
      if (child is RenderBox) outermost = child;
      within = parent is RenderSliver
          ? within + (parent.childScrollOffset(child) ?? 0)
          : 0;
      child = parent;
    }
    final inBox = MatrixUtils.transformPoint(
      slot.getTransformTo(outermost),
      Offset.zero,
    );
    final inViewport = MatrixUtils.transformPoint(
      slot.getTransformTo(viewport),
      Offset.zero,
    );
    return _SlotPlace(
      _spanOf(viewport, slot),
      viewport.contentOffsetOf(child as RenderSliver, within + inBox.dy),
      inViewport.dx,
      slot.size.width,
    );
  }

  /// Where the slot is as the anchors see the content (see [_spanOf]).
  final _Span span;

  /// How far down the content the slot's top is, and so where it's painted, less how
  /// far the content is scrolled.
  final double top;

  /// How far the slot's left edge is from the content's.
  final double left;

  final double width;

  double get height => span.end - span.start;

  @override
  bool operator ==(Object other) =>
      other is _SlotPlace &&
      other.span == span &&
      other.top == top &&
      other.left == left &&
      other.width == width;

  @override
  int get hashCode => Object.hash(span, top, left, width);
}

class _RenderSheetSlot extends RenderProxyBox {
  _RenderSheetSlot(this._id, this._entry, this.displacement);

  Object get id => _id;
  Object _id;
  set id(Object value) {
    if (value == _id) return;
    _id = value;
    // Laying out again has the viewport measure again; whether it's held may differ.
    markNeedsLayout();
    markNeedsPaint();
    entry.body?.markNeedsPaint();
  }

  _SheetEntry get entry => _entry;
  _SheetEntry _entry;
  set entry(_SheetEntry value) {
    if (value == _entry) return;
    if (attached) {
      _entry.slots.remove(this);
      value.slots.add(this);
    }
    _entry = value;
    markNeedsLayout();
  }

  /// How far the child is painted from here (see [SheetSlot.displacementOf]).
  ValueNotifier<double> displacement;

  /// Where the viewport last found this slot, or null before it has.
  _SlotPlace? place;

  /// Whether the content paints the child, wherever it's held, rather than this.
  bool get lifted => entry.holds.lifts(id);

  @override
  void attach(PipelineOwner owner) {
    super.attach(owner);
    entry.slots.add(this);
    entry.body?.markNeedsPaint();
  }

  @override
  void detach() {
    entry.slots.remove(this);
    place = null;
    entry.body?.markNeedsPaint();
    super.detach();
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    if (!lifted) super.paint(context, offset);
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) =>
      !lifted && super.hitTestChildren(result, position: position);

  @override
  void applyPaintTransform(RenderObject child, Matrix4 transform) {
    final body = entry.body;
    final held = lifted ? body?.paintedAt(this) : null;
    if (body == null || held == null) return;
    // From where this is to where the child is painted.
    final here = MatrixUtils.transformPoint(getTransformTo(body), Offset.zero);
    transform.translateByDouble(held.dx - here.dx, held.dy - here.dy, 0, 1);
  }
}

/// A slot's child held over the content: how far it has moved from its slot to where
/// it's held, and how far it has faded in there.
class _Hold {
  _Hold(
    this.id, {
    required TickerProvider vsync,
    required double strength,
    required double opacity,
  }) : strength = AnimationController(vsync: vsync, value: strength),
       opacity = AnimationController(vsync: vsync, value: opacity);

  final Object id;

  /// How far the child is from its slot (0) to where it's held (1).
  final AnimationController strength;

  /// How solid the child is while it's wholly out of its slot; it's always solid in
  /// it.
  final AnimationController opacity;

  /// Whether this is the slot the page holds, rather than one on its way back.
  bool isTarget = true;

  /// Whether it goes back by fading out where it is, under the child held now, rather
  /// than by moving to its slot.
  bool fading = false;

  final LayerHandle<ClipPathLayer> clip = LayerHandle();
  final LayerHandle<OpacityLayer> layer = LayerHandle();

  void dispose() {
    strength.dispose();
    opacity.dispose();
    clip.layer = null;
    layer.layer = null;
  }
}

/// The slots of a page whose children are over its content: the one it holds
/// ([SheetContent.held]), and any still on their way back to their slots since it
/// held them. Listeners hear as they move.
class _SheetHolds extends ChangeNotifier {
  _SheetHolds(this._vsync, this._entry);

  final TickerProvider _vsync;
  final _SheetEntry _entry;

  Duration duration = const Duration(milliseconds: 200);
  Curve curve = Curves.easeOut;

  /// In the order they're painted: the held one last, over the rest.
  final List<_Hold> _holds = [];
  List<_Hold> get all => _holds;

  bool get isEmpty => _holds.isEmpty;

  /// The slot the page holds, by its ID.
  Object? get target => _target;
  Object? _target;
  set target(Object? id) {
    if (id == _target) return;
    // The next layout takes it up, once the slots are where the change finds them.
    _changedFrom ??= (_target,);
    _target = id;
  }

  /// The slot that was held when [target] changed, until [take] has taken that up.
  (Object?,)? _changedFrom;

  /// Holds [id] from the first, with nothing to show the change.
  void start(Object? id) {
    _target = id;
    if (id != null) _holds.add(_make(id, strength: 1, opacity: 1));
  }

  _Hold _make(Object id, {required double strength, required double opacity}) {
    final hold = _Hold(id, vsync: _vsync, strength: strength, opacity: opacity);
    hold.strength.addListener(notifyListeners);
    hold.opacity.addListener(notifyListeners);
    return hold;
  }

  _Hold? find(Object id) => _holds.firstWhereOrNull((hold) => hold.id == id);

  /// Whether the child of the slot [id] is over the content, rather than in its slot.
  bool lifts(Object id) => _holds.any((hold) => hold.id == id);

  /// Takes up a change of [target], if there's been one: the newly held child starts
  /// for where it's held, and the one held before starts back for its slot. If
  /// [crossFades] says so of the two, the one fades into the other where they are
  /// instead.
  void take({required bool Function(_Hold from, Object to) crossFades}) {
    final change = _changedFrom;
    if (change == null) return;
    _changedFrom = null;
    final to = _target;
    if (change.$1 == to) return;
    final from = switch (change.$1) {
      final id? => find(id),
      null => null,
    };
    var incoming = to == null ? null : find(to);
    final fade =
        from != null && to != null && incoming == null && crossFades(from, to);
    if (from != null) {
      from
        ..isTarget = false
        ..fading = fade;
      if (fade) {
        _animate(from.opacity, 0);
      } else {
        _animate(from.strength, 0);
        _animate(from.opacity, 1);
      }
    }
    if (to != null) {
      if (incoming == null) {
        incoming = _make(to, strength: fade ? 1 : 0, opacity: fade ? 0 : 1);
      } else {
        _holds.remove(incoming);
      }
      _holds.add(incoming);
      incoming
        ..isTarget = true
        ..fading = false;
      _animate(incoming.strength, 1);
      _animate(incoming.opacity, 1);
      _repaint(to);
    }
  }

  void _animate(AnimationController controller, double to) {
    if (controller.value == to) {
      controller.stop();
    } else {
      controller.animateTo(to, duration: duration, curve: curve);
    }
  }

  /// Sends the child of [hold], which was fading out where it was, back to its slot
  /// instead: [solid] at once, if it is already.
  void slideBack(_Hold hold, {required bool solid}) {
    hold.fading = false;
    _animate(hold.strength, 0);
    if (solid) {
      hold.opacity.value = 1;
    } else {
      _animate(hold.opacity, 1);
    }
  }

  /// Puts the child of [hold] back in its slot.
  void release(_Hold hold) {
    _holds.remove(hold);
    hold.dispose();
    _repaint(hold.id);
  }

  /// Paints the slot [id] and the content again, as one takes the child from the
  /// other.
  void _repaint(Object id) {
    _entry.slotFor(id)?.markNeedsPaint();
    _entry.body?.markNeedsPaint();
  }

  @override
  void dispose() {
    for (final hold in _holds) {
      hold.dispose();
    }
    _holds.clear();
    super.dispose();
  }
}

/// Where a held child is painted.
class _Held {
  const _Held(this.box, this.displacement);

  /// Its box, from the content's top left corner: as wide and tall as its slot.
  final Rect box;

  /// How far that is above its slot (see [SheetSlot.displacementOf]).
  final double displacement;

  Offset get offset => box.topLeft;

  /// How far out of its slot it is, from 0 in it to 1 wholly out of it.
  double get away => box.height > 0
      ? (displacement.abs() / box.height).clamp(0.0, 1.0)
      : (isInPlace ? 0 : 1);

  /// Whether it's wholly out of its slot.
  bool get isAway =>
      box.height > 0 && displacement.abs() >= box.height - _epsilon;

  /// Whether it's where its slot is.
  bool get isInPlace => displacement.abs() < precisionErrorTolerance;

  /// How much of this the box of [other] covers, top to bottom, from 0 to 1.
  double coveredBy(_Held other) {
    if (box.height <= 0) return 0;
    final overlap =
        math.min(box.bottom, other.box.bottom) -
        math.max(box.top, other.box.top);
    return (overlap / box.height).clamp(0.0, 1.0);
  }
}

enum _BodySlot { viewport, footer }

/// A page's content as it's painted: the viewport, over it the children of the slots
/// the page holds, and over those the footer.
///
/// It's as tall as the viewport, which moves with the sheet, so the footer and the
/// held children are placed in it by the sheet's extent (and how far the content is
/// scrolled) each time it's painted. Moving the sheet or scrolling builds and lays out
/// neither.
///
/// Nothing shows through the footer or a child held out of its slot, whether or not
/// they paint a background: the content is clipped out of their boxes, and isn't hit
/// there. Each is a layer, so as the boxes move only the clips change shape, and
/// nothing is painted again.
class _SheetBody
    extends SlottedMultiChildRenderObjectWidget<_BodySlot, RenderBox> {
  const _SheetBody({
    required this.entry,
    required this.controller,
    required this.extent,
    required this.offset,
    required this.held,
    required this.footerPadding,
    required this.viewport,
    required this.footer,
  });

  final _SheetEntry entry;
  final SheetController controller;
  final double Function() extent;
  final ViewportOffset offset;
  final Object? held;
  final double footerPadding;
  final Widget viewport;
  final Widget? footer;

  @override
  Iterable<_BodySlot> get slots => _BodySlot.values;

  @override
  Widget? childForSlot(_BodySlot slot) => switch (slot) {
    _BodySlot.viewport => viewport,
    _BodySlot.footer => footer,
  };

  @override
  _RenderSheetBody createRenderObject(BuildContext context) => _RenderSheetBody(
    entry: entry,
    controller: controller,
    extent: extent,
    offset: offset,
    held: held,
    footerPadding: footerPadding,
  );

  @override
  void updateRenderObject(BuildContext context, _RenderSheetBody renderObject) {
    renderObject
      ..entry = entry
      ..controller = controller
      ..extent = extent
      ..offset = offset
      ..held = held
      ..footerPadding = footerPadding;
  }
}

class _RenderSheetBody extends RenderBox
    with SlottedContainerRenderObjectMixin<_BodySlot, RenderBox> {
  _RenderSheetBody({
    required this._entry,
    required this._controller,
    required this.extent,
    required this._offset,
    required this._held,
    required this._footerPadding,
  }) {
    _entry.body = this;
  }

  _SheetEntry get entry => _entry;
  _SheetEntry _entry;
  set entry(_SheetEntry value) {
    if (value == _entry) return;
    if (attached) {
      _entry.holds.removeListener(_handleMove);
      value.holds.addListener(_handleMove);
    }
    if (_entry.body == this) _entry.body = null;
    _entry = value..body = this;
    markNeedsLayout();
  }

  SheetController get controller => _controller;
  SheetController _controller;
  set controller(SheetController value) {
    if (value == _controller) return;
    if (attached) {
      _controller.removeListener(_handleMove);
      value.addListener(_handleMove);
    }
    _controller = value;
    markNeedsPaint();
  }

  /// Where the sheet is painted (see [_SheetScaffoldState._paintExtent]).
  double Function() extent;

  /// How far the content is scrolled.
  ViewportOffset get offset => _offset;
  ViewportOffset _offset;
  set offset(ViewportOffset value) {
    if (value == _offset) return;
    if (attached) {
      _offset.removeListener(_handleMove);
      value.addListener(_handleMove);
    }
    _offset = value;
    markNeedsPaint();
  }

  /// The slot the page holds, which the next layout takes up when it changes.
  Object? get held => _held;
  Object? _held;
  set held(Object? value) {
    if (value == _held) return;
    _held = value;
    markNeedsLayout();
  }

  /// How much of the footer's height is behind the navigation bar, below what the
  /// content keeps clear of (see [SheetGeometry.footerPadding]).
  double get footerPadding => _footerPadding;
  double _footerPadding;
  set footerPadding(double value) {
    if (value == _footerPadding) return;
    _footerPadding = value;
    markNeedsLayout();
  }

  RenderBox? get _viewport => childForSlot(_BodySlot.viewport);
  RenderBox? get _footer => childForSlot(_BodySlot.footer);

  final LayerHandle<ClipPathLayer> _clip = LayerHandle();

  @override
  void attach(PipelineOwner owner) {
    super.attach(owner);
    _controller.addListener(_handleMove);
    _offset.addListener(_handleMove);
    _entry.holds.addListener(_handleMove);
  }

  @override
  void detach() {
    _controller.removeListener(_handleMove);
    _offset.removeListener(_handleMove);
    _entry.holds.removeListener(_handleMove);
    super.detach();
  }

  @override
  void dispose() {
    _clip.layer = null;
    if (_entry.body == this) _entry.body = null;
    super.dispose();
  }

  /// The sheet, the scroll or a hold moved: the footer and the held children are
  /// somewhere else in this.
  void _handleMove() {
    final holding = !entry.holds.isEmpty;
    if (!holding && _footer == null) return;
    markNeedsPaint();
    if (!holding) return;
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      _scheduleTidy();
    } else {
      _tidy();
    }
  }

  bool _tidyScheduled = false;

  /// Tidies after this frame, for when it's building, laying out or painting.
  void _scheduleTidy() {
    if (_tidyScheduled) return;
    _tidyScheduled = true;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _tidyScheduled = false;
      if (attached) _tidy();
    });
  }

  /// Puts back the children that have reached their slots, and tells every slot's
  /// child how far it is from its slot now.
  void _tidy() {
    _releaseSettled();
    for (final slot in entry.slots) {
      slot.displacement.value = _heldAt(slot)?.displacement ?? 0;
    }
  }

  /// Puts back in their slots the children the page no longer holds that are painted
  /// where their slots are: nothing changes on screen. One that has faded out under
  /// the child held now, or is no longer under it, goes back to its slot from there.
  /// So too any whose slot has gone.
  void _releaseSettled() {
    final holds = entry.holds;
    for (final hold in holds.all.toList()) {
      if (hold.isTarget) continue;
      final held = _place(hold);
      if (held == null) {
        holds.release(hold);
      } else if (!hold.fading) {
        if (held.isInPlace) holds.release(hold);
      } else {
        final covered = _covered(held);
        if (hold.opacity.value > 0 && covered > 0) continue;
        if (held.isInPlace) {
          holds.release(hold);
        } else {
          holds.slideBack(hold, solid: covered == 0);
        }
      }
    }
  }

  SheetGeometry? get _geometry => entry.geometry;

  double get _extent {
    final extent = this.extent();
    final geometry = _geometry;
    return geometry == null ? extent : extent.clamp(0.0, geometry.size.height);
  }

  /// The top of the footer, or of whatever else covers the bottom of the window,
  /// from the content's top.
  double get _footerTop {
    final geometry = _geometry;
    return geometry == null
        ? size.height - entry.footerHeight
        : geometry.contentShown(_extent);
  }

  /// Where the child [hold] holds is painted: null if its slot isn't laid out.
  _Held? _place(_Hold hold) {
    final place = entry.slotFor(hold.id)?.place;
    return place == null ? null : _placeAt(place, hold.strength.value);
  }

  /// Where the child of the slot at [place] is painted, [strength] of the way from
  /// its slot to where it's held.
  _Held _placeAt(_SlotPlace place, double strength) {
    final natural = place.top - (offset.hasPixels ? offset.pixels : 0);
    // Above the footer and, sooner than that, below the content's top.
    final clamped = math.max(0.0, math.min(natural, _footerTop - place.height));
    final displacement = (natural - clamped) * strength;
    return _Held(
      Offset(place.left, natural - displacement) &
          Size(place.width, place.height),
      displacement,
    );
  }

  /// How much of [held], a child fading out, is under the child the page holds now,
  /// from 0 to 1.
  double _covered(_Held held) {
    final target = switch (entry.holds.target) {
      final id? => entry.holds.find(id),
      null => null,
    };
    final over = target == null ? null : _place(target);
    return over == null ? 0 : held.coveredBy(over);
  }

  /// How solid the child [hold] holds is painted at [held]. The held child is solid
  /// in its slot, and as solid as it has faded in once wholly out of it; so is one on
  /// its way back to its slot. One fading out is as faded as it's under the child held
  /// now, so it's solid by the time it's out from under it.
  double _opacityOf(_Hold hold, _Held held) {
    final faded = 1 - hold.opacity.value;
    if (faded == 0) return 1;
    return 1 - faded * (hold.fading ? _covered(held) : held.away);
  }

  /// The footer's box in this: all of it, down behind the navigation bar.
  Rect? get _footerBox => switch (_footer) {
    final footer? => Offset(0, _footerTop) & footer.size,
    null => null,
  };

  /// The boxes of the children held out of their slots, in the order they're painted,
  /// each with its hold.
  List<(_Hold, _Held)> _lifted() => [
    for (final hold in entry.holds.all)
      if (_place(hold) case final held? when !held.isInPlace) (hold, held),
  ];

  /// All of this but [boxes], or null if that's all of it.
  Path? _allBut(Iterable<Rect> boxes) {
    if (boxes.isEmpty) return null;
    final out = Path();
    for (final box in boxes) {
      out.addRect(box);
    }
    return Path.combine(
      PathOperation.difference,
      Path()..addRect(Offset.zero & size),
      out,
    );
  }

  /// Where the child of [slot] is painted, if it's held.
  _Held? _heldAt(_RenderSheetSlot slot) => switch (entry.holds.find(slot.id)) {
    final hold? when slot.place != null => _place(hold),
    _ => null,
  };

  /// Where the child of [slot] is painted in this, if it's held.
  Offset? paintedAt(_RenderSheetSlot slot) => _heldAt(slot)?.offset;

  /// Whether the child held as [from] fades into the child of the slot [to], rather
  /// than each moving: when [to]'s child comes to rest just where [from]'s is painted
  /// now, each covering all of the other's height to within [_epsilon]. Both held
  /// against the same edge, say, or [from]'s in its slot exactly where [to]'s is held.
  bool _crossFades(_Hold from, Object to) {
    final a = _place(from);
    final place = entry.slotFor(to)?.place;
    if (a == null || place == null) return false;
    final b = _placeAt(place, 1);
    final overlap =
        math.min(a.box.bottom, b.box.bottom) - math.max(a.box.top, b.box.top);
    final taller = math.max(a.box.height, b.box.height);
    return taller > 0 && overlap >= taller - _epsilon;
  }

  @override
  Size computeDryLayout(BoxConstraints constraints) => constraints.biggest;

  @override
  void performLayout() {
    size = constraints.biggest;
    var footerHeight = 0.0;
    if (_footer case final footer?) {
      footer.layout(
        BoxConstraints(
          minWidth: size.width,
          maxWidth: size.width,
          maxHeight: size.height,
        ),
        parentUsesSize: true,
      );
      footerHeight = math.max(0, footer.size.height - footerPadding);
    }
    if (footerHeight != entry.footerHeight) {
      entry.footerHeight = footerHeight;
      // The space at the content's end follows it, and so do the anchors, which the
      // viewport places as it's laid out.
      invokeLayoutCallback<BoxConstraints>((_) {
        entry.endInset?.markNeedsLayout();
        _viewport?.markNeedsLayout();
      });
    }
    _viewport?.layout(BoxConstraints.tight(size));
    // With the slots where they are now.
    entry.holds.take(crossFades: _crossFades);
    if (!entry.holds.isEmpty) {
      _releaseSettled();
      _scheduleTidy();
    }
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    final footerBox = _footerBox;
    final lifted = _lifted();
    // The content, but not where the footer or a child held out of its slot is.
    final shown = _allBut([
      ?footerBox,
      for (final (_, held) in lifted) held.box,
    ]);
    if (_viewport case final viewport?) {
      if (shown == null) {
        _clip.layer = null;
        context.paintChild(viewport, offset);
      } else {
        _clip.layer = context.pushClipPath(
          needsCompositing,
          offset,
          Offset.zero & size,
          shown,
          (context, offset) => context.paintChild(viewport, offset),
          oldLayer: _clip.layer,
        );
      }
    }
    for (final hold in entry.holds.all) {
      final child = entry.slotFor(hold.id)?.child;
      final held = _place(hold);
      final alpha = held == null ? 0 : (255 * _opacityOf(hold, held)).round();
      if (child == null || held == null || alpha == 0) {
        hold.clip.layer = null;
        hold.layer.layer = null;
        continue;
      }
      // Within the content, as the viewport keeps its own, and not where the footer
      // is. One on its way back to its slot is as the content is: not where a child
      // painted over it is held. One fading out shows through the one fading in.
      final holds = entry.holds.all;
      final over = [
        ?footerBox,
        if (!hold.fading)
          for (final (other, held) in lifted)
            if (holds.indexOf(other) > holds.indexOf(hold)) held.box,
      ];
      hold.clip.layer = context.pushClipPath(
        needsCompositing,
        offset,
        Offset.zero & size,
        _allBut(over) ?? (Path()..addRect(Offset.zero & size)),
        (context, offset) {
          if (alpha < 255) {
            hold.layer.layer = context.pushOpacity(
              offset,
              alpha,
              (context, offset) =>
                  context.paintChild(child, offset + held.offset),
              oldLayer: hold.layer.layer,
            );
          } else {
            hold.layer.layer = null;
            context.paintChild(child, offset + held.offset);
          }
        },
        oldLayer: hold.clip.layer,
      );
    }
    if (_footer case final footer?) {
      context.paintChild(footer, offset + Offset(0, _footerTop));
    }
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) {
    bool hit(RenderBox? child, Offset offset) =>
        child != null &&
        result.addWithPaintOffset(
          offset: offset,
          position: position,
          hitTest: (result, position) =>
              child.hitTest(result, position: position),
        );
    // As painted, from the top: the footer, the held children, the content. Nothing
    // is under the footer, or under a child held out of its slot.
    if (_footerBox case final box? when box.contains(position)) {
      return hit(_footer, box.topLeft);
    }
    for (final hold in entry.holds.all.reversed) {
      final held = _place(hold);
      if (held == null) continue;
      final shows = _opacityOf(hold, held) > 0;
      if (shows && hit(entry.slotFor(hold.id)?.child, held.offset)) return true;
      if (!held.isInPlace && held.box.contains(position)) return false;
    }
    return hit(_viewport, Offset.zero);
  }

  @override
  void applyPaintTransform(RenderBox child, Matrix4 transform) {
    if (child == _footer) transform.translateByDouble(0, _footerTop, 0, 1);
  }
}

/// A page's footer, which is within the page's scrollable but doesn't scroll with its
/// content: its semantics say so, as a pinned header's do.
class _SheetFooter extends SingleChildRenderObjectWidget {
  const _SheetFooter({required super.child});

  @override
  RenderObject createRenderObject(BuildContext context) => _RenderSheetFooter();
}

class _RenderSheetFooter extends RenderProxyBox {
  @override
  void describeSemanticsConfiguration(SemanticsConfiguration config) {
    super.describeSemanticsConfiguration(config);
    config
      ..addTagForChildren(RenderViewport.useTwoPaneSemantics)
      ..addTagForChildren(RenderViewport.excludeFromScrolling);
  }
}

/// The space at the end of a page's content that keeps it clear of what covers the
/// bottom of the window: [inset], and the page's footer, whose height isn't known
/// until it's laid out.
class _SheetEndInset extends LeafRenderObjectWidget {
  const _SheetEndInset({required this.entry, required this.inset});

  final _SheetEntry entry;
  final double inset;

  @override
  _RenderSheetEndInset createRenderObject(BuildContext context) =>
      _RenderSheetEndInset(entry, inset);

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderSheetEndInset renderObject,
  ) {
    renderObject
      ..entry = entry
      ..inset = inset;
  }
}

class _RenderSheetEndInset extends RenderBox {
  _RenderSheetEndInset(this._entry, this._inset);

  _SheetEntry get entry => _entry;
  _SheetEntry _entry;
  set entry(_SheetEntry value) {
    if (value == _entry) return;
    if (attached) {
      if (_entry.endInset == this) _entry.endInset = null;
      value.endInset = this;
    }
    _entry = value;
    markNeedsLayout();
  }

  double get inset => _inset;
  double _inset;
  set inset(double value) {
    if (value == _inset) return;
    _inset = value;
    markNeedsLayout();
  }

  @override
  void attach(PipelineOwner owner) {
    super.attach(owner);
    entry.endInset = this;
  }

  @override
  void detach() {
    if (entry.endInset == this) entry.endInset = null;
    super.detach();
  }

  @override
  Size computeDryLayout(BoxConstraints constraints) =>
      constraints.constrain(Size(0, inset + entry.footerHeight));

  @override
  void performLayout() {
    size = computeDryLayout(constraints);
  }
}
