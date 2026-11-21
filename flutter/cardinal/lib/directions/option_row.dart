/// A way to make the trip as a row of the directions sheet's list: every row, the
/// placeholder that stands for one on its way, and the message that stands for none are
/// one height, so the row held above the buttons never changes the sheet's anchors.
library;

import 'dart:math' as math;

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../sheet/sheet.dart';
import '../ui/adaptive.dart';
import '../units.dart';
import 'format.dart';
import 'speed_profile.dart';
import 'transit_sheet.dart' show clock;

/// The text scale from which rows take the roomier layout: the arrival time on a line
/// of its own, and two lines each for the name and the caption.
const double roomyTextScale = 1.5;

/// Whether text is scaled so far that rows, and whatever else lays itself out by
/// them, take the roomier layout. By the scale alone, never by what fits.
bool isRoomyText(TextScaler scaler) => scaler.scale(16) / 16 >= roomyTextScale;

/// How a row of the list is laid out: one height for every row of every way of
/// traveling, which depends only on the platform, the theme's text and how far the
/// system scales it.
@immutable
class OptionRowMetrics {
  const OptionRowMetrics._({
    required this.roomy,
    required this.headline,
    required this.arrival,
    required this.name,
    required this.caption,
    required this.headlineHeight,
    required this.arrivalHeight,
    required this.nameHeight,
    required this.captionHeight,
    required this.margin,
    required this.padding,
    required this.spacing,
    required this.radius,
    required this.top,
  });

  /// The layout for the theme and text scale around [context].
  factory OptionRowMetrics.of(BuildContext context) {
    final theme = Theme.of(context);
    final scaler = MediaQuery.textScalerOf(context);
    final cupertino = isCupertinoOf(context);
    final roomy = isRoomyText(scaler);
    final muted = theme.colorScheme.onSurfaceVariant;
    final text = theme.textTheme;
    final headline = text.titleLarge!.copyWith(fontWeight: FontWeight.w700);
    final arrival = text.bodySmall!.copyWith(color: muted);
    final name = text.titleSmall!;
    final caption = text.bodySmall!.copyWith(color: muted);
    final base = DefaultTextStyle.of(context).style;
    double lines(TextStyle style, int count) {
      final painter = TextPainter(
        text: TextSpan(
          text: List.filled(count, ' ').join('\n'),
          style: base.merge(style),
        ),
        textDirection: Directionality.of(context),
        textScaler: scaler,
      )..layout();
      final height = painter.height;
      painter.dispose();
      return height;
    }

    return OptionRowMetrics._(
      roomy: roomy,
      headline: headline,
      arrival: arrival,
      name: name,
      caption: caption,
      headlineHeight: lines(headline, 1),
      arrivalHeight: lines(arrival, 1),
      nameHeight: lines(name, roomy ? 2 : 1),
      captionHeight: lines(caption, roomy ? 2 : 1),
      margin: cupertino ? 16 : 8,
      padding: cupertino
          ? const EdgeInsets.symmetric(horizontal: 14, vertical: 10)
          : const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      spacing: cupertino ? 8 : 2,
      radius: cupertino ? 16 : 14,
      top: cupertino ? 10 : 8,
    );
  }

  /// The height of a row at [context]'s text scale: see [height].
  static double heightOf(BuildContext context) =>
      OptionRowMetrics.of(context).height;

  /// Whether rows take the roomier layout (see [roomyTextScale]).
  final bool roomy;

  final TextStyle headline;
  final TextStyle arrival;
  final TextStyle name;
  final TextStyle caption;

  /// What each line of a row reserves: the height of its text at the text scale, for
  /// as many lines as it may take.
  final double headlineHeight;
  final double arrivalHeight;
  final double nameHeight;
  final double captionHeight;

  /// The space on either side of a row.
  final double margin;

  /// Around a row's content, within its fill.
  final EdgeInsets padding;

  /// The space under a row, before the next.
  final double spacing;

  /// How round a row's corners are.
  final double radius;

  /// The space above the first row, under the tabs.
  final double top;

  /// Between the lines of a row.
  static const gap = 6.0;

  /// Whether the name and the caption take two lines.
  int get textLines => roomy ? 2 : 1;

