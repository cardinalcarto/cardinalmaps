part of '../sheet.dart';

/// A page's content in the sheet: where the sheet can rest while it's showing, and the
/// slivers it scrolls through once the sheet is at its highest anchor.
///
/// Usually the child of a [SheetPage]. Its slivers are laid out in a viewport as tall as
/// the content shows at the highest anchor, so they can scroll to their end there.
class SheetContent extends StatefulWidget {
  const SheetContent({
    super.key,
    required this.anchors,
    this.freeRanges = const [],
    this.initialAnchor = 0,
    this.initialSection,
    this.initialMaxFraction = 1,
    this.showsBottomBar = false,
    this.draggable = true,
    this.collapsible = false,
    this.keepsHeight = false,
    this.scrollId,
    this.footer,
    this.held,
    this.holdDuration = const Duration(milliseconds: 200),
    this.holdCurve = Curves.easeOut,
    required this.slivers,
  }) : assert(anchors.length > 0);

  /// Where the sheet can rest, in any order.
  final List<SheetAnchor> anchors;

  /// Stretches between pairs of [anchors] where the sheet rests wherever it's let go,
  /// gliding to a stop after a fling, rather than springing to an anchor.
  final List<SheetRange> freeRanges;

  /// The index into [anchors] the sheet moves to when this content first shows.
  final int initialAnchor;

  /// A section (see [SheetSection]) for the sheet to first rest showing the content
  /// down to the end of, within one of the [freeRanges], rather than at
  /// [initialAnchor]: as [SheetAnchor.section] would rest, but no higher than
  /// [initialMaxFraction]. If that isn't within one of the [freeRanges], the sheet
  /// first rests at [initialAnchor] after all. Like a section anchor, it can't be
  /// placed until its section and the ranges' ends are laid out.
  final Object? initialSection;

  /// The highest the sheet first rests for [initialSection], as a fraction of the
  /// furthest it can be pulled up (see [SheetAnchor.fraction]).
  final double initialMaxFraction;

  /// Whether the scaffold's bottom bar shows on this page, in which case the anchors
  /// and the end of the content keep clear of it. It slides away and back as pages
  /// change.
  final bool showsBottomBar;

  /// Whether dragging moves the sheet while this page shows. If not, the sheet stays
  /// where it is under a finger, a mouse wheel or a tap on the handle, and content too
  /// long to fit still scrolls; [SheetController.animateTo] and page changes still move
  /// it.
  final bool draggable;

  /// Whether the sheet can be pulled down out of sight while this page shows, to rest
  /// at [SheetAnchor.collapsed] below its [anchors] until [SheetController.expand]
  /// brings it back. Usually for a page that [showsBottomBar], where the bar is the
  /// way to bring it back.
  final bool collapsible;

  /// Whether the sheet stays where it rests when this page comes in, pushed or gone
  /// back to, if this page has that anchor (or range) too. Otherwise, as for any page,
  /// it moves to the [initialAnchor] of a page pushed, and back to where it was left
  /// on a page gone back to.
  final bool keepsHeight;

  /// Which content [slivers] are, for a page that shows different content in turn
  /// (a list, then one of its items). When it changes, the content scrolls to where
  /// that content was last left, or to its top the first time, rather than keeping the
  /// last content's scroll offset; each content's offset is remembered while the page
  /// shows. Null, the default, is one content throughout.
  final Object? scrollId;

  /// Stays at the bottom of the window (of the column, in a wide window) wherever the
  /// sheet is, above the navigation bar, the scaffold's bottom bar on a page that
  /// [showsBottomBar], and the keyboard while it shows. It's as wide as the content
  /// and takes its own height, which shouldn't depend on how far the sheet is pulled
  /// up. It's part of the page, so it comes and goes with it.
  ///
  /// Its height joins what covers the bottom of the window
  /// ([SheetGeometry.bottomInset]), so the anchors and the end of the content keep
  /// clear of it. The content passes behind it as the sheet lowers, so it wants a
  /// background of its own. A vertical drag on it moves the sheet as one on the
  /// content does.
  final Widget? footer;

