/// Rows that change smoothly: a column whose rows grow and fade in as they appear and
/// shrink and fade out as they go, a sliver that holds one of its rows where it is on
/// screen while the rows above it change, and text that cross-fades as it changes.
library;

import 'dart:math' as math;

import 'package:flutter/rendering.dart';
import 'package:material_ui/material_ui.dart';

/// How long a row takes to appear, go, or change size, and text to cross-fade.
const rowDuration = Duration(milliseconds: 250);

/// The curve rows and text change on.
const rowCurve = Curves.easeOutCubic;

/// [rows] top to bottom, each with a key unique among them that it keeps while it
/// shows. A row with a new key grows and fades in; one whose key is gone shrinks and
/// fades out where it was; one that changes height eases to its new one. The rows
/// there at first are simply there.
class AnimatedRows extends StatefulWidget {
  const AnimatedRows({super.key, required this.rows});

  final List<Widget> rows;

  @override
  State<AnimatedRows> createState() => _AnimatedRowsState();
}

class _Entry {
  _Entry(this.row, {required this.appear});

  Widget row;
  final bool appear;
  bool leaving = false;

  Key get key => row.key!;
}

class _AnimatedRowsState extends State<AnimatedRows> {
  late var _entries = [
    for (final row in widget.rows) _Entry(row, appear: false),
  ];

  @override
  void didUpdateWidget(AnimatedRows oldWidget) {
    super.didUpdateWidget(oldWidget);
    final old = {for (final e in _entries) e.key: e};
    final keys = {for (final row in widget.rows) row.key!};
    // Each row going stays after the nearest row before it that stays.
    final after = <Key?, List<_Entry>>{};
    Key? kept;
    for (final e in _entries) {
      if (keys.contains(e.key)) {
        kept = e.key;
      } else {
        e.leaving = true;
        (after[kept] ??= []).add(e);
      }
    }
    _entries = [
      ...?after[null],
      for (final row in widget.rows) ...[
        switch (old[row.key]) {
          final e? =>
            e
              ..row = row
              ..leaving = false,
          null => _Entry(row, appear: true),
        },
        ...?after[row.key],
      ],
    ];
  }

  void _gone(Key key) {
    if (!mounted) return;
    setState(() => _entries.removeWhere((e) => e.key == key && e.leaving));
  }

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      for (final e in _entries)
        _RowSlot(
          key: e.key,
          appear: e.appear,
          leaving: e.leaving,
          onGone: () => _gone(e.key),
          child: e.row,
        ),
    ],
  );
}

class _RowSlot extends StatefulWidget {
  const _RowSlot({
    super.key,
    required this.appear,
    required this.leaving,
    required this.onGone,
    required this.child,
  });

  final bool appear;
  final bool leaving;
  final VoidCallback onGone;
  final Widget child;

  @override
  State<_RowSlot> createState() => _RowSlotState();
}

class _RowSlotState extends State<_RowSlot>
    with SingleTickerProviderStateMixin {
  late final _shown = AnimationController(
    vsync: this,
    duration: rowDuration,
    value: widget.appear ? 0 : 1,
  )..addStatusListener(_status);
  late final _curve = CurvedAnimation(parent: _shown, curve: rowCurve);

  @override
  void initState() {
    super.initState();
    if (widget.leaving) {
      _shown.reverse();
    } else if (widget.appear) {
      _shown.forward();
    }
  }

  void _status(AnimationStatus status) {
    if (status == AnimationStatus.dismissed && widget.leaving) widget.onGone();
  }

  @override
  void didUpdateWidget(_RowSlot oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.leaving != oldWidget.leaving) {
      widget.leaving ? _shown.reverse() : _shown.forward();
    }
  }

  @override
  void dispose() {
    _curve.dispose();
    _shown.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => SizeTransition(
    sizeFactor: _curve,
    alignment: Alignment.topCenter,
    child: FadeTransition(
      opacity: _curve,
      child: AnimatedSize(
        duration: rowDuration,
        curve: rowCurve,
        alignment: Alignment.topCenter,
        child: widget.child,
      ),
    ),
  );
}