  /// How tall a row is with the space under it: every row's place in the list, and
  /// what's held above the buttons.
  double get height =>
      padding.vertical +
      headlineHeight +
      (roomy ? arrivalHeight : 0) +
      gap +
      nameHeight +
      gap +
      routeGraphHeight +
      gap +
      captionHeight +
      spacing;
}

/// The colors a row is filled with.
({Color selected, Color plain}) _rowFills(BuildContext context) {
  final theme = Theme.of(context);
  final scheme = theme.colorScheme;
  if (!isCupertinoOf(context)) {
    return (
      selected: scheme.primaryContainer,
      plain: scheme.primaryContainer.withValues(alpha: 0),
    );
  }
  // Tints, as the sheet is glass while it's low: nothing on it is opaque.
  final dark = theme.brightness == Brightness.dark;
  return (
    selected: scheme.primary.withValues(alpha: dark ? 0.3 : 0.16),
    plain: groupedRowColor(context).withValues(alpha: 0.7),
  );
}

/// What every row of the list is drawn in: its place in the list, [OptionRowMetrics]
/// tall, with [child] in a rounded fill, and the shadow it casts while it's held away
/// from its place.
class _RowFrame extends StatelessWidget {
  const _RowFrame({
    required this.metrics,
    required this.selected,
    required this.child,
    this.onTap,
  });