  /// The [SheetSlot] among the [slivers] whose child is held in view, by the slot's
  /// ID, or null to hold none.
  ///
  /// While its slot is in view the child shows there, as any other. Once the sheet
  /// lowers, or the content scrolls, so far that the slot would pass under the
  /// [footer] (or whatever else covers the bottom of the window), or above the top of
  /// the content, the child stops there, and the rest of the content passes behind it:
  /// so it wants a background of its own. It's painted over the content, and so
  /// outside whatever the slot's ancestors paint around it (a clip, a fade).
  ///
  /// When this changes the children move to and from their places over
  /// [holdDuration]; when both are wholly out of view past the same edge, the one
  /// fades into the other in place instead.
  ///
  /// The slot must be built to be held: one in a sliver that only builds what's near
  /// the viewport (a [SliverList], say) isn't held once it scrolls far enough away
  /// not to be. A [SliverToBoxAdapter] always builds its child.
  final Object? held;

  /// How long a child takes to move to or from where it's held when [held] changes.
  final Duration holdDuration;

  /// The curve a child moves along when [held] changes.
  final Curve holdCurve;

  final List<Widget> slivers;

  @override
  State<SheetContent> createState() => _SheetContentState();
}

class _SheetContentState extends State<SheetContent>
    with TickerProviderStateMixin {
  late final _SheetEntry _entry = _SheetEntry(this);
  _SheetScaffoldState? _scaffold;
  _SheetScrollController? _scroll;

  /// Where the sheet can rest on this page: [SheetContent.anchors], and the collapsed
  /// anchor after them, so [SheetContent.initialAnchor] is still theirs.
  List<SheetAnchor> get _anchors => widget.collapsible
      ? [...widget.anchors, const SheetAnchor.collapsed()]
      : widget.anchors;

  /// [SheetScaffold]'s geometry, as this page sees it.
  SheetGeometry _geometry(_SheetScope scope) => widget.showsBottomBar
      ? scope.geometry.withBottomBar(scope.bottomBarHeight)
      : scope.geometry;

  @override
  void initState() {
    super.initState();
    _entry.holds
      ..duration = widget.holdDuration
      ..curve = widget.holdCurve
      ..start(widget.held);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final scope = _SheetScope.of(context);
    if (scope.state != _scaffold) {
      _scaffold?._unregister(_entry);
      _scroll?.dispose();
      final scaffold = _scaffold = scope.state;
      _scroll = _SheetScrollController(
        scope.controller,
        () => scaffold._active == _entry,
      );
      scaffold._register(_entry);
    }
    _entry
      ..route = ModalRoute.of(context)
      ..baseGeometry = _geometry(scope)
      ..physics = scope.controller.physics
      ..anchors = _anchors
      ..freeRanges = widget.freeRanges
      ..initialAnchor = widget.initialAnchor
      ..initialSection = widget.initialSection
      ..initialMaxFraction = widget.initialMaxFraction
      ..showsBottomBar = widget.showsBottomBar
      ..draggable = widget.draggable
      ..keepsHeight = widget.keepsHeight;
    _scaffold!._scheduleUpdate();
  }

  /// Where each content [SheetContent.scrollId] names was left, by its ID.
  final Map<Object?, double> _scrolledTo = {};

  @override
  void didUpdateWidget(SheetContent oldWidget) {
    super.didUpdateWidget(oldWidget);
    final scroll = _scroll;
    if (widget.scrollId != oldWidget.scrollId &&
        scroll != null &&
        scroll.hasClients) {
      // Before this frame lays out the new content, so it never shows at the old
      // content's offset. Layout then brings an offset past its end back within it.
      final position = scroll.position;
      _scrolledTo[oldWidget.scrollId] = position.pixels;
      position.jumpTo(_scrolledTo.remove(widget.scrollId) ?? 0);
    }
    _entry
      ..baseGeometry = _geometry(_SheetScope.of(context))
      ..anchors = _anchors
      ..freeRanges = widget.freeRanges
      ..initialAnchor = widget.initialAnchor
      ..initialSection = widget.initialSection
      ..initialMaxFraction = widget.initialMaxFraction
      ..showsBottomBar = widget.showsBottomBar
      ..draggable = widget.draggable
      ..keepsHeight = widget.keepsHeight;
    _entry.holds
      ..duration = widget.holdDuration
      ..curve = widget.holdCurve
      ..target = widget.held;
    if (widget.draggable != oldWidget.draggable) _scaffold?._scheduleUpdate();
  }

  @override
  void dispose() {
    _scaffold?._unregister(_entry);
    _scroll?.dispose();
    _entry.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final scope = _SheetScope.of(context);
    final bottom = math.max(
      _entry.baseGeometry?.bottomInset ?? 0,
      MediaQuery.viewInsetsOf(context).bottom,
    );
    return _SheetContentScope(
      entry: _entry,
      child: _SheetContentLayout(
        entry: _entry,
        geometry: _entry.baseGeometry,
        anchors: _anchors,
        freeRanges: widget.freeRanges,
        physics: _entry.physics,
        held: widget.held,
        child: Scrollable(
          controller: _scroll,
          // Always, to catch drags for the sheet; without that, content that fits
          // doesn't respond to them at all.
          physics: widget.draggable
              ? const AlwaysScrollableScrollPhysics()
              : const ScrollPhysics(),
          // The footer and the held child are within the scrollable, so a drag on
          // either moves the sheet as one on the content does.
          viewportBuilder: (context, offset) => _SheetBody(
            entry: _entry,
            controller: scope.controller,
            extent: scope.state._paintExtent,
            offset: offset,
            held: widget.held,
            // Each a layer of its own, as the sheet moving repaints only where they
            // are.
            viewport: RepaintBoundary(
              child: _SheetViewport(
                entry: _entry,
                offset: offset,
                slivers: [
                  ...widget.slivers,
                  // Clear of the navigation bar or keyboard, and the footer, at the
                  // end.
                  SliverToBoxAdapter(
                    child: _SheetEndInset(entry: _entry, inset: bottom),
                  ),
                ],
              ),
            ),
            footer: switch (widget.footer) {
              final footer? => _SheetFooter(
                child: RepaintBoundary(child: footer),
              ),
              null => null,
            },
          ),
        ),
      ),
    );
  }
}

