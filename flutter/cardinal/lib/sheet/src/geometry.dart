part of '../sheet.dart';

/// Where the sheet sits on screen.
enum SheetPlacement {
  /// Full width along the bottom; at its top anchor it fills the screen, its corners
  /// squaring off as it slides under the status bar.
  bottom,

  /// A column along the start edge, for wide windows, stopping short of the status bar.
  docked,
}

/// The sheet's layout for a given window: everything that turns an extent (how far the
/// sheet is pulled up, in logical pixels from the bottom of the screen) into positions.
@immutable
class SheetGeometry {
  const SheetGeometry({
    required this.size,
    required this.padding,
    required this.placement,
    this.keyboard = 0,
    this.bottomBar = 0,
    this.footer = 0,
    this.handleHeight = 24,
    this.cornerRadius = 28,
    this.dockedWidth = 400,
    this.margin = 16,
  });

  /// Lays the sheet out for a window of [size] with system UI over [padding] and the
  /// [keyboard] that tall, docking it from [dockedBreakpoint] wide (Material 3's medium
  /// window class).
  factory SheetGeometry.forWindow(
    Size size,
    EdgeInsets padding, {
    double keyboard = 0,
    double dockedBreakpoint = 600,
  }) {
    return SheetGeometry(
      size: size,
      padding: padding,
      keyboard: keyboard,
      placement: size.width >= dockedBreakpoint
          ? SheetPlacement.docked
          : SheetPlacement.bottom,
    );
  }

  /// The area the sheet moves within: the whole window, under the system UI.
  final Size size;

  /// The system UI over [size] (status bar, navigation bar, cutouts): the sheet's
  /// content never rests under it.
  final EdgeInsets padding;

  final SheetPlacement placement;

  /// How tall the keyboard is over the bottom of [size], 0 while it's hidden. Anchors
  /// rest above it, and the sheet can't be pulled down behind it.
  final double keyboard;

  /// How tall the scaffold's bottom bar is over the navigation bar, 0 if the page
  /// doesn't show one (see [SheetContent.showsBottomBar]).
  final double bottomBar;

  /// How tall the page's footer is (see [SheetContent.footer]), 0 if it hasn't one. It
  /// sits over whatever else covers the bottom of the window.
  final double footer;

  /// What covers the bottom of the window: the keyboard, or else the navigation bar and
  /// the bottom bar, which the keyboard hides, and over either the page's footer. The
  /// content's anchors are measured from its top.
  double get bottomInset =>
      math.max(padding.bottom + bottomBar, keyboard) + footer;

  /// This geometry for a page that shows a bottom bar [height] tall.
  SheetGeometry withBottomBar(double height) => SheetGeometry(
    size: size,
    padding: padding,
    placement: placement,
    keyboard: keyboard,
    bottomBar: height,
    footer: footer,
    handleHeight: handleHeight,
    cornerRadius: cornerRadius,
    dockedWidth: dockedWidth,
    margin: margin,
  );

  /// This geometry for a page whose footer is [height] tall.
  SheetGeometry withFooter(double height) => SheetGeometry(
    size: size,
    padding: padding,
    placement: placement,
    keyboard: keyboard,
    bottomBar: bottomBar,
    footer: height,
    handleHeight: handleHeight,
    cornerRadius: cornerRadius,
    dockedWidth: dockedWidth,
    margin: margin,
  );

  /// The drag handle's strip at the top of the sheet.
  final double handleHeight;

  /// The radius of the sheet's top corners while it's below the status bar.
  final double cornerRadius;

  /// The widest a docked sheet gets.
  final double dockedWidth;

  /// The gap around a docked sheet.
  final double margin;

  bool get _docked => placement == SheetPlacement.docked;

  /// The sheet's left edge.
  double get left => _docked ? padding.left + margin : 0;

  /// The sheet's width.
  double get width => _docked
      ? math.max(
          0,
          math.min(dockedWidth, size.width - left - margin - padding.right),
        )
      : size.width;

  /// The space either side of the content, keeping it clear of side insets.
  EdgeInsets get contentPadding => _docked
      ? EdgeInsets.zero
      : EdgeInsets.only(left: padding.left, right: padding.right);

