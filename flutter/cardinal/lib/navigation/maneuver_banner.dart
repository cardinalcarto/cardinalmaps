import 'dart:math' as math;

import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../ui/adaptive.dart';
import '../ui/symbols.dart';
import '../units.dart';
import 'lanes.dart';
import 'maneuver_icon.dart';
import 'navigation_format.dart';
import 'step_list.dart';

/// The banner at the top of the navigation screen: the next maneuver's icon, how far
/// off it is, and what to do there, and under that, when there is one, the maneuver
/// right after it. It opens into the list of the trip's steps after that one
/// ([StepList]).
///
/// It's Ferrostar iOS's instructions banner (spec §2.1, `InstructionsView` in
/// FerrostarSwiftUI): iOS's rather than Android's because only iOS shows the secondary
/// row, and only iOS's opens and closes by dragging. Each row is an icon column, then
/// the text: the primary row has the distance above two lines at most of the
/// instruction, and the secondary row, on a band of its own, just its instruction.
///
/// Ferrostar shows neither lanes nor exit numbers on a phone, so these are the app's
/// own:
///
/// - When the banner's sub content has lanes, a [LaneStrip] shows under the primary
///   row, centered, above the secondary row if there is one: the sub content is lane
///   guidance for the primary maneuver. Sub content without lanes isn't shown; the
///   router gives none.
/// - A row whose content has exit numbers starts its text with an [ExitBadge], as
///   exit signs put the number above the destination.
///
/// ## The step list
///
/// With [upcoming] steps to list, a handle shows along the banner's bottom, and the
/// banner opens as iOS's `TopDrawerView` does (spec §1.4):
///
/// - a tap on the handle, or on the banner itself, opens it, and another closes it,
///   easing in and out over [ManeuverBanner.toggleDuration];
/// - dragging the handle down pulls the list out after the finger, and dragging it up
///   when open pushes it back; let go, it opens if the drag would have carried on past
///   [ManeuverBanner.dragThreshold] down, and closes if past as far up, as iOS
///   projects a fling;
/// - open, it fills the height it's given, the banner's rows staying at the top as
///   they were, with the list scrolling under them and the handle, now a chevron, at
///   the bottom.
///
/// The handle's drag starts anywhere from 20 above it down, as iOS's does, so it needn't
/// be hit exactly. It's Ferrostar's on each platform: Android's 24 × 4 pill and
/// chevron on Material, iOS's 40 × 6 capsule and bold chevron on Cupertino. Unlike
/// iOS's, it has a row of its own under the list, rather than blurring the list's end
/// under it; and closed, it sits in a 12-high strip at the bottom, the bottom row's
/// padding giving up 4 to it, where iOS adds 6 below a 16 padding.
///
/// Whether it's open is [expanded]'s, which the screen holds, as iOS's `isExpanded`
/// binding is held by its overlay, so it stays open across a new route or a turn of
/// the screen; set from outside, the banner opens or closes as for a tap. With nothing
/// to list, as at the last step, there's no handle, and it shows closed.
///
/// Nothing shows without an [instruction], as in Ferrostar: before the trip has one,
/// and between routes.
///
/// ## Status
///
/// While there's a [status], it shows in the banner's card instead of the maneuver,
/// with no handle: the navigation screen's status card or its arrival
/// (`NavigationStatusContent`, `ArrivalContent`). The list doesn't show under it, but
/// [expanded] keeps what it was, so the banner comes back as it was once the status
/// goes, as it does across a new route. [collapsedHeight] is the status card's height
/// meanwhile.
///
/// Material on Android and the desktop takes its type from Ferrostar Android's
/// (`InstructionRowTheme.kt`); Cupertino takes iOS's, and its card's corners and
/// shadow. Both are tighter than Ferrostar's 16 around each row.
class ManeuverBanner extends StatefulWidget {
  const ManeuverBanner({
    super.key,
    required this.instruction,
    required this.distance,
    required this.units,
    this.upcoming = const [],
    this.expanded,
    this.collapsedHeight,
    this.status,
    this.drivingSide,
  });

  final NavVisualInstruction? instruction;

  /// Which side of the road traffic keeps to on the current step, which the lanes'
  /// U-turn arrows turn by.
  final NavDrivingSide? drivingSide;

  /// What shows in the banner's card instead of the maneuver and its list, while set
  /// (see the class's description).
  final Widget? status;

  /// How far the maneuver is, in meters; null leaves the distance out.
  final double? distance;