/// The sheet's view of a page's content: where its sections are, where its anchors
/// rest, and where the sheet last rested on it.
class _SheetEntry extends ChangeNotifier {
  _SheetEntry(TickerProvider vsync) {
    holds = _SheetHolds(vsync, this);
  }

  ModalRoute<Object?>? route;

  /// [SheetScaffold]'s geometry as the page sees it, but for its footer.
  SheetGeometry? baseGeometry;

  /// How tall the page's footer was last laid out, 0 if it hasn't one.
  double footerHeight = 0;

  /// [SheetScaffold]'s geometry as the page sees it.
  SheetGeometry? get geometry =>
      footerHeight == 0 ? baseGeometry : baseGeometry?.withFooter(footerHeight);

  SheetPhysics physics = const SheetPhysics();
  List<SheetAnchor> anchors = const [];
  List<SheetRange> freeRanges = const [];
  int initialAnchor = 0;
  Object? initialSection;
  double initialMaxFraction = 1;
  bool showsBottomBar = false;
  bool draggable = true;
  bool keepsHeight = false;

  /// What the bottom bar's [_SheetScaffoldState._barShown] is on this page.
  double get _barValue => showsBottomBar ? 1 : 0;

  final Set<_RenderSheetSection> sections = {};
  Map<Object, _Span> spans = const {};

  /// The page's slots, and which of their children are held (see [SheetContent.held]).
  final Set<_RenderSheetSlot> slots = {};
  late final _SheetHolds holds;

