part of '../sheet.dart';

/// Somewhere the sheet can come to rest.
///
/// Every anchor has a gravity: how hard it holds on to a fling passing over it. A fling
/// is caught by the first anchor it reaches at less than that anchor's gravity, so a
/// strong anchor stops all but the hardest flings and an anchor with no gravity only
/// stops flings that run out of speed near it. The fling always reaches the next anchor
/// in its direction, however weak.
@immutable
sealed class SheetAnchor {
  const SheetAnchor({this.gravity});

  /// Rests with the section [id] (see [SheetSection]) and everything above it showing.
  const factory SheetAnchor.section(Object id, {double? gravity}) =
      _SectionAnchor;

  /// Rests at [fraction] of the furthest the sheet can be pulled up, measured from the
  /// keyboard's top while it shows.
  const factory SheetAnchor.fraction(double fraction, {double? gravity}) =
      _FractionAnchor;

  /// Rests as far up as the sheet goes: the full screen, or the top of a docked column.
  const factory SheetAnchor.full({double? gravity}) = _FullAnchor;

  /// Rests out of sight below the bottom of the window, on a [SheetContent.collapsible]
  /// page, until [SheetController.expand] brings it back. It can't be placed while the
  /// keyboard shows, as the sheet can't go behind the keyboard.
  const factory SheetAnchor.collapsed({double? gravity}) = _CollapsedAnchor;

  /// The speed, in logical pixels per second, below which a fling passing over this
  /// anchor stops here. Null takes [SheetPhysics.gravity].
  final double? gravity;

  /// The extent this anchor rests at, or null if it can't be placed yet (its section
  /// hasn't been laid out).
  double? _resolve(SheetGeometry geometry, Map<Object, _Span> sections);
}

final class _SectionAnchor extends SheetAnchor {
  const _SectionAnchor(this.id, {super.gravity});

  final Object id;

  @override
  double? _resolve(SheetGeometry geometry, Map<Object, _Span> sections) {
    final span = sections[id];
    return span == null ? null : geometry.extentShowing(span.end);
  }

  @override
  bool operator ==(Object other) =>
      other is _SectionAnchor && other.id == id && other.gravity == gravity;

  @override
  int get hashCode => Object.hash(_SectionAnchor, id, gravity);

  @override
  String toString() => 'SheetAnchor.section($id)';
}

final class _FractionAnchor extends SheetAnchor {
  const _FractionAnchor(this.fraction, {super.gravity});

  final double fraction;

  @override
  double _resolve(SheetGeometry geometry, Map<Object, _Span> sections) =>
      geometry.extentAtFraction(fraction);

  @override
  bool operator ==(Object other) =>
      other is _FractionAnchor &&
      other.fraction == fraction &&
      other.gravity == gravity;

  @override
  int get hashCode => Object.hash(_FractionAnchor, fraction, gravity);

  @override
  String toString() => 'SheetAnchor.fraction($fraction)';
}

final class _FullAnchor extends SheetAnchor {
  const _FullAnchor({super.gravity});

  @override
  double _resolve(SheetGeometry geometry, Map<Object, _Span> sections) =>
      geometry.maxExtent;

  @override
  bool operator ==(Object other) =>
      other is _FullAnchor && other.gravity == gravity;

  @override
  int get hashCode => Object.hash(_FullAnchor, gravity);

  @override
  String toString() => 'SheetAnchor.full()';
}

final class _CollapsedAnchor extends SheetAnchor {
  const _CollapsedAnchor({super.gravity});

  @override
  double? _resolve(SheetGeometry geometry, Map<Object, _Span> sections) =>
      geometry.keyboard > 0 ? null : 0;

  @override
  bool operator ==(Object other) =>
      other is _CollapsedAnchor && other.gravity == gravity;

  @override
  int get hashCode => Object.hash(_CollapsedAnchor, gravity);

  @override
  String toString() => 'SheetAnchor.collapsed()';
}

/// An anchor placed for the current layout.
@immutable
class ResolvedAnchor {
  const ResolvedAnchor(this.anchor, this.extent, this.gravity);

  final SheetAnchor anchor;
  final double extent;
  final double gravity;

  @override
  bool operator ==(Object other) =>
      other is ResolvedAnchor &&
      other.anchor == anchor &&
      other.extent == extent &&
      other.gravity == gravity;

  @override
  int get hashCode => Object.hash(anchor, extent, gravity);

  @override
  String toString() =>
      'ResolvedAnchor($anchor at ${extent.toStringAsFixed(1)})';
}

/// Where a section sits in its content, from the content's top.
@immutable
class _Span {
  const _Span(this.start, this.end);

  final double start;
  final double end;

  @override
  bool operator ==(Object other) =>
      other is _Span && other.start == start && other.end == end;

  @override
  int get hashCode => Object.hash(start, end);
}

/// Places [anchors], lowest first, dropping any that can't be placed yet.
List<ResolvedAnchor> _resolveAnchors(
  List<SheetAnchor> anchors,
  SheetGeometry geometry,
  Map<Object, _Span> sections,
  SheetPhysics physics,
) {
  final resolved = <ResolvedAnchor>[
    for (final anchor in anchors)
      if (anchor._resolve(geometry, sections) case final extent?)
        ResolvedAnchor(anchor, extent, anchor.gravity ?? physics.gravity),
  ];
  return resolved..sort((a, b) => a.extent.compareTo(b.extent));
}