  /// The content's width.
  double get contentWidth => math.max(0, width - contentPadding.horizontal);

  /// The furthest the sheet can be pulled up.
  double get maxExtent =>
      _docked ? math.max(0, size.height - padding.top - margin) : size.height;

  /// How far below [maxExtent] the sheet starts squaring its corners and making room
  /// for the status bar. Wide enough that the content never moves faster than half
  /// again the sheet's speed.
  double get morphDistance =>
      math.max(48, 2 * (padding.top - handleHeight).abs());

  /// How far the sheet has turned into a full screen, from 0 below the morph to 1 at
  /// [maxExtent]. Always 0 when docked.
  double expansion(double extent) {
    if (_docked) return 0;
    return ((extent - (maxExtent - morphDistance)) / morphDistance).clamp(
      0.0,
      1.0,
    );
  }

  /// The corner radius at [extent].
  double cornerRadiusAt(double extent) =>
      cornerRadius * (1 - expansion(extent));

  /// How opaque a sheet over a backdrop is at [extent] (see [SheetScaffold.backdrop]):
  /// clear while it peeks, up to a third of the way up, and solid from halfway, where
  /// its content needs the contrast more than the map needs to show through.
  double solidity(double extent) {
    final clearTo = maxExtent / 3;
    final solidFrom = maxExtent / 2;
    return ((extent - clearTo) / (solidFrom - clearTo)).clamp(0.0, 1.0);
  }

  /// How far the content starts below the sheet's top edge at [extent]: below the drag
  /// handle, or below the status bar once expanded.
  double contentOffset(double extent) =>
      lerpDouble(handleHeight, padding.top, expansion(extent))!;

  /// The content's height on screen at [extent], from its top to the bottom of the screen.
  double contentHeight(double extent) =>
      math.max(0, extent - contentOffset(extent));

  /// The content's height on screen at [maxExtent], the most any content can show.
  double get fullContentHeight => contentHeight(maxExtent);

  /// The extent at which the content down to [y] (measured from its top) shows above
  /// the bottom inset.
  double extentShowing(double y) {
    final target = y + bottomInset;
    // Below the morph the content sits a handle's height below the top edge...
    final below = target + handleHeight;
    final morphStart = maxExtent - morphDistance;
    if (_docked || below <= morphStart) return math.min(below, maxExtent);
    // ...and within it, its offset moves linearly with the extent.
    final k = (padding.top - handleHeight) / morphDistance;
    return math.min(
      (target + handleHeight - k * morphStart) / (1 - k),
      maxExtent,
    );
  }

  /// How much of [y] (measured from the content's top) shows above the bottom inset at
  /// [extent]; the inverse of [extentShowing].
  double contentShown(double extent) =>
      extent - bottomInset - contentOffset(extent);

  /// The extent [fraction] of the way from the keyboard's top (the bottom, while it's
  /// hidden) to [maxExtent].
  double extentAtFraction(double fraction) {
    final floor = math.min(keyboard, maxExtent);
    return floor + (maxExtent - floor) * fraction.clamp(0.0, 1.0);
  }

  /// The part of the window the sheet covers at [extent], for the body to keep its
  /// focus clear of.
  EdgeInsets obscured(double extent) {
    if (_docked) {
      return extent > 0 ? EdgeInsets.only(left: left + width) : EdgeInsets.zero;
    }
    return EdgeInsets.only(bottom: math.min(extent, size.height));
  }

  @override
  bool operator ==(Object other) =>
      other is SheetGeometry &&
      other.size == size &&
      other.padding == padding &&
      other.placement == placement &&
      other.keyboard == keyboard &&
      other.bottomBar == bottomBar &&
      other.footer == footer &&
      other.handleHeight == handleHeight &&
      other.cornerRadius == cornerRadius &&
      other.dockedWidth == dockedWidth &&
      other.margin == margin;

  @override
  int get hashCode => Object.hash(
    size,
    padding,
    placement,
    keyboard,
    bottomBar,
    footer,
    handleHeight,
    cornerRadius,
    dockedWidth,
    margin,
  );
}
