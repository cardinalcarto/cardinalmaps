part of '../sheet.dart';

/// Scrolls a page's content and the sheet as one: dragging up raises the sheet to its
/// highest anchor before scrolling the content, and dragging down scrolls the content
/// back to its top before lowering the sheet.
class _SheetScrollController extends ScrollController {
  _SheetScrollController(this.sheet, this.isActive);

  final SheetController sheet;

  /// Whether this content's page is the current one, the only one that moves the sheet.
  final bool Function() isActive;

  @override
  ScrollPosition createScrollPosition(
    ScrollPhysics physics,
    ScrollContext context,
    ScrollPosition? oldPosition,
  ) {
    return _SheetScrollPosition(
      sheet: sheet,
      isActive: isActive,
      physics: physics,
      context: context,
      oldPosition: oldPosition,
    );
  }
}

class _SheetScrollPosition extends ScrollPositionWithSingleContext {
  _SheetScrollPosition({
    required this.sheet,
    required this.isActive,
    required super.physics,
    required super.context,
    super.oldPosition,
  });

  final SheetController sheet;
  final bool Function() isActive;

  bool get _atTop => pixels <= minScrollExtent;

  bool _disposed = false;

  /// Scrolls on at [velocity] from where a glide of the sheet left off (see
  /// [SheetRange.carriesIntoScroll]).
  void _carryOn(double velocity) {
    if (_disposed || !isActive() || !hasContentDimensions) return;
    // Not held now, so this is the content's own fling.
    goBallistic(velocity);
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }

  @override
  ScrollHoldController hold(VoidCallback holdCancelCallback) {
    if (isActive()) sheet.hold(holder: this);
    return super.hold(holdCancelCallback);
  }

  @override
  Drag drag(DragStartDetails details, VoidCallback dragCancelCallback) {
    if (isActive()) sheet.hold(holder: this);
    return super.drag(details, dragCancelCallback);
  }

  @override
  void applyUserOffset(double delta) {
    // A positive delta is the finger moving down.
    if (!isActive() || !sheet.isDraggable) {
      return super.applyUserOffset(delta);
    }
    if (sheet.isTransitioning) return;
    if (delta < 0) {
      final rest = delta + sheet.dragBy(-delta);
      if (rest != 0) super.applyUserOffset(rest);
    } else {
      final scroll = math.min(delta, math.max(0.0, pixels - minScrollExtent));
      if (scroll > 0) super.applyUserOffset(scroll);
      if (delta > scroll) sheet.dragBy(scroll - delta);
    }
  }

  @override
  void goBallistic(double velocity) {
    // Called on letting go, but also when the content changes size while idle: only a
    // sheet this content's finger is holding settles, not one held by the drag handle.
    if (isActive() && sheet.isHeldBy(this)) {
      // A positive velocity is the content moving up, as the sheet does when raised.
      if (!sheet.isAtMax || (_atTop && velocity < 0)) {
        sheet.release(velocity, holder: this, carryOn: _carryOn);
        goIdle();
        return;
      }
      if (SheetLog.enabled && velocity != 0) {
        debugPrint(SheetLog.dragEnd(reported: velocity, released: 0));
      }
      sheet.release(0, holder: this);
    }
    super.goBallistic(velocity);
  }

  @override
  void pointerScroll(double delta) {
    // A positive delta scrolls the content down, as raising the sheet would.
    if (isActive() && sheet.isDraggable && !sheet.isTransitioning) {
      if (delta > 0 && !sheet.isAtMax ||
          delta < 0 && _atTop && sheet.extent > sheet._min) {
        sheet._nudge(delta);
        return;
      }
    }
    super.pointerScroll(delta);
  }
}