  final OptionRowMetrics metrics;
  final bool selected;
  final VoidCallback? onTap;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final fills = _rowFills(context);
    final corners = BorderRadius.circular(metrics.radius);
    return SizedBox(
      height: metrics.height,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          const Positioned(
            left: 0,
            right: 0,
            top: -_HeldShadow.depth,
            bottom: -_HeldShadow.depth,
            child: _HeldShadow(),
          ),
          Positioned.fill(
            child: Padding(
              padding: EdgeInsets.fromLTRB(
                metrics.margin,
                0,
                metrics.margin,
                metrics.spacing,
              ),
              child: AnimatedContainer(
                duration: const Duration(milliseconds: 200),
                curve: Curves.easeOut,
                decoration: BoxDecoration(
                  color: selected ? fills.selected : fills.plain,
                  borderRadius: corners,
                ),
                child: Material(
                  type: MaterialType.transparency,
                  child: InkWell(
                    onTap: onTap,
                    borderRadius: corners,
                    child: Padding(padding: metrics.padding, child: child),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The soft shadow a row casts on the list while it's held away from its place, on
/// the edge the list passes behind: above it when it's held up from below, under it
/// when it's held down from above. It fades in over the first [_fadeIn] pixels the row
/// is from its place, so it never pops.
class _HeldShadow extends StatefulWidget {
  const _HeldShadow();

  /// How far the shadow reaches from the row's edge.
  static const depth = 10.0;

  static const _fadeIn = 8.0;

  @override
  State<_HeldShadow> createState() => _HeldShadowState();
}

/// How much of its shadow a row shows when it's painted [displacement] from its place
/// in the list (see [SheetSlot.displacementOf]): from none in place to all of it
/// [_HeldShadow._fadeIn] pixels away, positive above the row and negative under it.
double heldShadowStrength(double displacement) =>
    (displacement / _HeldShadow._fadeIn).clamp(-1.0, 1.0);

class _HeldShadowState extends State<_HeldShadow> {
  ValueListenable<double>? _displacement;

  /// How much of the shadow shows, from 0 to 1, above the row (positive) or under it
  /// (negative).
  final _strength = ValueNotifier<double>(0);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final displacement = SheetSlot.displacementOf(context);
    if (displacement == _displacement) return;
    _displacement?.removeListener(_moved);
    _displacement = displacement..addListener(_moved);
    _moved();
  }

  void _moved() => _strength.value = heldShadowStrength(_displacement!.value);

  @override
  void dispose() {
    _displacement?.removeListener(_moved);
    _strength.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => IgnorePointer(
    // A layer of its own: as it fades, nothing else of the row is painted again.
    child: RepaintBoundary(
      child: CustomPaint(
        painter: _HeldShadowPainter(
          _strength,
          Theme.of(context).colorScheme.shadow,
        ),
      ),
    ),
  );
}

class _HeldShadowPainter extends CustomPainter {
  _HeldShadowPainter(this.strength, this.color) : super(repaint: strength);

  final ValueListenable<double> strength;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final value = strength.value;
    if (value == 0) return;
    const depth = _HeldShadow.depth;
    final above = value > 0;
    final rect = above
        ? Rect.fromLTWH(0, 0, size.width, depth)
        : Rect.fromLTWH(0, size.height - depth, size.width, depth);
    final dark = color.withValues(alpha: 0.2 * value.abs());
    canvas.drawRect(
      rect,
      Paint()
        ..shader = LinearGradient(
          begin: above ? Alignment.bottomCenter : Alignment.topCenter,
          end: above ? Alignment.topCenter : Alignment.bottomCenter,
          colors: [dark, dark.withValues(alpha: 0)],
        ).createShader(rect),
    );
  }

  @override
  bool shouldRepaint(_HeldShadowPainter oldDelegate) =>
      strength != oldDelegate.strength || color != oldDelegate.color;
}

/// What a list of routes shares, for their rows to compare: how long the slowest
/// takes, which is as wide as a graph gets, and the fastest any of them goes, which is
/// as tall.
@immutable
class RouteScale {
  const RouteScale({required this.slowest, required this.topSpeed});

  factory RouteScale.of(List<DirectionsRoute> routes) => RouteScale(
    slowest: routes.fold(0, (most, route) => math.max(most, route.duration)),
    topSpeed: routes
        .expand((route) => route.runs)
        .fold(0, (most, run) => math.max(most, run.speed)),
  );

  final double slowest;
  final double topSpeed;

  /// How much of the graph's room [route] takes: its share of the slowest's time.
  double share(DirectionsRoute route) =>
      slowest > 0 ? (route.duration / slowest).clamp(0.0, 1.0) : 1;
}

/// A street route as a row: how long it takes and when it arrives, what it's called,
/// its graph, and how it differs from the others.
///
/// The selected row has a tonal fill and its time in the primary color; nothing else
/// about it changes, so selecting one moves nothing.
class OptionRow extends StatelessWidget {
  const OptionRow({
    super.key,
    required this.routes,
    required this.index,
    required this.scale,
    required this.selected,
    required this.driving,
    required this.units,
    required this.now,
    required this.metrics,
    required this.onTap,
  });

  /// The routes of the list, and which of them this row is.
  final List<DirectionsRoute> routes;
  final int index;
  final RouteScale scale;
  final bool selected;

  /// Whether the routes are driven: the graph is then the speed profile, and the
  /// caption says what a driver would want to know.
  final bool driving;

  /// Metric or imperial, for the distance.
  final UnitSystem units;

  /// The time, for when the route arrives.
  final DateTime now;
  final OptionRowMetrics metrics;
  final VoidCallback onTap;

  /// Between the time a route takes and when it arrives, on the one line.
  static const _headlineGap = 8.0;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final route = routes[index];
    final duration = formatDuration(route.duration);
    final arrives = clock(
      context,
      now.add(Duration(milliseconds: (route.duration * 1000).round())),
    );
    final arrival = 'Arrive $arrives';
    final name = routeName(AppLocalizations.of(context), route);
    final facts = routeFacts(routes, index, driving: driving, units: units);
    final tones = profileTones(scheme);
    // One line of its own width, shrunk to fit rather than cut off.
    Widget whole(String text, {required double height, Alignment? alignment}) =>
        SizedBox(
          height: height,
          child: FittedBox(
            fit: BoxFit.scaleDown,
            alignment: alignment ?? AlignmentDirectional.centerStart,
            child: Text(text, maxLines: 1, softWrap: false),
          ),
        );
    final headline = AnimatedDefaultTextStyle(
      duration: const Duration(milliseconds: 200),
      curve: Curves.easeOut,
      style: metrics.headline.copyWith(
        color: selected ? scheme.primary : scheme.onSurface,
      ),
      child: whole(duration, height: metrics.headlineHeight),
    );
    return Semantics(
      button: true,
      selected: selected,
      // All of it, whatever the row has room to show.
      label: [duration, arrival, ?name, ...facts].join(', '),
      excludeSemantics: true,
      onTap: onTap,
      child: _RowFrame(
        metrics: metrics,
        selected: selected,
        onTap: onTap,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (metrics.roomy) ...[
              headline,
              DefaultTextStyle.merge(
                style: metrics.arrival,
                child: whole(arrival, height: metrics.arrivalHeight),
              ),
            ] else
              LayoutBuilder(
                builder: (context, constraints) => Row(
                  crossAxisAlignment: CrossAxisAlignment.baseline,
                  textBaseline: TextBaseline.alphabetic,
                  children: [
                    // All of it, in as much of the line as it needs: it shrinks
                    // only if the whole line is too little.
                    ConstrainedBox(
                      constraints: BoxConstraints(
                        maxWidth: constraints.maxWidth - _headlineGap,
                      ),
                      child: headline,
                    ),
                    const SizedBox(width: _headlineGap),
                    // The one that gives way if the two can't share the line.
                    Expanded(
                      child: Text(
                        arrival,
                        style: metrics.arrival,
                        textAlign: TextAlign.end,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                  ],
                ),
              ),
            const SizedBox(height: OptionRowMetrics.gap),
            // A route with no named roads keeps the line's room.
            SizedBox(
              height: metrics.nameHeight,
              child: name == null
                  ? null
                  : Text(
                      name,
                      style: metrics.name,
                      maxLines: metrics.textLines,
                      overflow: TextOverflow.ellipsis,
                    ),
            ),
            const SizedBox(height: OptionRowMetrics.gap),
            FractionallySizedBox(
              alignment: AlignmentDirectional.centerStart,
              widthFactor: scale.share(route),
              child: driving
                  ? SpeedProfile(
                      runs: route.runs,
                      topSpeed: scale.topSpeed,
                      tones: tones,
                    )
                  : RouteBar(color: tones.other),
            ),
            const SizedBox(height: OptionRowMetrics.gap),
            SizedBox(
              height: metrics.captionHeight,
              child: Text(
                facts.join(' · '),
                style: metrics.caption,
                maxLines: metrics.textLines,
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Stands for a row whose route is on its way, as tall as one: grey where its lines
/// will be.
class OptionPlaceholder extends StatelessWidget {
  const OptionPlaceholder({super.key, required this.metrics});

  final OptionRowMetrics metrics;

  @override
  Widget build(BuildContext context) {
    final color = Theme.of(context).colorScheme.onSurface
        .withValues(alpha: 0.08);
    Widget block(double height, double widthFactor) => SizedBox(
      height: height,
      child: FractionallySizedBox(
        alignment: AlignmentDirectional.centerStart,
        widthFactor: widthFactor,
        heightFactor: 0.6,
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: color,
            borderRadius: BorderRadius.circular(4),
          ),
        ),
      ),
    );
    return ExcludeSemantics(
      child: _RowFrame(
        metrics: metrics,
        selected: false,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            block(
              metrics.headlineHeight +
                  (metrics.roomy ? metrics.arrivalHeight : 0),
              0.3,
            ),
            const SizedBox(height: OptionRowMetrics.gap),
            block(metrics.nameHeight, 0.55),
            const SizedBox(height: OptionRowMetrics.gap),
            FractionallySizedBox(
              alignment: AlignmentDirectional.centerStart,
              widthFactor: 0.7,
              child: RouteBar(color: color),
            ),
            const SizedBox(height: OptionRowMetrics.gap),
            block(metrics.captionHeight, 0.4),
          ],
        ),
      ),
    );
  }
}

/// Says why there are no rows, in the place of one and as tall. The words shrink to
/// fit rather than be cut off.
class OptionMessage extends StatelessWidget {
  const OptionMessage({
    super.key,
    required this.message,
    required this.metrics,
  });

  final String message;
  final OptionRowMetrics metrics;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return _RowFrame(
      metrics: metrics,
      selected: false,
      child: LayoutBuilder(
        builder: (context, constraints) => FittedBox(
          fit: BoxFit.scaleDown,
          child: SizedBox(
            width: constraints.maxWidth,
            child: Text(
              message,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The rows of a street way of traveling, each in a [SheetSlot] the sheet can hold
/// above its buttons: the routes, or placeholders while they're [loading], or
/// [message] in the first row's place when there are none.
///
/// The slots are the list's, not the routes': the row in a slot fades into whatever
/// takes its place (another way's route, a placeholder, the message), so a row held
/// above the buttons changes in place. A slot that's no longer needed stays until
/// what was in it has faded and gone back to its place.
class OptionList extends StatefulWidget {
  const OptionList({
    super.key,
    required this.routes,
    required this.selected,
    required this.loading,
    required this.message,
    required this.driving,
    required this.units,
    required this.now,
    required this.opening,
    required this.reveal,
    required this.onSelect,
  });

  /// How many rows stand for routes on their way.
  static const placeholders = 3;

  /// How many rows the sheet first opens to show, if there are as many.
  static const shownAtFirst = 3;

  /// The ID of the slot of the row at [index], for [SheetContent.held].
  static Object slotId(int index) => (#directionsOption, index);

  final List<DirectionsRoute> routes;

  /// The index of the selected one of [routes].
  final int selected;
  final bool loading;

  /// What to say with no routes and none on their way.
  final String message;
  final bool driving;
  final UnitSystem units;
  final DateTime now;

  /// The ID of a [SheetSection] that ends under the last row the sheet first opens to
  /// show.
  final Object opening;

  /// Fades the rows in, and out, with the rest of what the page shows around its
  /// content.
  final Animation<double> reveal;
  final ValueChanged<int> onSelect;

  /// How many rows there are: the routes, the placeholders, or the message.
  int get count => loading
      ? placeholders
      : routes.isEmpty
      ? 1
      : routes.length;

  /// The slot the sheet holds: the selected route's, or the first.
  Object get held => slotId(loading || routes.isEmpty ? 0 : selected);

  @override
  State<OptionList> createState() => _OptionListState();
}

class _OptionListState extends State<OptionList>
    with SingleTickerProviderStateMixin {
  static const _fade = Duration(milliseconds: 200);

  /// How many slots there are: [OptionList.count], or as many as there were until
  /// the rows no longer needed have faded.
  late int _slots = widget.count;

  /// Runs while slots that are no longer needed fade, a little longer than they take.
  late final _lingering =
      AnimationController(
        vsync: this,
        duration: const Duration(milliseconds: 250),
      )..addStatusListener((status) {
        if (status.isCompleted) setState(() => _slots = widget.count);
      });

  @override
  void didUpdateWidget(OptionList oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.count >= _slots) {
      _slots = widget.count;
      _lingering.stop();
    } else if (widget.count != oldWidget.count || !_lingering.isAnimating) {
      _lingering.forward(from: 0);
    }
  }

  @override
  void dispose() {
    _lingering.dispose();
    super.dispose();
  }

  /// What the slot at [index] shows, keyed by what it is, so one fades into another.
  Widget _row(int index, OptionRowMetrics metrics, RouteScale scale) {
    if (index >= widget.count) {
      return const SizedBox.shrink(key: ValueKey(#none));
    }
    if (widget.loading) {
      return OptionPlaceholder(
        key: const ValueKey(#placeholder),
        metrics: metrics,
      );
    }
    if (widget.routes.isEmpty) {
      return OptionMessage(
        key: ValueKey(widget.message),
        message: widget.message,
        metrics: metrics,
      );
    }
    return OptionRow(
      key: ObjectKey(widget.routes[index]),
      routes: widget.routes,
      index: index,
      scale: scale,
      selected: index == widget.selected,
      driving: widget.driving,
      units: widget.units,
      now: widget.now,
      metrics: metrics,
      onTap: () => widget.onSelect(index),
    );
  }

  @override
  Widget build(BuildContext context) {
    final metrics = OptionRowMetrics.of(context);
    final scale = RouteScale.of(widget.routes);
    final opening = math.min(widget.count, OptionList.shownAtFirst);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (var i = 0; i < _slots; i++) ...[
          SheetSlot(
            key: ValueKey(i),
            id: OptionList.slotId(i),
            child: SizedBox(
              height: metrics.height,
              child: FadeTransition(
                opacity: widget.reveal,
                child: AnimatedSwitcher(
                  duration: _fade,
                  switchInCurve: Curves.easeOut,
                  switchOutCurve: Curves.easeOut,
                  child: _row(i, metrics, scale),
                ),
              ),
            ),
          ),
          // Its end is the end of the last row the sheet first opens to show.
          if (i == opening - 1)
            SheetSection(
              key: const ValueKey(#opening),
              id: widget.opening,
              child: const SizedBox(width: double.infinity),
            ),
        ],
      ],
    );
  }
}