  _RenderSheetSlot? slotFor(Object id) =>
      slots.firstWhereOrNull((slot) => slot.id == id);

  /// Where the slot whose child the page holds sits in the content, if it's laid out.
  _Span? get _heldSpan => switch (holds.target) {
    final id? => slotFor(id)?.place?.span,
    null => null,
  };

  _RenderSheetBody? body;

  /// The space at the end of the content, which follows the footer's height.
  _RenderSheetEndInset? endInset;
  List<ResolvedAnchor> resolved = const [];
  List<ResolvedRange> ranges = const [];

  /// Where the sheet rested when this page was last current.
  _Rest? memory;

  _RenderSheetContent? content;
  bool _notifying = false;
  bool _relayingOut = false;
  bool _disposed = false;

  SheetAnchor? get initial => anchors.isEmpty
      ? null
      : anchors[initialAnchor.clamp(0, anchors.length - 1)];

  ResolvedAnchor? resolvedFor(SheetAnchor? anchor) => anchor == null
      ? null
      : resolved.firstWhereOrNull((a) => a.anchor == anchor);

  /// Where the sheet first rests on this page: at the initial anchor, or within a
  /// range, showing the content down to [initialSection]. Null if that can't be
  /// placed yet.
  _Rest? get initialRest {
    final anchor = switch (initial) {
      final initial? => _AtAnchor(initial),
      _ => null,
    };
    final section = initialSection;
    if (section == null) return anchor;
    final geometry = this.geometry;
    if (geometry == null) return null;
    final showing = _SectionAnchor(section)
        ._resolve(geometry, spans, _heldSpan);
    // Not until its section and every range are laid out, as a range placed later
    // might have it.
    if (showing == null || ranges.length < freeRanges.length) return null;
    final extent = math.min(
      showing,
      geometry.extentAtFraction(initialMaxFraction),
    );
    final range = ranges.firstWhereOrNull((r) => r.contains(extent));
    if (range == null) return anchor;
    final at = resolved.firstWhereOrNull(
      (a) => (a.extent - extent).abs() < _epsilon,
    );
    return at != null
        ? _AtAnchor(at.anchor)
        : _InRange(range.range, range.fractionOf(extent));
  }

  /// Where the sheet rests on this page: where it was left, or where it first rests.
  _Rest? get resting => memory ?? initialRest;

  /// The extent the sheet rests at on this page, or null if it can't be placed yet.
  double? get restingExtent => resting?.extentIn(resolved, ranges);

  bool get isAnimating => route?.animation?.isAnimating ?? false;

  /// The highest the sheet goes on this page: its highest anchor, or the top of a range
  /// above it.
  double? get topExtent {
    final anchor = resolved.lastOrNull?.extent;
    final range = ranges.map((r) => r.upper).maxOrNull;
    if (anchor == null || range == null) return anchor ?? range;
    return math.max(anchor, range);
  }

  /// The height the content's viewport takes: what the content shows as high as the
  /// sheet goes.
  double viewportHeight(double full) {
    final geometry = this.geometry;
    final top = topExtent;
    if (top == null || geometry == null) return full;
    return math.min(full, geometry.contentHeight(top));
  }

  /// Places the anchors for the current sections and geometry.
  void resolve() {
    final geometry = this.geometry;
    if (geometry == null) return;
    final held = _heldSpan;
    final placed = _resolveAnchors(anchors, geometry, spans, physics, held);
    final placedRanges = _resolveRanges(
      freeRanges,
      placed,
      geometry,
      spans,
      held,
    );
    if (listEquals(placed, resolved) && listEquals(placedRanges, ranges)) {
      return;
    }
    resolved = placed;
    ranges = placedRanges;
    _changed();
  }

