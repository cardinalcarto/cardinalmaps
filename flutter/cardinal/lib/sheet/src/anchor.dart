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