  /// [UnitSystem.metric] or [UnitSystem.imperial], resolved for the device's region.
  final UnitSystem units;

  /// The route's steps after the current one, which the banner opens to list (see
  /// [upcomingSteps]); those without a banner of their own, as the route's last, aren't
  /// listed.
  final List<NavStep> upcoming;

  /// Whether the list is open; the banner keeps its own if it's not given.
  final ValueNotifier<bool>? expanded;

  /// Set to how tall the banner is closed, the handle included, once it's laid out,
  /// and to 0 without an [instruction]: what the screen lays out the controls under,
  /// which an open banner covers rather than pushes down, as on iOS (iOS's
  /// `sizeWhenNotExpanded`).
  final ValueNotifier<double>? collapsedHeight;

  /// The icon's size, and its column's width: iOS draws the icon up to 48 wide.
  static const iconSize = 48.0;

  /// The secondary row's icon: smaller than the primary's, so the row reads as the
  /// lesser one, in a column as wide as the primary's so the texts still line up.
  static const secondaryIconSize = 32.0;

  /// How long a tap takes to open or close the list: iOS's `easeInOut(duration: 0.2)`.
  static const toggleDuration = Duration(milliseconds: 200);

  /// How far a drag must be headed, down to open or up to close, to change the list:
  /// iOS's 200.
  static const dragThreshold = 200.0;

  /// The closed handle's strip, and the open handle's row: Ferrostar Android's 36.
  static const collapsedHandleHeight = 12.0;
  static const expandedHandleHeight = 36.0;

  /// How far above the handle its drag starts: iOS's 20.
  static const handleReach = 20.0;

  /// Between the icon column and the text.
  static const gap = 12.0;

  /// Around each row's content: Ferrostar has 16 at the sides and above and below,
  /// and 8 more between the rows.
  static const sides = 12.0;
  static const ends = 10.0;
  static const _between = 6.0;

  /// The primary row's distance style and instruction style, which the step list's
  /// rows share.
  ///
  /// iOS: Title 1 bold for the distance, Title 2 in the secondary label color for the
  /// instruction (Title 3's size standing in, the app's theme having no Title 2).
  /// Android: title large for the distance, headline small for the instruction.
  static (TextStyle?, TextStyle?) primaryStyles(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final text = theme.textTheme;
    final cupertino = isCupertinoOf(context);
    return (
      (cupertino ? text.headlineMedium : text.titleLarge)?.copyWith(
        color: colors.onSurface,
      ),
      (cupertino
              ? text.titleLarge?.copyWith(fontWeight: FontWeight.w400)
              : text.headlineSmall)
          ?.copyWith(color: colors.onSurfaceVariant),
    );
  }

  @override
  State<ManeuverBanner> createState() => _ManeuverBannerState();
}

