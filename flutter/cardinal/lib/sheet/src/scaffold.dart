part of '../sheet.dart';

/// Lays a sheet over [body], with [floating] between them.
///
/// [sheet] is the sheet's navigator, whose pages are [SheetPage]s showing
/// [SheetContent]: pushing one swaps the sheet's content and moves the one sheet to the
/// new page's anchor, and popping moves it back to where the page below was left.
///
/// Below [dockedBreakpoint] wide the sheet runs full width along the bottom and turns
/// into a full screen at its top; from there it docks as a column at the start edge.
class SheetScaffold extends StatefulWidget {
  const SheetScaffold({
    super.key,
    required this.body,
    required this.sheet,
    this.floating,
    this.bottomBar,
    this.bottomBarHeight = 56,
    this.controller,
    this.physics = const SheetPhysics(),
    this.dockedBreakpoint = 600,
    this.color,
    this.elevation = 3,
    this.bodyOverlayStyle,
  });

  /// What the sheet lies over: the map.
  final Widget body;

  /// The sheet's navigator.
  final Widget sheet;

  /// Between [body] and the sheet: buttons that ride above the sheet, say. Fills the
  /// scaffold; a [SheetBuilder] can follow the sheet.
  final Widget? floating;

  /// A bar along the bottom of the sheet, over it and the navigation bar, on the pages
  /// that show it ([SheetContent.showsBottomBar]). It paints its own background behind
  /// the navigation bar.
  final Widget? bottomBar;

  /// How tall [bottomBar] is over the navigation bar.
  final double bottomBarHeight;

  /// Moves the sheet; the scaffold makes its own if null.
  final SheetController? controller;

  /// The physics of the scaffold's own controller.
  final SheetPhysics physics;

  final double dockedBreakpoint;

  /// The sheet's color; [ColorScheme.surfaceContainerLow] if null.
  final Color? color;

  final double elevation;

  /// The system bars' style over [body]; while the sheet covers the status bar, the
  /// style follows the sheet's color instead.
  final SystemUiOverlayStyle? bodyOverlayStyle;

  /// The controller of the nearest sheet.
  static SheetController controllerOf(BuildContext context) =>
      _SheetScope.of(context).controller;

  /// Where the nearest sheet is, without listening for it to move.
  static SheetView viewOf(BuildContext context) {
    final scope = _SheetScope.of(context);
    final entry = _SheetContentScope.maybeOf(context);
    return SheetView._(
      scope.controller,
      entry?.geometry ?? scope.geometry,
      entry,
    );
  }

  /// How tall the nearest sheet's bottom bar is, 0 if it hasn't one.
  static double bottomBarHeightOf(BuildContext context) =>
      _SheetScope.of(context).bottomBarHeight;

  @override
  State<SheetScaffold> createState() => _SheetScaffoldState();
}