/// A stretch of the sheet's travel where it comes to rest wherever it's let go, rather
/// than springing to an anchor.
///
/// Its ends are placed as anchors are, but needn't be among the content's
/// [SheetContent.anchors]: the sheet can be pulled up (or down) as far as a range goes
/// whether or not an anchor waits there. A fling within the range glides to a stop where
/// it runs out of speed. One that would carry past an end stops at that end, unless
/// there are anchors beyond it to catch the fling as usual. No anchor may lie strictly
/// between the ends.
@immutable
class SheetRange {
  const SheetRange(this.from, this.to, {this.carriesIntoScroll = false});

  /// The anchors at the range's ends, in either order.
  final SheetAnchor from;
  final SheetAnchor to;

  /// Whether a fling up the content that would carry the sheet past the range's upper
  /// end, when that's as high as the sheet goes, glides there and carries on scrolling
  /// the content with the speed it has left, as if the sheet and its content were one
  /// long scroll. Otherwise the sheet stops there, and the content stays where it is.
  final bool carriesIntoScroll;

  @override
  bool operator ==(Object other) =>
      other is SheetRange &&
      other.from == from &&
      other.to == to &&
      other.carriesIntoScroll == carriesIntoScroll;

  @override
  int get hashCode => Object.hash(SheetRange, from, to, carriesIntoScroll);

  @override
  String toString() => 'SheetRange($from, $to)';
}

/// A range placed for the current layout, with [lower] at or below [upper].
@immutable
class ResolvedRange {
  const ResolvedRange(
    this.range,
    this.lower,
    this.upper, {
    this.fromIsUpper = false,
  });

  final SheetRange range;
  final double lower;
  final double upper;

  /// Whether [SheetRange.from] is the upper end, and [SheetRange.to] the lower.
  final bool fromIsUpper;

  /// Where [end], one of the range's ends, is placed.
  double extentOf(SheetAnchor end) =>
      (end == range.from) != fromIsUpper ? lower : upper;

  /// Whether [extent] lies within the range, ends included.
  bool contains(double extent) =>
      extent >= lower - _epsilon && extent <= upper + _epsilon;

  /// How far [extent] is from [lower] to [upper], from 0 to 1.
  double fractionOf(double extent) => upper - lower <= _epsilon
      ? 0
      : ((extent - lower) / (upper - lower)).clamp(0.0, 1.0);

  /// The extent [fraction] of the way from [lower] to [upper].
  double extentAt(double fraction) => lower + (upper - lower) * fraction;

  @override
  bool operator ==(Object other) =>
      other is ResolvedRange &&
      other.range == range &&
      other.lower == lower &&
      other.upper == upper &&
      other.fromIsUpper == fromIsUpper;

  @override
  int get hashCode => Object.hash(range, lower, upper, fromIsUpper);

  @override
  String toString() =>
      'ResolvedRange($range, ${lower.toStringAsFixed(1)}–${upper.toStringAsFixed(1)})';
}

/// Places [ranges], lowest first, dropping any whose ends can't be placed yet. No anchor
/// among [anchors], already placed, may lie within one.
List<ResolvedRange> _resolveRanges(
  List<SheetRange> ranges,
  List<ResolvedAnchor> anchors,
  SheetGeometry geometry,
  Map<Object, _Span> sections,
) {
  final resolved = <ResolvedRange>[];
  for (final range in ranges) {
    final from = range.from._resolve(geometry, sections);
    final to = range.to._resolve(geometry, sections);
    if (from == null || to == null) continue;
    final lower = math.min(from, to);
    final upper = math.max(from, to);
    assert(
      !anchors.any(
        (a) => a.extent > lower + _epsilon && a.extent < upper - _epsilon,
      ),
      'No anchor may lie within $range.',
    );
    resolved.add(ResolvedRange(range, lower, upper, fromIsUpper: from > to));
  }
  return resolved..sort((a, b) => a.lower.compareTo(b.lower));
}

/// Where the sheet rests: at an anchor, or somewhere within a range.
@immutable
sealed class _Rest {
  const _Rest();

  /// The extent this rest is at among [anchors] and [ranges], or null if they don't
  /// have it.
  double? extentIn(List<ResolvedAnchor> anchors, List<ResolvedRange> ranges);

  /// Whether a page with [anchors] and [ranges], placed or not, can rest here.
  bool isAmong(List<SheetAnchor> anchors, List<SheetRange> ranges);
}

final class _AtAnchor extends _Rest {
  const _AtAnchor(this.anchor);

  final SheetAnchor anchor;

  @override
  double? extentIn(List<ResolvedAnchor> anchors, List<ResolvedRange> ranges) =>
      anchors.firstWhereOrNull((a) => a.anchor == anchor)?.extent;

  @override
  bool isAmong(List<SheetAnchor> anchors, List<SheetRange> ranges) =>
      anchors.contains(anchor);

  @override
  bool operator ==(Object other) =>
      other is _AtAnchor && other.anchor == anchor;

  @override
  int get hashCode => anchor.hashCode;
}

final class _InRange extends _Rest {
  const _InRange(this.range, this.fraction);

  final SheetRange range;

  /// How far along the range, from its lower end (0) to its upper end (1), so it keeps
  /// its place as the range's ends move.
  final double fraction;

  @override
  double? extentIn(List<ResolvedAnchor> anchors, List<ResolvedRange> ranges) =>
      ranges.firstWhereOrNull((r) => r.range == range)?.extentAt(fraction);

  @override
  bool isAmong(List<SheetAnchor> anchors, List<SheetRange> ranges) =>
      ranges.contains(range);

  @override
  bool operator ==(Object other) =>
      other is _InRange && other.range == range && other.fraction == fraction;

  @override
  int get hashCode => Object.hash(range, fraction);
}