  /// Finds where the sections sit in [viewport], after it's laid out.
  void measure(_RenderSheetViewport viewport) {
    final measured = <Object, _Span>{};
    for (final section in sections) {
      if (!section.hasSize ||
          RenderAbstractViewport.maybeOf(section) != viewport) {
        continue;
      }
      measured[section.id] = _spanOf(viewport, section);
    }
    if (!mapEquals(measured, spans)) {
      spans = measured;
      _changed();
    }
    // And the slots, which the held child is placed by.
    var moved = false;
    for (final slot in slots) {
      final place =
          slot.hasSize && RenderAbstractViewport.maybeOf(slot) == viewport
          ? _SlotPlace.of(viewport, slot)
          : null;
      if (place != slot.place) {
        slot.place = place;
        moved = true;
      }
    }
    assert(
      slots.map((slot) => slot.id).toSet().length == slots.length,
      'Two SheetSlots in one SheetContent have the same ID.',
    );
    if (moved) body?.markNeedsPaint();
    resolve();

    // The viewport's height follows the highest anchor, which may have moved.
    final content = this.content;
    if (content != null && !content.layingOut && !_relayingOut) {
      if ((viewportHeight(content.fullHeight) - content.childHeight).abs() >
          _epsilon) {
        _relayingOut = true;
        SchedulerBinding.instance.addPostFrameCallback((_) {
          _relayingOut = false;
          if (content.attached) content.markNeedsLayout();
        });
      }
    }
  }

  /// Tells listeners after this frame, as this is called from layout.
  void _changed() {
    if (_notifying) return;
    _notifying = true;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _notifying = false;
      if (!_disposed) notifyListeners();
    });
    SchedulerBinding.instance.ensureVisualUpdate();
  }

  @override
  void dispose() {
    _disposed = true;
    holds.dispose();
    super.dispose();
  }
}

/// Where [box] sits in [viewport]'s content, as an anchor shows down to it.
_Span _spanOf(RenderAbstractViewport viewport, RenderBox box) {
  var top = viewport.getOffsetToReveal(box, 0).offset;
  if (!top.isFinite) {
    // A box in a pinned sliver can't be revealed at the leading edge (Flutter says
    // infinity), but it can at the trailing edge, a viewport's height less its own
    // above where its top is.
    final bottom = viewport.getOffsetToReveal(box, 1).offset;
    top = bottom + (viewport as RenderBox).size.height - box.size.height;
  }
  return _Span(top, top + box.size.height);
}

class _SheetContentScope extends InheritedWidget {
  const _SheetContentScope({required this.entry, required super.child});

  final _SheetEntry entry;

  static _SheetEntry? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<_SheetContentScope>()?.entry;

  static _SheetEntry of(BuildContext context) {
    final entry = maybeOf(context);
    assert(
      entry != null,
      'SheetSection is only for use within a SheetContent.',
    );
    return entry!;
  }

  @override
  bool updateShouldNotify(_SheetContentScope oldWidget) =>
      entry != oldWidget.entry;
}

/// Sizes the content's viewport to what it shows at its highest anchor.
class _SheetContentLayout extends SingleChildRenderObjectWidget {
  const _SheetContentLayout({
    required this.entry,
    required this.geometry,
    required this.anchors,
    required this.freeRanges,
    required this.physics,
    required this.held,
    required super.child,
  });

  final _SheetEntry entry;
  final SheetGeometry? geometry;
  final List<SheetAnchor> anchors;
  final List<SheetRange> freeRanges;
  final SheetPhysics physics;
  final Object? held;

  @override
  _RenderSheetContent createRenderObject(BuildContext context) =>
      _RenderSheetContent(entry)
        ..config = (geometry, anchors, freeRanges, physics, held);

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderSheetContent renderObject,
  ) {
    renderObject
      ..entry = entry
      ..config = (geometry, anchors, freeRanges, physics, held);
  }
}

class _RenderSheetContent extends RenderProxyBox {
  _RenderSheetContent(this._entry) {
    _entry.content = this;
  }

  _SheetEntry get entry => _entry;
  _SheetEntry _entry;
  set entry(_SheetEntry value) {
    if (value == _entry) return;
    _entry = value..content = this;
    markNeedsLayout();
  }