class _ManeuverBannerState extends State<ManeuverBanner>
    with SingleTickerProviderStateMixin {
  /// How far open the list is, from 0 closed to 1 open.
  late final _open = AnimationController(
    vsync: this,
    duration: ManeuverBanner.toggleDuration,
  );

  ValueNotifier<bool>? _own;
  ValueNotifier<bool> get _expanded =>
      widget.expanded ?? (_own ??= ValueNotifier(false));

  /// How tall the banner's rows are, as last laid out.
  double _rowsHeight = 0;

  /// The most the banner may be, as last laid out.
  double _maxHeight = 0;

  /// How tall the list is open.
  double get _full => math.max(
    0,
    _maxHeight - _rowsHeight - ManeuverBanner.expandedHandleHeight,
  );

  /// Where a drag of the handle started from, and how far it's gone.
  double _dragFrom = 0;
  double _dragged = 0;

  @override
  void initState() {
    super.initState();
    _expanded.addListener(_expandedChanged);
    _open.value = _expanded.value ? 1 : 0;
  }

  @override
  void didUpdateWidget(ManeuverBanner old) {
    super.didUpdateWidget(old);
    final was = old.expanded ?? _own!;
    if (was != _expanded) {
      was.removeListener(_expandedChanged);
      _expanded.addListener(_expandedChanged);
      _expandedChanged();
    }
  }

  @override
  void dispose() {
    _expanded.removeListener(_expandedChanged);
    _own?.dispose();
    _open.dispose();
    super.dispose();
  }

  void _expandedChanged() {
    if (!mounted) return;
    _settle();
    // The handle turns at once, as iOS's does, while the list eases.
    setState(() {});
  }

  /// Eases the list open or closed, to where [_expanded] says.
  void _settle() =>
      _open.animateTo(_expanded.value ? 1 : 0, curve: Curves.easeInOut);

  void _toggle() => _expanded.value = !_expanded.value;

  void _dragStart(DragStartDetails _) {
    _open.stop();
    _dragFrom = _open.value;
    _dragged = 0;
  }

  void _dragUpdate(DragUpdateDetails details) {
    _dragged += details.primaryDelta ?? 0;
    if (_full <= 0) return;
    _open.value = (_dragFrom + _dragged / _full).clamp(0.0, 1.0);
  }

  /// Opens or closes by where the drag would have stopped, as iOS projects a drag's
  /// end.
  void _dragEnd(DragEndDetails details) {
    final projected =
        _dragged + (details.primaryVelocity ?? 0) * _projectedSeconds;
    final open = _expanded.value
        ? projected > -ManeuverBanner.dragThreshold
        : projected > ManeuverBanner.dragThreshold;
    if (open == _expanded.value) {
      _settle();
    } else {
      _expanded.value = open;
    }
  }

  /// How long a fling's speed carries a drag on, projected: UIKit's normal
  /// deceleration, 0.998 a millisecond, comes to just under half a second.
  static const _projectedSeconds = 0.998 / (1 - 0.998) / 1000;

  void _rowsLaidOut(double height) {
    if (height == _rowsHeight) return;
    _rowsHeight = height;
    _report();
  }

  /// How tall [ManeuverBanner.status]'s card is, as last laid out.
  double _statusHeight = 0;

  void _statusLaidOut(double height) {
    if (height == _statusHeight) return;
    _statusHeight = height;
    _report();
  }

  /// Tells [ManeuverBanner.collapsedHeight] how tall the banner is closed, once the
  /// frame's laid out.
  void _report() {
    final notifier = widget.collapsedHeight;
    if (notifier == null) return;
    SchedulerBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      notifier.value = widget.status != null
          ? _statusHeight
          : widget.instruction == null
          ? 0
          : _rowsHeight +
                (_listed.isEmpty ? 0 : ManeuverBanner.collapsedHandleHeight);
    });
  }

  List<NavStep> get _listed => [
    for (final step in widget.upcoming)
      if (step.visualInstructions.isNotEmpty) step,
  ];

  @override
  Widget build(BuildContext context) {
    _report();
    final status = widget.status;
    if (status != null) {
      return _Card(
        child: _ReportHeight(onHeight: _statusLaidOut, child: status),
      );
    }
    final instruction = widget.instruction;
    if (instruction == null) return const SizedBox.shrink();
    final colors = Theme.of(context).colorScheme;
    final cupertino = isCupertinoOf(context);
    final listed = _listed;
    final hasList = listed.isNotEmpty;
    final band = cupertino
        ? colors.surfaceContainerLow
        : colors.surfaceContainerHigh;

    Widget rows = _ReportHeight(
      onHeight: _rowsLaidOut,
      child: _BannerRows(
        instruction: instruction,
        distance: widget.distance,
        units: widget.units,
        drivingSide: widget.drivingSide,
        band: band,
        // The handle's strip takes some of the bottom row's padding.
        bottom: hasList
            ? ManeuverBanner.ends -
                  (ManeuverBanner.collapsedHandleHeight -
                      _Handle.collapsedTop -
                      _Handle.collapsedBelow)
            : ManeuverBanner.ends,
      ),
    );
    if (!hasList) return _Card(child: rows);
    rows = cupertino
        ? GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: _toggle,
            child: rows,
          )
        : InkWell(onTap: _toggle, child: rows);
    final words = AppLocalizations.of(context);

    return LayoutBuilder(
      builder: (context, constraints) => AnimatedBuilder(
        animation: _open,
        builder: (context, _) {
          final expanded = _expanded.value;
          final handleHeight = expanded
              ? ManeuverBanner.expandedHandleHeight
              : ManeuverBanner.collapsedHandleHeight;
          final height = constraints.maxHeight;
          _maxHeight = height.isFinite ? height : 0;
          final open = _open.value;
          final list = StepList(steps: listed, units: widget.units);
          return _Card(
            child: Stack(
              children: [
                Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    rows,
                    if (open >= 1)
                      Expanded(child: list)
                    else if (open > 0)
                      Flexible(
                        child: SizedBox(height: open * _full, child: list),
                      ),
                    _Handle(
                      expanded: expanded,
                      height: handleHeight,
                      // Closed, it's on the secondary row's band, which it ends.
                      color:
                          !expanded &&
                              open == 0 &&
                              instruction.secondary != null
                          ? band
                          : colors.surface,
                    ),
                  ],
                ),
                Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  height: handleHeight + ManeuverBanner.handleReach,
                  child: Semantics(
                    button: true,
                    label: expanded
                        ? words.navigationHideSteps
                        : words.navigationShowSteps,
                    onTap: _toggle,
                    excludeSemantics: true,
                    child: GestureDetector(
                      behavior: HitTestBehavior.translucent,
                      onTap: _toggle,
                      onVerticalDragStart: _dragStart,
                      onVerticalDragUpdate: _dragUpdate,
                      onVerticalDragEnd: _dragEnd,
                    ),
                  ),
                ),
              ],
            ),
          );
        },
      ),
    );
  }
}

