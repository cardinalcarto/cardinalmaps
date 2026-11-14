import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoButton, CupertinoIcons;
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../ui/adaptive.dart';
import '../units.dart';
import 'navigation_format.dart';

/// The pill along the bottom of the navigation screen: when you'll arrive, how long
/// that is, how far it is, and End.
///
/// It's Ferrostar's trip progress (spec §2.3) in its default, simplified style: the
/// three values without captions, in that order, then the exit button, a close glyph.
/// The values share the width in equal columns, each centered, as Ferrostar Android lays
/// them out (`TripProgressView.kt:76-138`), and each shrinks to fit rather than being
/// cut off, as on iOS (`minimumScaleFactor`), which the spec recommends together (§4).
///
/// Material on Android and the desktop takes Android's type and its round exit button,
/// in the theme's secondary color; Cupertino takes iOS's type, its capsule exit button
/// and its shadow, as the maneuver banner does. Both are tighter than Ferrostar's
/// padding, to match the banner.
///
/// The arrival time is now plus [progress]'s time left, worked out on each build, as
/// Ferrostar does, so it moves as the trip runs late or early.
///
/// Without [progress], before the trip's first fix, only End shows: Ferrostar shows no
/// trip progress then, but End has to be there to leave by. Once [arrived], the pill
/// stays until you End, showing nothing left to go and the time you arrived (the time
/// this first saw it), where Ferrostar takes its whole overlay away.
class TripProgressPill extends StatefulWidget {
  const TripProgressPill({
    super.key,
    required this.progress,
    required this.arrived,
    required this.units,
    required this.onEnd,
    this.clock = DateTime.now,
  });

  /// How far and how long is left; null before the trip has a fix, and once arrived.
  final NavProgress? progress;

  /// Whether the trip is over, at the destination.
  final bool arrived;

  /// [UnitSystem.metric] or [UnitSystem.imperial], resolved for the device's region.
  final UnitSystem units;

  /// Ends the trip.
  final VoidCallback onEnd;

  /// What time it is, which tests set.
  final DateTime Function() clock;

  @override
  State<TripProgressPill> createState() => _TripProgressPillState();
}

class _TripProgressPillState extends State<TripProgressPill> {
  /// When the pill first saw the trip arrived, which it keeps showing as the arrival
  /// time; null while it hasn't.
  DateTime? _arrivedAt;

  @override
  void initState() {
    super.initState();
    if (widget.arrived) _arrivedAt = widget.clock();
  }

  @override
  void didUpdateWidget(TripProgressPill old) {
    super.didUpdateWidget(old);
    if (!widget.arrived) {
      _arrivedAt = null;
    } else {
      _arrivedAt ??= widget.clock();
    }
  }

  /// Between the pill's edge and what's in it: Ferrostar has 12 all round on Android,
  /// and on iOS 32 at the start, 12 at the end and 8 above and below.
  static const _start = 16.0;
  static const _end = 6.0;
  static const _ends = 6.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final text = theme.textTheme;
    final cupertino = isCupertinoOf(context);
    final words = AppLocalizations.of(context);

    final arrivedAt = _arrivedAt;
    final progress = widget.progress;
    final List<String>? values;
    if (arrivedAt != null) {
      values = [
        formatArrivalTime(context, arrivedAt),
        formatTripDuration(0, words),
        formatManeuverDistance(0, units: widget.units, words: words),
      ];
    } else if (progress != null) {
      final seconds = progress.durationRemaining;
      final eta = widget.clock().add(
        Duration(
          milliseconds: seconds.isFinite && seconds > 0
              ? (seconds * 1000).round()
              : 0,
        ),
      );
      values = [
        formatArrivalTime(context, eta),
        formatTripDuration(seconds, words),
        formatManeuverDistance(
          progress.distanceRemaining,
          units: widget.units,
          words: words,
        ),
      ];
    } else {
      values = null;
    }

    // iOS: Title 2 bold, the primary label color (Title 3's size standing in, as in
    // the banner). Android: title medium, semibold.
    final style =
        (cupertino
                ? text.titleLarge?.copyWith(fontWeight: FontWeight.w700)
                : text.titleMedium?.copyWith(fontWeight: FontWeight.w600))
            ?.copyWith(color: colors.onSurface);
    final row = Padding(
      padding: const EdgeInsetsDirectional.fromSTEB(_start, _ends, _end, _ends),
      child: Row(
        children: [
          if (values == null)
            const Spacer()
          else
            for (final value in values)
              Expanded(
                child: Center(
                  child: FittedBox(
                    fit: BoxFit.scaleDown,
                    child: Text(value, maxLines: 1, style: style),
                  ),
                ),
              ),
          _EndButton(onPressed: widget.onEnd),
        ],
      ),
    );
    const shape = StadiumBorder();
    if (cupertino) {
      // iOS's pill: corners of radius 48, more than half its height, and a shadow of
      // radius 12, as the banner's.
      return DecoratedBox(
        decoration: ShapeDecoration(
          color: colors.surface,
          shape: shape,
          shadows: const [BoxShadow(color: Color(0x55000000), blurRadius: 12)],
        ),
        child: row,
      );
    }
    return Material(
      color: colors.surface,
      elevation: 3,
      shape: shape,
      child: row,
    );
  }
}

/// Ferrostar's exit button: on Android a round button in the secondary color with a
/// close icon (`TripProgressView.kt:119-134`, 50 across), on iOS an `xmark` in a capsule
/// of the secondary background. Tighter here: 48 across, the least Material taps, and
/// 44 on iOS, the least Apple's are.
class _EndButton extends StatelessWidget {
  const _EndButton({required this.onPressed});

  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    // End's accessibility label and tooltip: what the text button it replaces said.
    final label = AppLocalizations.of(context).navigationEnd;
    if (!isCupertinoOf(context)) {
      return IconButton(
        tooltip: label,
        onPressed: onPressed,
        style: IconButton.styleFrom(
          backgroundColor: colors.secondary,
          foregroundColor: colors.onSecondary,
          fixedSize: const Size.square(48),
        ),
        icon: const Icon(Icons.close),
      );
    }
    return Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: CupertinoButton(
        padding: EdgeInsets.zero,
        minimumSize: const Size.square(44),
        onPressed: onPressed,
        child: DecoratedBox(
          // iOS's secondary system background: the iOS theme's container low, as in
          // the banner.
          decoration: ShapeDecoration(
            color: colors.surfaceContainerLow,
            shape: const StadiumBorder(),
          ),
          child: SizedBox.square(
            dimension: 44,
            child: Icon(
              CupertinoIcons.xmark,
              size: 18,
              weight: 700,
              color: colors.onSurface,
            ),
          ),
        ),
      ),
    );
  }
}