class _SheetScaffoldState extends State<SheetScaffold>
    with TickerProviderStateMixin {
  SheetController? _ownController;
  SheetController get _controller => widget.controller ?? _ownController!;

  SheetGeometry? _geometry;
  ModalRoute<Object?>? _route;

  /// The pages' contents, in the order they were shown (so roughly stack order).
  final List<_SheetEntry> _entries = [];
  final Map<_SheetEntry, Animation<double>> _animations = {};

  /// The content of the current page, which the sheet's anchors come from.
  _SheetEntry? _active;

  /// The page transition the sheet is following: from [under] to [top], or if there's
  /// nothing under it, from where the sheet and bottom bar were when it began.
  ({_SheetEntry top, _SheetEntry? under, double? extent, double bar})?
  _transition;

  /// How far the bottom bar is showing, from 0 (below the screen) to 1.
  final _barShown = ValueNotifier<double>(0);

  bool _updateScheduled = false;

  /// Whether the route holding the scaffold is current, for back gestures.
  bool get _isRouteCurrent => _route?.isCurrent ?? true;

  @override
  void initState() {
    super.initState();
    if (widget.controller == null) {
      _ownController = SheetController(vsync: this, physics: widget.physics);
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _route = ModalRoute.of(context);
  }

  @override
  void didUpdateWidget(SheetScaffold oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.controller == null && _ownController == null) {
      _ownController = SheetController(vsync: this, physics: widget.physics);
    }
    _ownController?.physics = widget.physics;
  }

  @override
  void dispose() {
    for (final MapEntry(key: entry, value: animation) in _animations.entries) {
      _unlisten(entry, animation);
    }
    _ownController?.dispose();
    _barShown.dispose();
    super.dispose();
  }

  void _register(_SheetEntry entry) {
    _entries.add(entry);
    _scheduleUpdate();
  }

  void _unregister(_SheetEntry entry) {
    _entries.remove(entry);
    if (_animations.remove(entry) case final animation?) {
      _unlisten(entry, animation);
    }
    if (_active == entry) _active = null;
    if (_transition?.top == entry || _transition?.under == entry) {
      _transition = null;
    }
    _scheduleUpdate();
  }

  void _unlisten(_SheetEntry entry, Animation<double> animation) {
    animation
      ..removeListener(_requestUpdate)
      ..removeStatusListener(_handleStatus);
    entry.removeListener(_requestUpdate);
  }

  void _handleStatus(AnimationStatus status) => _requestUpdate();

  /// Updates now, unless the framework is building or laying out.
  void _requestUpdate() {
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      _scheduleUpdate();
    } else {
      _update();
    }
  }

  void _scheduleUpdate() {
    if (_updateScheduled) return;
    _updateScheduled = true;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      _updateScheduled = false;
      if (mounted) _update();
    });
    SchedulerBinding.instance.ensureVisualUpdate();
  }

  /// Listens to each page's content and route animation.
  void _listen() {
    for (final entry in _entries) {
      final animation = entry.route?.animation;
      final old = _animations[entry];
      if (animation == old) continue;
      if (old != null) _unlisten(entry, old);
      if (animation == null) continue;
      _animations[entry] = animation;
      animation
        ..addListener(_requestUpdate)
        ..addStatusListener(_handleStatus);
      entry.addListener(_requestUpdate);
    }
  }

  /// Finds the current page and any transition, and moves the sheet to match.
  void _update() {
    _listen();
    final controller = _controller;
    final current = _entries.lastWhereOrNull(
      (e) => e.route?.isCurrent ?? false,
    );
    final top = _entries.lastWhereOrNull((e) => e.isAnimating);

    if (top == null) {
      _setActive(current);
      if (current != null) _barShown.value = current._barValue;
      if (_transition != null) {
        _transition = null;
        controller._endTransition(current?.memory ?? current?.initial);
      }
      return;
    }

    // A page being pushed (or dragged back) is current, and lies over the page below
    // it; a page being popped lies over the page now current.
    final _SheetEntry? under;
    if (top.route!.isCurrent) {
      final below = _entries.sublist(0, _entries.indexOf(top));
      under = below.lastWhereOrNull((e) => e.route?.isActive ?? false);
    } else {
      under = current;
    }
    if (_transition?.top != top || _transition?.under != under) {
      // Remember where the sheet was left on the page it's leaving. A page only just
      // pushed may already be active (an update can come before its transition starts),
      // but the sheet is still where the page below left it.
      final arriving = _active == top && top.memory == null;
      if (!controller.isTransitioning) {
        if (!arriving) {
          _active?.memory = controller._nearestAnchor;
        } else if (under != null) {
          under.memory = controller._nearestOf(under.resolved) ?? under.memory;
        }
      }
      top.memory ??= top.initial;
      under?.memory ??= under.initial;
      _transition = (
        top: top,
        under: under,
        extent: controller.hasExtent ? controller.extent : null,
        bar: _barShown.value,
      );
      controller._beginTransition();
    }
    _setActive(current ?? top);

    // Either page's anchors may not be laid out yet; until they are, hold still.
    final to = top.resting?.extent;
    final from = under?.resting?.extent ?? _transition!.extent;
    final route = top.route!;
    final t = route is _SheetPageRoute
        ? route._arrival
        : route.animation!.value;
    final extent = from == null
        ? to
        : (to == null ? from : lerpDouble(from, to, t));
    if (extent != null) controller._follow(extent);
    _barShown.value = lerpDouble(
      under?._barValue ?? _transition!.bar,
      top._barValue,
      t,
    )!;
  }

  void _setActive(_SheetEntry? entry) {
    _active = entry;
    final geometry = _geometry;
    if (entry == null || geometry == null) return;
    _controller._setAnchors(
      entry.resolved,
      geometry.maxExtent,
      floor: geometry.keyboard,
      draggable: entry.draggable,
    );
    if (!_controller.hasExtent && entry.resting != null) {
      _controller._place(entry.resting!);
    }
  }

  /// Where to paint the sheet: before it's placed, where the current page will place it.
  double _paintExtent() {
    if (_controller.hasExtent) return _controller.extent;
    final current = _entries.lastWhereOrNull(
      (e) => e.route?.isCurrent ?? false,
    );
    return current?.resting?.extent ?? 0;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final color = widget.color ?? theme.colorScheme.surfaceContainerLow;
    final padding = MediaQuery.viewPaddingOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    final controller = _controller;
    return LayoutBuilder(
      builder: (context, constraints) {
        final geometry = SheetGeometry.forWindow(
          constraints.biggest,
          padding,
          keyboard: keyboard,
          dockedBreakpoint: widget.dockedBreakpoint,
        );
        if (geometry != _geometry) {
          _geometry = geometry;
          _scheduleUpdate();
        }
        return _SheetScope(
          state: this,
          controller: controller,
          geometry: geometry,
          bottomBarHeight: widget.bottomBar == null
              ? 0
              : widget.bottomBarHeight,
          color: color,
          child: _SystemBarsStyle(
            controller: controller,
            geometry: geometry,
            sheetColor: color,
            bodyStyle: widget.bodyOverlayStyle,
            child: Stack(
              fit: StackFit.expand,
              children: [
                RepaintBoundary(child: widget.body),
                ?widget.floating,
                RepaintBoundary(
                  child: GestureDetector(
                    onVerticalDragStart: (_) => controller.hold(),
                    onVerticalDragUpdate: (details) =>
                        controller.dragBy(-details.primaryDelta!),
                    onVerticalDragEnd: (details) =>
                        controller.release(-details.primaryVelocity!),
                    onVerticalDragCancel: () => controller.release(0),
                    child: _SheetSurface(
                      controller: controller,
                      geometry: geometry,
                      extent: _paintExtent,
                      color: color,
                      shadowColor: theme.colorScheme.shadow,
                      elevation: widget.elevation,
                      handle: _DragHandle(controller: controller),
                      content: RepaintBoundary(
                        child: Material(
                          type: MaterialType.transparency,
                          child: MediaQuery.removePadding(
                            context: context,
                            removeTop: true,
                            removeBottom: true,
                            removeLeft: true,
                            removeRight: true,
                            child: ScrollConfiguration(
                              // Drag with a mouse too, as with a finger.
                              behavior: ScrollConfiguration.of(context)
                                  .copyWith(
                                    dragDevices: PointerDeviceKind.values
                                        .toSet(),
                                    scrollbars: false,
                                  ),
                              child: widget.sheet,
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
                if (widget.bottomBar case final bar?)
                  _BottomBar(
                    shown: _barShown,
                    geometry: geometry,
                    height: widget.bottomBarHeight,
                    child: bar,
                  ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// The scaffold's bottom bar, along the bottom of the sheet: as much of it as [shown]
/// says rises from below the screen.
class _BottomBar extends StatelessWidget {
  const _BottomBar({
    required this.shown,
    required this.geometry,
    required this.height,
    required this.child,
  });

  final ValueListenable<double> shown;
  final SheetGeometry geometry;
  final double height;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final total = height + geometry.padding.bottom;
    return ValueListenableBuilder(
      valueListenable: shown,
      child: child,
      builder: (context, shown, child) => Positioned(
        left: geometry.left,
        width: geometry.width,
        height: total,
        bottom: total * (shown - 1),
        child: Offstage(offstage: shown == 0, child: child),
      ),
    );
  }
}

class _SheetScope extends InheritedWidget {
  const _SheetScope({
    required this.state,
    required this.controller,
    required this.geometry,
    required this.bottomBarHeight,
    required this.color,
    required super.child,
  });

  final _SheetScaffoldState state;
  final SheetController controller;
  final SheetGeometry geometry;

  /// How tall the scaffold's bottom bar is, 0 if it hasn't one.
  final double bottomBarHeight;

  /// The sheet's color, which pages paint behind themselves to cover the page below.
  final Color color;

  static _SheetScope of(BuildContext context) {
    final scope = context.dependOnInheritedWidgetOfExactType<_SheetScope>();
    assert(scope != null, 'No SheetScaffold above this widget.');
    return scope!;
  }

  @override
  bool updateShouldNotify(_SheetScope oldWidget) =>
      state != oldWidget.state ||
      controller != oldWidget.controller ||
      geometry != oldWidget.geometry ||
      bottomBarHeight != oldWidget.bottomBarHeight ||
      color != oldWidget.color;
}

/// Styles the system bars for whatever's under them: the body, or the sheet once it
/// covers the status bar.
class _SystemBarsStyle extends StatefulWidget {
  const _SystemBarsStyle({
    required this.controller,
    required this.geometry,
    required this.sheetColor,
    required this.bodyStyle,
    required this.child,
  });

  final SheetController controller;
  final SheetGeometry geometry;
  final Color sheetColor;
  final SystemUiOverlayStyle? bodyStyle;
  final Widget child;

  @override
  State<_SystemBarsStyle> createState() => _SystemBarsStyleState();
}

class _SystemBarsStyleState extends State<_SystemBarsStyle> {
  bool _covered = false;

  bool get _isCovered =>
      widget.geometry.expansion(widget.controller.extent) > 0.5;

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_handleChange);
    _covered = _isCovered;
  }

  @override
  void didUpdateWidget(_SystemBarsStyle oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.controller != oldWidget.controller) {
      oldWidget.controller.removeListener(_handleChange);
      widget.controller.addListener(_handleChange);
    }
    _covered = _isCovered;
  }

  @override
  void dispose() {
    widget.controller.removeListener(_handleChange);
    super.dispose();
  }

  void _handleChange() {
    if (_isCovered != _covered) setState(() => _covered = _isCovered);
  }

  static SystemUiOverlayStyle _over(Brightness brightness) {
    final style = brightness == Brightness.dark
        ? SystemUiOverlayStyle.light
        : SystemUiOverlayStyle.dark;
    return style.copyWith(
      statusBarColor: Colors.transparent,
      systemNavigationBarColor: Colors.transparent,
      systemNavigationBarDividerColor: Colors.transparent,
      systemNavigationBarContrastEnforced: false,
    );
  }

  @override
  Widget build(BuildContext context) {
    final sheet = _over(
      ThemeData.estimateBrightnessForColor(widget.sheetColor),
    );
    final body = widget.bodyStyle ?? _over(Theme.of(context).brightness);
    // The navigation bar is over the sheet whenever it shows.
    final style = (_covered ? sheet : body).copyWith(
      systemNavigationBarIconBrightness:
          sheet.systemNavigationBarIconBrightness,
    );
    return AnnotatedRegion<SystemUiOverlayStyle>(
      value: style,
      child: widget.child,
    );
  }
}

class _DragHandle extends StatelessWidget {
  const _DragHandle({required this.controller});

  final SheetController controller;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.onSurfaceVariant
        .withValues(alpha: 0.4);
    return Semantics(
      button: true,
      label: 'Resize sheet',
      onTap: controller.cycle,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: controller.cycle,
        child: Center(
          child: Container(
            width: 32,
            height: 4,
            decoration: BoxDecoration(
              color: color,
              borderRadius: BorderRadius.circular(2),
            ),
          ),
        ),
      ),
    );
  }
}

enum _SurfaceSlot { handle, content }

/// The sheet itself: a rounded surface at the bottom of the scaffold, as tall as the
/// sheet's extent, with the drag handle at its top and the pages' content below.
///
/// Moving the sheet only repaints it: the content keeps its layout, and is clipped to
/// the surface where it extends below the screen.
class _SheetSurface
    extends SlottedMultiChildRenderObjectWidget<_SurfaceSlot, RenderBox> {
  const _SheetSurface({
    required this.controller,
    required this.geometry,
    required this.extent,
    required this.color,
    required this.shadowColor,
    required this.elevation,
    required this.handle,
    required this.content,
  });

  final SheetController controller;
  final SheetGeometry geometry;
  final double Function() extent;
  final Color color;
  final Color shadowColor;
  final double elevation;
  final Widget handle;
  final Widget content;

  @override
  Iterable<_SurfaceSlot> get slots => _SurfaceSlot.values;

  @override
  Widget? childForSlot(_SurfaceSlot slot) => switch (slot) {
    _SurfaceSlot.handle => handle,
    _SurfaceSlot.content => content,
  };

  @override
  _RenderSheetSurface createRenderObject(BuildContext context) =>
      _RenderSheetSurface(
        controller: controller,
        geometry: geometry,
        extent: extent,
        color: color,
        shadowColor: shadowColor,
        elevation: elevation,
      );

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderSheetSurface renderObject,
  ) {
    renderObject
      ..controller = controller
      ..geometry = geometry
      ..extent = extent
      ..color = color
      ..shadowColor = shadowColor
      ..elevation = elevation;
  }
}

class _RenderSheetSurface extends RenderBox
    with SlottedContainerRenderObjectMixin<_SurfaceSlot, RenderBox> {
  _RenderSheetSurface({
    required this._controller,
    required this._geometry,
    required this.extent,
    required this._color,
    required this._shadowColor,
    required this._elevation,
  });

  SheetController get controller => _controller;
  SheetController _controller;
  set controller(SheetController value) {
    if (value == _controller) return;
    if (attached) {
      _controller.removeListener(markNeedsPaint);
      value.addListener(markNeedsPaint);
    }
    _controller = value;
    markNeedsPaint();
  }

  SheetGeometry get geometry => _geometry;
  SheetGeometry _geometry;
  set geometry(SheetGeometry value) {
    if (value == _geometry) return;
    _geometry = value;
    markNeedsLayout();
  }

  double Function() extent;

  Color get color => _color;
  Color _color;
  set color(Color value) {
    if (value == _color) return;
    _color = value;
    markNeedsPaint();
  }

  Color get shadowColor => _shadowColor;
  Color _shadowColor;
  set shadowColor(Color value) {
    if (value == _shadowColor) return;
    _shadowColor = value;
    markNeedsPaint();
  }

  double get elevation => _elevation;
  double _elevation;
  set elevation(double value) {
    if (value == _elevation) return;
    _elevation = value;
    markNeedsPaint();
  }

  RenderBox? get _handle => childForSlot(_SurfaceSlot.handle);
  RenderBox? get _content => childForSlot(_SurfaceSlot.content);

  final LayerHandle<ClipRRectLayer> _clip = LayerHandle();
  final LayerHandle<OpacityLayer> _opacity = LayerHandle();

  @override
  void attach(PipelineOwner owner) {
    super.attach(owner);
    _controller.addListener(markNeedsPaint);
  }

  @override
  void detach() {
    _controller.removeListener(markNeedsPaint);
    super.detach();
  }

  @override
  void dispose() {
    _clip.layer = null;
    _opacity.layer = null;
    super.dispose();
  }

  double get _extent => extent().clamp(0.0, size.height);

  Offset _handleOffset(double extent) =>
      Offset(geometry.left, size.height - extent);

  Offset _contentOffset(double extent) => Offset(
    geometry.left + geometry.contentPadding.left,
    size.height - extent + geometry.contentOffset(extent),
  );

  RRect _surface(double extent) {
    final radius = Radius.circular(geometry.cornerRadiusAt(extent));
    return RRect.fromLTRBAndCorners(
      geometry.left,
      size.height - extent,
      geometry.left + geometry.width,
      size.height,
      topLeft: radius,
      topRight: radius,
    );
  }

  @override
  bool get sizedByParent => true;

  @override
  Size computeDryLayout(BoxConstraints constraints) => constraints.biggest;

  @override
  void performLayout() {
    _handle?.layout(
      BoxConstraints.tight(Size(geometry.width, geometry.handleHeight)),
    );
    _content?.layout(
      BoxConstraints.tight(
        Size(geometry.contentWidth, geometry.fullContentHeight),
      ),
    );
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    final extent = _extent;
    if (extent <= 0) {
      _clip.layer = null;
      _opacity.layer = null;
      return;
    }
    final surface = _surface(extent);
    final canvas = context.canvas;
    if (elevation > 0) {
      canvas.drawShadow(
        Path()..addRRect(surface.shift(offset)),
        shadowColor,
        elevation,
        false,
      );
    }
    canvas.drawRRect(surface.shift(offset), Paint()..color = color);
    _clip.layer = context.pushClipRRect(
      needsCompositing,
      offset,
      surface.outerRect,
      surface,
      (context, offset) {
        if (_content case final content?) {
          context.paintChild(content, offset + _contentOffset(extent));
        }
        final alpha = (255 * (1 - geometry.expansion(extent))).round();
        if (_handle case final handle? when alpha > 0) {
          _opacity.layer = context.pushOpacity(
            offset + _handleOffset(extent),
            alpha,
            (context, offset) => context.paintChild(handle, offset),
            oldLayer: _opacity.layer,
          );
        } else {
          _opacity.layer = null;
        }
      },
      oldLayer: _clip.layer,
    );
  }

  @override
  bool hitTest(BoxHitTestResult result, {required Offset position}) {
    final extent = _extent;
    if (extent <= 0 || !_surface(extent).contains(position)) return false;
    hitTestChildren(result, position: position);
    result.add(BoxHitTestEntry(this, position));
    return true;
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) {
    final extent = _extent;
    bool hit(RenderBox? child, Offset offset) =>
        child != null &&
        result.addWithPaintOffset(
          offset: offset,
          position: position,
          hitTest: (result, position) =>
              child.hitTest(result, position: position),
        );
    // The handle fades out over the content as the sheet expands.
    return geometry.expansion(extent) < 0.5
        ? hit(_handle, _handleOffset(extent)) ||
              hit(_content, _contentOffset(extent))
        : hit(_content, _contentOffset(extent)) ||
              hit(_handle, _handleOffset(extent));
  }

  @override
  void applyPaintTransform(RenderBox child, Matrix4 transform) {
    final extent = _extent;
    final offset = child == _handle
        ? _handleOffset(extent)
        : _contentOffset(extent);
    transform.translateByDouble(offset.dx, offset.dy, 0, 1);
  }
}