/// The banner's card: iOS's drawer on Cupertino, corner radius 12 and a shadow of
/// radius 12, SwiftUI's default shadow color being black at a third; a raised Material
/// surface elsewhere.
class _Card extends StatelessWidget {
  const _Card({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    const radius = BorderRadius.all(Radius.circular(12));
    if (isCupertinoOf(context)) {
      return DecoratedBox(
        decoration: BoxDecoration(
          color: colors.surface,
          borderRadius: radius,
          boxShadow: const [
            BoxShadow(color: Color(0x55000000), blurRadius: 12),
          ],
        ),
        child: ClipRRect(borderRadius: radius, child: child),
      );
    }
    return Material(
      color: colors.surface,
      elevation: 3,
      borderRadius: radius,
      clipBehavior: Clip.antiAlias,
      child: child,
    );
  }
}

/// The banner's rows: the maneuver, and the one after it on a [band] of its own if
/// there's one, with [bottom] below the last.
class _BannerRows extends StatelessWidget {
  const _BannerRows({
    required this.instruction,
    required this.distance,
    required this.units,
    required this.drivingSide,
    required this.band,
    required this.bottom,
  });

  final NavVisualInstruction instruction;
  final double? distance;
  final UnitSystem units;
  final NavDrivingSide? drivingSide;
  final Color band;
  final double bottom;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final text = theme.textTheme;
    final cupertino = isCupertinoOf(context);
    final secondary = instruction.secondary;
    final lanes = instruction.sub?.lanes ?? const <NavLane>[];
    final distance = this.distance;
    // The bottom padding of the last row before the secondary band, or of the last.
    final beforeBand = secondary == null ? bottom : ManeuverBanner._between;
    final (distanceStyle, textStyle) = ManeuverBanner.primaryStyles(context);
    const sides = ManeuverBanner.sides;

    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: EdgeInsets.fromLTRB(
            sides,
            ManeuverBanner.ends,
            sides,
            lanes.isEmpty ? beforeBand : ManeuverBanner._between,
          ),
          child: ManeuverRow(
            content: instruction.primary,
            distance: distance == null
                ? null
                : formatManeuverDistance(
                    distance,
                    units: units,
                    words: AppLocalizations.of(context),
                  ),
            distanceStyle: distanceStyle,
            textStyle: textStyle,
          ),
        ),
        if (lanes.isNotEmpty)
          Padding(
            padding: EdgeInsets.fromLTRB(sides, 0, sides, beforeBand),
            child: Center(
              child: LaneStrip(lanes: lanes, drivingSide: drivingSide),
            ),
          ),
        if (secondary != null)
          ColoredBox(
            // iOS's secondary system background: the iOS theme's container low.
            color: band,
            child: Padding(
              padding: EdgeInsets.fromLTRB(
                sides,
                ManeuverBanner._between,
                sides,
                bottom,
              ),
              child: ManeuverRow(
                content: secondary,
                iconSize: ManeuverBanner.secondaryIconSize,
                distance: null,
                distanceStyle: null,
                // iOS's Subheadline.
                textStyle: (cupertino ? text.bodyMedium : text.bodyLarge)
                    ?.copyWith(color: colors.onSurfaceVariant),
              ),
            ),
          ),
      ],
    );
  }
}

/// The handle along the banner's bottom: closed, Ferrostar Android's 24 × 4 pill in
/// the on-surface color on Material, and iOS's 40 × 6 capsule in a gray at 70% on
/// Cupertino (the theme's on-surface variant standing in for the gray); open, a
/// chevron pointing up, Android's 24 and iOS's bolder 28 (iOS has 32, in a row
/// taller than this one's 36).
class _Handle extends StatelessWidget {
  const _Handle({
    required this.expanded,
    required this.height,
    required this.color,
  });