  (SheetGeometry?, List<SheetAnchor>, List<SheetRange>, SheetPhysics, Object?)?
  _config;
  set config(
    (SheetGeometry?, List<SheetAnchor>, List<SheetRange>, SheetPhysics, Object?)
    value,
  ) {
    final old = _config;
    if (old != null &&
        old.$1 == value.$1 &&
        listEquals(old.$2, value.$2) &&
        listEquals(old.$3, value.$3) &&
        old.$4 == value.$4 &&
        old.$5 == value.$5) {
      return;
    }
    _config = value;
    markNeedsLayout();
  }

  bool layingOut = false;

  /// The height the content gets at the highest the sheet can be pulled.
  double fullHeight = 0;

  /// The height the viewport got.
  double childHeight = 0;

  @override
  void performLayout() {
    final child = this.child!;
    layingOut = true;
    fullHeight = constraints.maxHeight;
    entry.resolve();
    // Laying the viewport out measures the sections, which may move the highest anchor
    // and so the viewport's height.
    var height = entry.viewportHeight(fullHeight);
    child.layout(
      BoxConstraints.tightFor(width: constraints.maxWidth, height: height),
    );
    final measured = entry.viewportHeight(fullHeight);
    if ((measured - height).abs() > _epsilon) {
      height = measured;
      child.layout(
        BoxConstraints.tightFor(width: constraints.maxWidth, height: height),
      );
    }
    childHeight = height;
    size = constraints.biggest;
    layingOut = false;
  }
}

/// A viewport that measures the content's sections each time it's laid out.
class _SheetViewport extends Viewport {
  _SheetViewport({
    required this.entry,
    required super.offset,
    required super.slivers,
  });

  final _SheetEntry entry;

  @override
  RenderViewport createRenderObject(BuildContext context) {
    return _RenderSheetViewport(
      entry: entry,
      axisDirection: axisDirection,
      crossAxisDirection:
          crossAxisDirection ??
          Viewport.getDefaultCrossAxisDirection(context, axisDirection),
      offset: offset,
      scrollCacheExtent: scrollCacheExtent,
      clipBehavior: clipBehavior,
    );
  }

  @override
  void updateRenderObject(BuildContext context, RenderViewport renderObject) {
    super.updateRenderObject(context, renderObject);
    (renderObject as _RenderSheetViewport).entry = entry;
  }
}

class _RenderSheetViewport extends RenderViewport {
  _RenderSheetViewport({
    required this.entry,
    required super.axisDirection,
    required super.crossAxisDirection,
    required super.offset,
    super.scrollCacheExtent,
    super.clipBehavior,
  });

  _SheetEntry entry;

  /// How far down the content the point [within] the scroll extent of [sliver] is.
  double contentOffsetOf(RenderSliver sliver, double within) =>
      scrollOffsetOf(sliver, within);

  @override
  void performLayout() {
    super.performLayout();
    // Reading the sections' sizes is outside regular layout.
    invokeLayoutCallback<Constraints>((_) => entry.measure(this));
  }
}

/// Marks its child as a section of a [SheetContent], for anchors
/// ([SheetAnchor.section]) and [SheetView.reveal] to refer to by [id].
///
/// A section's height shouldn't depend on how far the sheet is pulled up, or its anchor
/// would move as the sheet moves towards it; width, position, opacity and the like can.
class SheetSection extends SingleChildRenderObjectWidget {
  const SheetSection({super.key, required this.id, super.child});

  final Object id;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderSheetSection(id, _SheetContentScope.of(context));

  @override
  void updateRenderObject(
    BuildContext context,
    // ignore: library_private_types_in_public_api
    _RenderSheetSection renderObject,
  ) {
    renderObject
      ..id = id
      ..entry = _SheetContentScope.of(context);
  }
}

class _RenderSheetSection extends RenderProxyBox {
  _RenderSheetSection(this._id, this._entry);

  Object get id => _id;
  Object _id;
  set id(Object value) {
    if (value == _id) return;
    _id = value;
    // Laying out again has the viewport measure again.
    markNeedsLayout();
  }

