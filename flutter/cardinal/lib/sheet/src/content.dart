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
    this.showsBottomBar = false,
    this.draggable = true,
    this.collapsible = false,
    required this.slivers,
  }) : assert(anchors.length > 0);

  /// Where the sheet can rest, in any order.
  final List<SheetAnchor> anchors;

  /// Stretches between pairs of [anchors] where the sheet rests wherever it's let go,
  /// gliding to a stop after a fling, rather than springing to an anchor.
  final List<SheetRange> freeRanges;

  /// The index into [anchors] the sheet moves to when this content first shows.
  final int initialAnchor;

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

  final List<Widget> slivers;

  @override
  State<SheetContent> createState() => _SheetContentState();
}

class _SheetContentState extends State<SheetContent> {
  final _SheetEntry _entry = _SheetEntry();
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
      ..geometry = _geometry(scope)
      ..physics = scope.controller.physics
      ..anchors = _anchors
      ..freeRanges = widget.freeRanges
      ..initialAnchor = widget.initialAnchor
      ..showsBottomBar = widget.showsBottomBar
      ..draggable = widget.draggable;
    _scaffold!._scheduleUpdate();
  }

  @override
  void didUpdateWidget(SheetContent oldWidget) {
    super.didUpdateWidget(oldWidget);
    _entry
      ..geometry = _geometry(_SheetScope.of(context))
      ..anchors = _anchors
      ..freeRanges = widget.freeRanges
      ..initialAnchor = widget.initialAnchor
      ..showsBottomBar = widget.showsBottomBar
      ..draggable = widget.draggable;
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
    final bottom = math.max(
      _entry.geometry?.bottomInset ?? 0,
      MediaQuery.viewInsetsOf(context).bottom,
    );
    return _SheetContentScope(
      entry: _entry,
      child: _SheetContentLayout(
        entry: _entry,
        geometry: _entry.geometry,
        anchors: _anchors,
        freeRanges: widget.freeRanges,
        physics: _entry.physics,
        child: Scrollable(
          controller: _scroll,
          // Always, to catch drags for the sheet; without that, content that fits
          // doesn't respond to them at all.
          physics: widget.draggable
              ? const AlwaysScrollableScrollPhysics()
              : const ScrollPhysics(),
          viewportBuilder: (context, offset) => _SheetViewport(
            entry: _entry,
            offset: offset,
            slivers: [
              ...widget.slivers,
              // Clear of the navigation bar or keyboard at the end.
              SliverToBoxAdapter(child: SizedBox(height: bottom)),
            ],
          ),
        ),
      ),
    );
  }
}

/// The sheet's view of a page's content: where its sections are, where its anchors
/// rest, and where the sheet last rested on it.
class _SheetEntry extends ChangeNotifier {
  ModalRoute<Object?>? route;
  SheetGeometry? geometry;
  SheetPhysics physics = const SheetPhysics();
  List<SheetAnchor> anchors = const [];
  List<SheetRange> freeRanges = const [];
  int initialAnchor = 0;
  bool showsBottomBar = false;
  bool draggable = true;

  /// What the bottom bar's [_SheetScaffoldState._barShown] is on this page.
  double get _barValue => showsBottomBar ? 1 : 0;

  final Set<_RenderSheetSection> sections = {};
  Map<Object, _Span> spans = const {};
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

  /// Where the sheet rests on this page: where it was left, or the initial anchor.
  _Rest? get resting =>
      memory ??
      switch (initial) {
        final initial? => _AtAnchor(initial),
        _ => null,
      };

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
    final placed = _resolveAnchors(anchors, geometry, spans, physics);
    final placedRanges = _resolveRanges(freeRanges, placed, geometry, spans);
    if (listEquals(placed, resolved) && listEquals(placedRanges, ranges)) {
      return;
    }
    resolved = placed;
    ranges = placedRanges;
    _changed();
  }

  /// Finds where the sections sit in [viewport], after it's laid out.
  void measure(RenderAbstractViewport viewport) {
    final measured = <Object, _Span>{};
    for (final section in sections) {
      if (!section.hasSize ||
          RenderAbstractViewport.maybeOf(section) != viewport) {
        continue;
      }
      var top = viewport.getOffsetToReveal(section, 0).offset;
      if (!top.isFinite) {
        // A section in a pinned sliver can't be revealed at the leading edge (Flutter
        // says infinity), but it can at the trailing edge, a viewport's height less
        // its own above where its top is.
        final bottom = viewport.getOffsetToReveal(section, 1).offset;
        top =
            bottom + (viewport as RenderBox).size.height - section.size.height;
      }
      measured[section.id] = _Span(top, top + section.size.height);
    }
    if (!mapEquals(measured, spans)) {
      spans = measured;
      _changed();
    }
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
    super.dispose();
  }
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
    required super.child,
  });

  final _SheetEntry entry;
  final SheetGeometry? geometry;
  final List<SheetAnchor> anchors;
  final List<SheetRange> freeRanges;
  final SheetPhysics physics;

  @override
  _RenderSheetContent createRenderObject(BuildContext context) =>
      _RenderSheetContent(entry)
        ..config = (geometry, anchors, freeRanges, physics);

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderSheetContent renderObject,
  ) {
    renderObject
      ..entry = entry
      ..config = (geometry, anchors, freeRanges, physics);
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

  (SheetGeometry?, List<SheetAnchor>, List<SheetRange>, SheetPhysics)? _config;
  set config(
    (SheetGeometry?, List<SheetAnchor>, List<SheetRange>, SheetPhysics) value,
  ) {
    final old = _config;
    if (old != null &&
        old.$1 == value.$1 &&
        listEquals(old.$2, value.$2) &&
        listEquals(old.$3, value.$3) &&
        old.$4 == value.$4) {
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