  final bool expanded;
  final double height;
  final Color color;

  /// Above the closed pill, and below it.
  static const collapsedTop = 2.0;
  static const collapsedBelow = 6.0;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final cupertino = isCupertinoOf(context);
    final tint = cupertino
        ? colors.onSurfaceVariant.withValues(alpha: 0.7)
        : colors.onSurface;
    final Widget mark;
    if (expanded) {
      mark = Center(
        child: Icon(
          AppSymbols.of(context).collapse,
          size: cupertino ? 28 : 24,
          color: tint,
          weight: cupertino ? 700 : null,
        ),
      );
    } else {
      final (width, thickness) = cupertino ? (40.0, 6.0) : (24.0, 4.0);
      mark = Align(
        alignment: Alignment.bottomCenter,
        child: Padding(
          padding: const EdgeInsets.only(bottom: collapsedBelow),
          child: Container(
            width: width,
            height: thickness,
            decoration: BoxDecoration(
              color: tint,
              borderRadius: BorderRadius.circular(thickness / 2),
            ),
          ),
        ),
      );
    }
    return ColoredBox(
      color: color,
      child: SizedBox(height: height, child: mark),
    );
  }
}

/// One row of the banner, or of the step list under it: [content]'s icon, then
/// [distance] above its text.
class ManeuverRow extends StatelessWidget {
  const ManeuverRow({
    super.key,
    required this.content,
    this.iconSize = ManeuverBanner.iconSize,
    required this.distance,
    required this.distanceStyle,
    required this.textStyle,
  });

  final NavInstructionContent content;

  /// The icon's size; its column is [ManeuverBanner.iconSize] wide whatever it is.
  final double iconSize;
  final String? distance;
  final TextStyle? distanceStyle;
  final TextStyle? textStyle;

  @override
  Widget build(BuildContext context) {
    final icon = maneuverIcon(content.maneuverType, content.maneuverModifier);
    final distance = this.distance;
    return Row(
      children: [
        // The column stays without an icon, as Ferrostar's does, so the text lines up
        // from one maneuver to the next.
        SizedBox(
          width: ManeuverBanner.iconSize,
          child: icon == null
              ? null
              : Icon(
                  icon,
                  size: iconSize,
                  color: Theme.of(context).colorScheme.onSurface,
                ),
        ),
        const SizedBox(width: ManeuverBanner.gap),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (distance != null) Text(distance, style: distanceStyle),
              Text.rich(
                TextSpan(
                  children: [
                    if (content.exitNumbers.isNotEmpty) ...[
                      WidgetSpan(
                        alignment: PlaceholderAlignment.middle,
                        child: ExitBadge(numbers: content.exitNumbers),
                      ),
                      const TextSpan(text: ' '),
                    ],
                    TextSpan(text: content.text),
                  ],
                ),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: textStyle,
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// The exit to take, as a badge before a maneuver's text: "Exit 154B", in a rounded
/// rectangle of the theme's primary color, the app's stand-in for the exit tab on a
/// highway sign. Several numbers, as for an exit signed 1A and 1B, are joined by a
/// slash.
class ExitBadge extends StatelessWidget {
  const ExitBadge({super.key, required this.numbers});

  final List<String> numbers;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: colors.primary,
        borderRadius: const BorderRadius.all(Radius.circular(6)),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
        child: Text(
          AppLocalizations.of(context).navigationExitNumber(numbers.join('/')),
          maxLines: 1,
          style: theme.textTheme.labelLarge?.copyWith(
            color: colors.onPrimary,
            fontWeight: FontWeight.w700,
          ),
        ),
      ),
    );
  }
}

/// Tells [onHeight] how tall [child] is each time it's laid out.
class _ReportHeight extends SingleChildRenderObjectWidget {
  const _ReportHeight({required this.onHeight, super.child});

  final ValueChanged<double> onHeight;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderReportHeight(onHeight);

  @override
  void updateRenderObject(
    BuildContext context,
    _RenderReportHeight renderObject,
  ) => renderObject.onHeight = onHeight;
}

class _RenderReportHeight extends RenderProxyBox {
  _RenderReportHeight(this.onHeight);

  ValueChanged<double> onHeight;

  @override
  void performLayout() {
    super.performLayout();
    onHeight(size.height);
  }
}