  _SheetEntry get entry => _entry;
  _SheetEntry _entry;
  set entry(_SheetEntry value) {
    if (value == _entry) return;
    if (attached) {
      _entry.sections.remove(this);
      value.sections.add(this);
    }
    _entry = value;
    markNeedsLayout();
  }

  @override
  void attach(PipelineOwner owner) {
    super.attach(owner);
    entry.sections.add(this);
  }

  @override
  void detach() {
    entry.sections.remove(this);
    super.detach();
  }
}

/// Where the sheet is, as seen from within it (or its scaffold).
class SheetView {
  const SheetView._(
    this.controller,
    this.geometry,
    this._entry, {
    required this._color,
    required this._translucent,
    required this.bottomBarCover,
  });

  final SheetController controller;
  final SheetGeometry geometry;
  final _SheetEntry? _entry;
  final Color _color;
  final bool _translucent;

  /// How much of the bottom of the window the scaffold's bottom bar covers, with the
  /// navigation bar under it: 0 while it's away, or if there isn't one.
  final double bottomBarCover;

  /// The sheet's color where it is now: for what pins to its content, say, to paint
  /// behind itself. Over a backdrop it's only as solid as the sheet
  /// ([SheetGeometry.solidity]).
  Color get surfaceColor => _translucent
      ? _color.withValues(alpha: _color.a * geometry.solidity(extent))
      : _color;

  /// How far the sheet is pulled up, in logical pixels from the bottom of the screen.
  double get extent => controller.extent;

  /// How far the sheet has turned into a full screen (see [SheetGeometry.expansion]).
  double get expansion => geometry.expansion(extent);

  /// The part of the window the sheet covers, and the bottom bar under a sheet
  /// that's lower than it.
  EdgeInsets get obscured => geometry.placement == SheetPlacement.docked
      ? geometry.obscured(extent)
      : geometry.obscured(math.max(extent, bottomBarCover));

  /// How much of the section [id] shows above the bottom inset, from 0 (none of it) to
  /// 1 (all of it). Always 0 outside a [SheetContent], or before the section's laid out.
  double reveal(Object id) {
    final span = _entry?.spans[id];
    if (span == null) return 0;
    final shown = geometry.contentShown(extent) - span.start;
    final height = span.end - span.start;
    if (height <= 0) return shown >= 0 ? 1 : 0;
    return (shown / height).clamp(0.0, 1.0);
  }

  /// How far the sheet is from [from] to [to], two of the content's anchors, from 0 at
  /// [from] (or beyond it) to 1 at [to] (or beyond it).
  double progress(SheetAnchor from, SheetAnchor to) {
    final a = _entry?.resolvedFor(from)?.extent;
    final b = _entry?.resolvedFor(to)?.extent;
    if (a == null || b == null) return 0;
    if (a == b) return extent >= b ? 1 : 0;
    return ((extent - a) / (b - a)).clamp(0.0, 1.0);
  }
}

/// Builds [builder] again whenever the sheet moves, with a [SheetView] of where it is.
///
/// For widgets that follow the sheet: keep what doesn't change in [child], which is
/// built once and passed back to [builder].
class SheetBuilder extends StatelessWidget {
  const SheetBuilder({super.key, required this.builder, this.child});

  final Widget Function(BuildContext context, SheetView sheet, Widget? child)
  builder;
  final Widget? child;

  @override
  Widget build(BuildContext context) {
    final scope = _SheetScope.of(context);
    final entry = _SheetContentScope.maybeOf(context);
    return ListenableBuilder(
      listenable: Listenable.merge([
        scope.controller,
        scope.state._barShown,
        ?entry,
      ]),
      builder: (context, child) => builder(
        context,
        SheetView._(
          scope.controller,
          entry?.geometry ?? scope.geometry,
          entry,
          color: scope.color,
          translucent: scope.translucent,
          bottomBarCover: scope.bottomBarCover,
        ),
        child,
      ),
      child: child,
    );
  }
}