/// [child], a box, as a sliver that holds what's under [anchor] where it is on screen
/// while [anchor] is set: when the rows above it change height, it scrolls the
/// viewport by as much, as far as it can scroll.
class AnchoredSliver extends SingleChildRenderObjectWidget {
  const AnchoredSliver({super.key, required this.anchor, super.child});

  /// The key of the widget to hold in place, or null to let it move.
  final GlobalKey? anchor;

  @override
  RenderAnchoredSliver createRenderObject(BuildContext context) =>
      RenderAnchoredSliver()..anchor = anchor;

  @override
  void updateRenderObject(
    BuildContext context,
    RenderAnchoredSliver renderObject,
  ) {
    renderObject.anchor = anchor;
  }
}

class RenderAnchoredSliver extends RenderSliverToBoxAdapter {
  GlobalKey? _anchor;

  /// Where the anchor's top was in the viewport's content at the last layout.
  double? _anchorAt;

  /// The scroll extent at the end of the last layout that didn't correct the scroll.
  double? _extent;

  set anchor(GlobalKey? key) {
    if (key == _anchor) return;
    _anchor = key;
    // As it is now, before anything changes.
    _anchorAt = key == null ? null : _topOf(key);
  }

  double? _topOf(GlobalKey key) {
    final box = key.currentContext?.findRenderObject();
    final child = this.child;
    if (box is! RenderBox ||
        !box.attached ||
        !box.hasSize ||
        child == null ||
        !child.hasSize) {
      return null;
    }
    // Below the slivers before this one, which may have changed too.
    return constraints.precedingScrollExtent +
        box.localToGlobal(Offset.zero, ancestor: child).dy;
  }

  ScrollPosition? get _position {
    for (var node = parent; node != null; node = node.parent) {
      if (node is RenderViewportBase) {
        final offset = node.offset;
        return offset is ScrollPosition ? offset : null;
      }
    }
    return null;
  }

  @override
  void performLayout() {
    super.performLayout();
    final key = _anchor;
    final before = _anchorAt;
    final now = key == null ? null : _topOf(key);
    _anchorAt = now;
    if (before != null && now != null && (now - before).abs() > 0.01) {
      var shift = now - before;
      final position = _position;
      if (position == null ||
          !position.hasPixels ||
          !position.hasContentDimensions) {
        shift = 0;
      } else {
        // As far as it can scroll once this layout is done.
        final grown = geometry!.scrollExtent - (_extent ?? 0);
        final low = position.minScrollExtent - position.pixels;
        final high = position.maxScrollExtent + grown - position.pixels;
        shift = shift.clamp(math.min(0.0, low), math.max(0.0, high));
      }
      if (shift.abs() > 0.01) {
        // The viewport scrolls by as much and lays this out again.
        geometry = SliverGeometry(scrollOffsetCorrection: shift);
        return;
      }
    }
    _extent = geometry!.scrollExtent;
  }
}

/// [child], cross-fading from what it showed when [value] changes, with the one
/// going under the one coming, both at [alignment].
class FadeSwap extends StatelessWidget {
  const FadeSwap({
    super.key,
    required this.value,
    required this.child,
    this.alignment = AlignmentDirectional.centerStart,
    this.fit = StackFit.loose,
  });

  /// What [child] shows: a change cross-fades.
  final Object? value;
  final AlignmentGeometry alignment;

  /// How the two are sized while they cross: [StackFit.passthrough] for blocks that
  /// fill the width given.
  final StackFit fit;
  final Widget child;

  @override
  Widget build(BuildContext context) => AnimatedSwitcher(
    duration: rowDuration,
    switchInCurve: rowCurve,
    switchOutCurve: rowCurve,
    layoutBuilder: (current, previous) => Stack(
      alignment: alignment,
      fit: fit,
      children: [...previous, ?current],
    ),
    child: KeyedSubtree(key: ValueKey(value), child: child),
  );
}
