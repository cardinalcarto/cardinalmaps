/// Rows that change smoothly: a column whose rows grow and fade in as they appear and
/// shrink and fade out as they go, a sliver that doesn't shrink from under the bottom
/// of the viewport, and text that cross-fades as it changes.
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

/// [child], a box, as a sliver that doesn't shrink from under the bottom of the
/// viewport: rows going below what's on screen would leave the viewport scrolled past
/// the end, which then scrolls back, moving everything showing. It keeps the room
/// showing instead, blank, and gives it up as it scrolls out of sight.
class SteadySliver extends SingleChildRenderObjectWidget {
  const SteadySliver({super.key, super.child});

  @override
  RenderSteadySliver createRenderObject(BuildContext context) =>
      RenderSteadySliver();
}

class RenderSteadySliver extends RenderSliverToBoxAdapter {
  /// The extent at the last layout.
  double _extent = 0;

  @override
  void performLayout() {
    super.performLayout();
    final laid = geometry!;
    if (laid.scrollOffsetCorrection != null) return;
    // As far down as shows now, at most as far as it reached before.
    final showing = constraints.scrollOffset + constraints.remainingPaintExtent;
    final extent = math.max(laid.scrollExtent, math.min(_extent, showing));
    _extent = extent;
    if (extent <= laid.scrollExtent) return;
    final paint = calculatePaintOffset(constraints, from: 0, to: extent);
    geometry = SliverGeometry(
      scrollExtent: extent,
      paintExtent: paint,
      maxPaintExtent: extent,
      hitTestExtent: paint,
      cacheExtent: calculateCacheOffset(constraints, from: 0, to: extent),
      hasVisualOverflow: laid.hasVisualOverflow,
    );
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
