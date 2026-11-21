/// The ways of traveling as tabs across the directions sheet, each with how long its
/// best option takes.
library;

import 'dart:math' as math;

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoSlidingSegmentedControl;
import 'package:material_ui/material_ui.dart';

import '../ui/adaptive.dart';
import '../ui/haptics.dart';
import 'option_row.dart' show textLinesHeight;
import 'route_source.dart';

/// What a tab says for a way of traveling with nothing to offer.
const noTimeDash = '—';

/// The most the system's text scale enlarges a tab's time, as four tabs share the
/// sheet's width.
const double _maxTextScale = 1.3;

/// Above the tabs on Android, and above the segmented control on iOS.
const double _tabsTop = 6, _segmentsTop = 10;

/// Above and below a tab's label on Android, and a segment's on iOS.
const double _tabPadding = 7, _segmentPadding = 4;

/// Round the segments, within the segmented control's track: its own default.
const EdgeInsets _trackPadding = EdgeInsets.symmetric(
  vertical: 2,
  horizontal: 3,
);

/// Four equal tabs, one for each way of traveling: its icon over [times]' entry for
/// it. A tab keeps the room for its time, which fades in when it comes; null leaves it
/// empty.
///
/// Tabs on Android, where the tooltip says the way and its time; a segmented control
/// on iOS.
class ModeTabs extends StatelessWidget {
  const ModeTabs({
    super.key,
    required this.selected,
    required this.wheelchair,
    required this.times,
    required this.onChanged,
  });

  final TravelMode selected;

  /// Whether walking is by wheelchair, which its tab then shows.
  final bool wheelchair;

  /// What each tab says under its icon: how long, or [noTimeDash].
  final Map<TravelMode, String?> times;
  final ValueChanged<TravelMode> onChanged;

  /// How tall the tabs are at [context]'s text size, whatever they say.
  static double heightOf(BuildContext context) {
    final label = _TabLabel.heightOf(
      context,
      MediaQuery.textScalerOf(context).clamp(maxScaleFactor: _maxTextScale),
    );
    return isCupertinoOf(context)
        ? _segmentsTop + _trackPadding.vertical + 2 * _segmentPadding + label
        : _tabsTop + 2 * _tabPadding + label;
  }

  void _change(TravelMode mode) {
    if (!mode.available || mode == selected) return;
    Haptics.selection();
    onChanged(mode);
  }

  /// What a screen reader, and Android's tooltip, call [mode]'s tab.
  String _label(TravelMode mode) {
    final name = mode.labelFor(wheelchair: wheelchair);
    return switch (times[mode]) {
      null => name,
      noTimeDash => '$name, no route',
      final time => '$name, $time',
    };
  }

  @override
  Widget build(BuildContext context) => MediaQuery.withClampedTextScaling(
    maxScaleFactor: _maxTextScale,
    child: Builder(
      builder: (context) =>
          isCupertinoOf(context) ? _segments(context) : _tabs(context),
    ),
  );

  Widget _tabs(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final index = TravelMode.values.indexOf(selected);
    final last = TravelMode.values.length - 1;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, _tabsTop, 12, 0),
      child: DecoratedBox(
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: scheme.outlineVariant)),
        ),
        child: Stack(
          children: [
            Row(
              children: [
                for (final mode in TravelMode.values)
                  Expanded(
                    child: Tooltip(
                      message: _label(mode),
                      child: Semantics(
                        button: true,
                        enabled: mode.available,
                        selected: mode == selected,
                        label: _label(mode),
                        excludeSemantics: true,
                        onTap: mode.available ? () => _change(mode) : null,
                        child: InkWell(
                          onTap: mode.available ? () => _change(mode) : null,
                          child: Padding(
                            padding: const EdgeInsets.symmetric(
                              vertical: _tabPadding,
                            ),
                            child: _TabLabel(
                              icon: mode.iconFor(wheelchair: wheelchair),
                              time: times[mode],
                              color: mode == selected
                                  ? scheme.primary
                                  : scheme.onSurfaceVariant,
                              emphasized: mode == selected,
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
              ],
            ),
            // The line under the selected tab, which slides to the next.
            Positioned.fill(
              child: IgnorePointer(
                child: AnimatedAlign(
                  duration: const Duration(milliseconds: 200),
                  curve: Curves.easeOut,
                  alignment: Alignment(-1 + 2 * index / last, 1),
                  child: FractionallySizedBox(
                    widthFactor: 1 / TravelMode.values.length,
                    child: ColoredBox(
                      color: scheme.primary,
                      child: const SizedBox(height: 3),
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _segments(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(16, _segmentsTop, 16, 0),
    child: CupertinoSlidingSegmentedControl<TravelMode>(
      padding: _trackPadding,
      groupValue: selected,
      disabledChildren: {
        for (final mode in TravelMode.values)
          if (!mode.available) mode,
      },
      children: {
        for (final mode in TravelMode.values)
          mode: Semantics(
            label: _label(mode),
            excludeSemantics: true,
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: _segmentPadding),
              child: _TabLabel(
                icon: mode.iconFor(wheelchair: wheelchair),
                time: times[mode],
                emphasized: mode == selected,
              ),
            ),
          ),
      },
      onValueChanged: (mode) {
        if (mode != null) _change(mode);
      },
    ),
  );
}

/// A tab's icon over its time, in [color] if given. The time's line is always there,
/// as tall as its text, and what it says fades in and out.
class _TabLabel extends StatelessWidget {
  const _TabLabel({
    required this.icon,
    required this.time,
    this.color,
    required this.emphasized,
  });

  final IconData icon;
  final String? time;
  final Color? color;
  final bool emphasized;

  static const _iconSize = 20.0, _gap = 2.0;

  /// The style of a tab's time.
  static TextStyle _style(BuildContext context, {required bool emphasized}) =>
      Theme.of(context).textTheme.labelMedium!
          .copyWith(fontWeight: emphasized ? FontWeight.w700 : FontWeight.w500);

  /// How tall a label is with its text scaled by [scaler], selected or not.
  static double heightOf(BuildContext context, TextScaler scaler) {
    double line(bool emphasized) => textLinesHeight(
      context,
      _style(context, emphasized: emphasized),
      textScaler: scaler,
    );
    return _iconSize + _gap + math.max(line(true), line(false));
  }

  @override
  Widget build(BuildContext context) {
    final style = _style(context, emphasized: emphasized);
    final end = color ?? DefaultTextStyle.of(context).style.color;
    return TweenAnimationBuilder<Color?>(
      tween: ColorTween(end: end),
      duration: const Duration(milliseconds: 200),
      curve: Curves.easeOut,
      builder: (context, color, _) => Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: _iconSize, color: color),
          const SizedBox(height: _gap),
          Stack(
            alignment: Alignment.center,
            children: [
              // Keeps the line's height, whatever it says.
              Text(' ', style: style, maxLines: 1),
              AnimatedSwitcher(
                duration: const Duration(milliseconds: 200),
                switchInCurve: Curves.easeOut,
                switchOutCurve: Curves.easeOut,
                child: time == null
                    ? const SizedBox.shrink(key: ValueKey(#none))
                    : FittedBox(
                        key: ValueKey(time),
                        fit: BoxFit.scaleDown,
                        child: Text(
                          time!,
                          style: style.copyWith(color: color),
                          maxLines: 1,
                          softWrap: false,
                        ),
                      ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
