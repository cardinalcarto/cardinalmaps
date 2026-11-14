/// What the navigation screen says about the trip itself, rather than the road ahead:
/// a status card in the maneuver banner's place while something is wrong or pending,
/// the arrival there at the end, and a small indicator when fixes have stopped
/// coming.
library;

import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoActivityIndicator;
import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../ui/adaptive.dart';
import '../ui/symbols.dart';
import 'maneuver_banner.dart';
import 'maneuver_icon.dart';
import 'navigation_controller.dart';
import 'navigation_location.dart';

/// What the navigation screen's status card says, in the maneuver banner's place (see
/// [navigationStatusOf]). Highest priority first: only the first that applies shows.
///
/// Ferrostar has none of these but iOS's "Return to the Route" for being off the route
/// (spec §2.8); here off the route is a status, "Off route", since a new route comes by
/// itself.
enum NavigationStatus {
  /// Fixes stopped, and [NavigationController.locationProblem] says why: what's wrong,
  /// with a button to put it right.
  locationProblem,

  /// A new route is being found.
  rerouting,

  /// The last try at a new route failed, and [NavigationController.rerouteFailure]
  /// says why; the controller keeps trying.
  rerouteFailed,

  /// Off the route, for the hold before a new route is asked for.
  offRoute,

  /// Waiting for a first fix good enough to start from.
  acquiring,
}

/// The status card [controller] calls for, or null when the banner shows the maneuver
/// (or, at the destination, the arrival).
///
/// A location problem comes first: nothing else can be put right while there are no
/// fixes. Then rerouting over a failure before it, since the controller is trying again
/// and that's the news; then the failure, while it waits to try again; then off the
/// route, before the first try; then waiting for GPS. None shows once arrived: the
/// trip is over, so whatever the fixes do no longer matters.
NavigationStatus? navigationStatusOf(NavigationController controller) {
  final state = controller.state;
  if (state == NavigationState.idle || state == NavigationState.arrived) {
    return null;
  }
  if (controller.locationProblem != null) {
    return NavigationStatus.locationProblem;
  }
  return switch (state) {
    NavigationState.rerouting => NavigationStatus.rerouting,
    NavigationState.offRoute =>
      controller.rerouteFailure != null
          ? NavigationStatus.rerouteFailed
          : NavigationStatus.offRoute,
    NavigationState.acquiring => NavigationStatus.acquiring,
    NavigationState.onRoute ||
    NavigationState.idle ||
    NavigationState.arrived => null,
  };
}

/// How long since the last fix came before the screen says the GPS signal is lost
/// ([StaleFixIndicator]).
///
/// Fixes come once a second, so this is four missed in a row: long enough that a fix
/// or two lost under a bridge or between tall buildings doesn't flash it on and off,
/// and short enough to say so within a block of a tunnel's mouth at city speeds.
const staleFixAge = Duration(seconds: 5);

/// Whether [controller]'s trip is one whose fixes going stale is worth saying: under
/// way, past waiting for its first fix (which the status card already says), with no
/// location problem (which its card says), and not yet arrived.
bool staleFixApplies(NavigationController controller) =>
    controller.locationProblem == null &&
    controller.lastFixAt != null &&
    switch (controller.state) {
      NavigationState.onRoute ||
      NavigationState.offRoute ||
      NavigationState.rerouting => true,
      NavigationState.idle ||
      NavigationState.acquiring ||
      NavigationState.arrived => false,
    };

/// The status card's content, in the maneuver banner's card ([ManeuverBanner.status]):
/// an icon in the banner's icon column, what's up, and under it a line more or a
/// button where there's something to say or do.
///
/// Rerouting shows a small progress indicator in the icon's place. A location problem
/// has a button: Open settings for location turned off or not allowed, and Try again
/// for location that failed, which [onAction] does.
///
/// It's announced to screen readers as it changes, as a live region.
class NavigationStatusContent extends StatelessWidget {
  const NavigationStatusContent({
    super.key,
    required this.status,
    this.problem,
    this.failure,
    this.onAction,
  });

  final NavigationStatus status;

  /// Why fixes stopped, for [NavigationStatus.locationProblem].
  final LocationProblem? problem;

  /// Why the last new route failed, for [NavigationStatus.rerouteFailed].
  final RerouteFailure? failure;

  /// What the location problem's button does.
  final VoidCallback? onAction;

  @override
  Widget build(BuildContext context) {
    final words = AppLocalizations.of(context);
    final symbols = AppSymbols.of(context);
    final (
      Widget leading,
      String title,
      String? detail,
      String? action,
    ) = switch (status) {
      NavigationStatus.locationProblem => switch (problem ??
          LocationProblem.failed) {
        LocationProblem.off => (
          _icon(context, symbols.locationOff),
          words.navigationLocationOff,
          words.navigationLocationOffDetail,
          words.navigationOpenSettings,
        ),
        LocationProblem.denied => (
          _icon(context, symbols.locationOff),
          words.navigationLocationDenied,
          words.navigationLocationDeniedDetail,
          words.navigationOpenSettings,
        ),
        LocationProblem.failed => (
          _icon(context, symbols.locationFailed),
          words.navigationLocationFailed,
          null,
          words.navigationTryAgain,
        ),
      },
      NavigationStatus.rerouting => (
        const _Progress(),
        words.navigationRerouting,
        null,
        null,
      ),
      NavigationStatus.rerouteFailed => (
        _icon(context, symbols.warning),
        switch (failure ?? RerouteFailure.failed) {
          RerouteFailure.noRoute => words.navigationRerouteNoRoute,
          RerouteFailure.outsideOfflineArea =>
            words.navigationRerouteOutsideArea,
          RerouteFailure.noOfflineArea => words.navigationRerouteNoOfflineArea,
          RerouteFailure.failed => words.navigationRerouteFailed,
        },
        words.navigationRerouteRetrying,
        null,
      ),
      NavigationStatus.offRoute => (
        _icon(context, symbols.offRoute),
        words.navigationOffRoute,
        null,
        null,
      ),
      NavigationStatus.acquiring => (
        _icon(context, symbols.waitingForGps),
        words.navigationWaitingForGps,
        null,
        null,
      ),
    };
    return Semantics(
      liveRegion: true,
      child: _StatusRow(
        leading: leading,
        title: title,
        detail: detail,
        action: action == null
            ? null
            : AppButton(label: action, onPressed: onAction),
      ),
    );
  }

  /// A status's icon: the size of the banner's secondary row's, in its icon column.
  static Widget _icon(BuildContext context, IconData icon) => Icon(
    icon,
    size: ManeuverBanner.secondaryIconSize,
    color: Theme.of(context).colorScheme.onSurfaceVariant,
  );
}

/// The arrival, in the maneuver banner's card ([ManeuverBanner.status]) once the trip
/// has arrived: the arrival icon, "You've arrived", and the destination's name under it
/// if it has one.
///
/// The icon is the route's own arrival maneuver's, [side] being the side of the road
/// it says the destination is on, or the plain arrival icon without one. It's the size
/// of the banner's maneuver icons, and the title has the banner instruction's type, so
/// it reads as the trip's last banner.
class ArrivalContent extends StatelessWidget {
  const ArrivalContent({super.key, required this.destinationName, this.side});

  final String? destinationName;

  /// The side the destination is on, from the route's arrival maneuver.
  final ManeuverModifier? side;

  /// The arrival maneuver's side of the road in [steps], a route's: the modifier of the
  /// last step's banner that arrives, if one does.
  static ManeuverModifier? sideOf(List<NavStep> steps) {
    for (final step in steps.reversed) {
      for (final visual in step.visualInstructions.reversed) {
        if (visual.primary.maneuverType == ManeuverType.arrive) {
          return visual.primary.maneuverModifier;
        }
      }
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final icon =
        maneuverIcon(ManeuverType.arrive, side) ??
        maneuverIcon(ManeuverType.arrive, null);
    return Semantics(
      liveRegion: true,
      child: _StatusRow(
        leading: Icon(
          icon,
          size: ManeuverBanner.iconSize,
          color: colors.onSurface,
        ),
        title: AppLocalizations.of(context).navigationArrived,
        detail: destinationName,
        prominent: true,
      ),
    );
  }
}

/// A status card's row, laid out as the banner's: [leading] in the icon column, then
/// [title], with [detail] and [action] under it.
///
/// The title has the banner instruction's type, in the on-surface color, as it's what
/// the card says rather than a maneuver's text under its distance; [prominent], for
/// the arrival, gives it the banner distance's type instead, and the detail the
/// instruction's. The detail otherwise has the secondary row's type.
class _StatusRow extends StatelessWidget {
  const _StatusRow({
    required this.leading,
    required this.title,
    this.detail,
    this.action,
    this.prominent = false,
  });

  final Widget leading;
  final String title;
  final String? detail;
  final Widget? action;
  final bool prominent;

  /// Between the text and the button under it.
  static const _actionGap = 8.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final text = theme.textTheme;
    final cupertino = isCupertinoOf(context);
    final (distanceStyle, instructionStyle) = ManeuverBanner.primaryStyles(
      context,
    );
    final titleStyle = prominent
        ? distanceStyle
        : instructionStyle?.copyWith(color: colors.onSurface);
    final detailStyle = prominent
        ? instructionStyle
        : (cupertino ? text.bodyMedium : text.bodyLarge)?.copyWith(
            color: colors.onSurfaceVariant,
          );
    final detail = this.detail;
    final action = this.action;
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: ManeuverBanner.sides,
        vertical: ManeuverBanner.ends,
      ),
      child: Row(
        children: [
          SizedBox(
            width: ManeuverBanner.iconSize,
            child: Center(heightFactor: 1, child: leading),
          ),
          const SizedBox(width: ManeuverBanner.gap),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(title, style: titleStyle),
                if (detail != null)
                  Text(
                    detail,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: detailStyle,
                  ),
                if (action != null) ...[
                  const SizedBox(height: _actionGap),
                  action,
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// The small progress indicator in the rerouting card's icon column: Material's
/// circular one on Android and the desktop, the activity indicator on iOS.
class _Progress extends StatelessWidget {
  const _Progress();

  static const size = 24.0;

  @override
  Widget build(BuildContext context) {
    if (isCupertinoOf(context)) {
      return const CupertinoActivityIndicator(radius: size / 2);
    }
    return const SizedBox.square(
      dimension: size,
      child: CircularProgressIndicator(strokeWidth: 3),
    );
  }
}

/// A small pill saying "GPS signal lost" once [staleFixAge] has gone by with no new
/// fix, while [applies] (see [staleFixApplies]); it goes as soon as a fix comes.
///
/// The age is timed on the device's own steady clock from when the screen saw
/// [lastFixAt] change, rather than from the fix's timestamp: GPS time and the phone's
/// clock can disagree by seconds, which would show it all the time, or never.
///
/// It isn't a status card, as the trip carries on with the last fix: it sits under
/// the map buttons, at the end side, clear of the banner and the chevron. Material on
/// Android and the desktop, a raised surface; on iOS, the trip-progress pill's shadow.
class StaleFixIndicator extends StatefulWidget {
  const StaleFixIndicator({
    super.key,
    required this.lastFixAt,
    required this.applies,
  });

  /// When the last fix was taken; a change means one came.
  final DateTime? lastFixAt;
  final bool applies;

  @override
  State<StaleFixIndicator> createState() => _StaleFixIndicatorState();
}

class _StaleFixIndicatorState extends State<StaleFixIndicator> {
  Timer? _timer;
  bool _stale = false;

  @override
  void initState() {
    super.initState();
    _restart();
  }

  @override
  void didUpdateWidget(StaleFixIndicator old) {
    super.didUpdateWidget(old);
    if (widget.lastFixAt != old.lastFixAt || widget.applies != old.applies) {
      _restart();
    }
  }

  /// A fix came, or whether it applies changed: not stale, until [staleFixAge] goes by.
  void _restart() {
    _timer?.cancel();
    _timer = null;
    _stale = false;
    if (!widget.applies || widget.lastFixAt == null) return;
    _timer = Timer(staleFixAge, () {
      if (mounted) setState(() => _stale = true);
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  static const _sides = 12.0;
  static const _ends = 6.0;

  @override
  Widget build(BuildContext context) {
    if (!_stale) return const SizedBox.shrink();
    final theme = Theme.of(context);
    final colors = theme.colorScheme;
    final cupertino = isCupertinoOf(context);
    final content = Semantics(
      liveRegion: true,
      child: Padding(
        padding: const EdgeInsets.symmetric(
          horizontal: _sides,
          vertical: _ends,
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          spacing: 6,
          children: [
            Icon(AppSymbols.of(context).gpsLost, size: 18, color: colors.error),
            Text(
              AppLocalizations.of(context).navigationGpsLost,
              style: theme.textTheme.labelLarge?.copyWith(
                color: colors.onSurface,
              ),
            ),
          ],
        ),
      ),
    );
    const shape = StadiumBorder();
    if (cupertino) {
      return DecoratedBox(
        decoration: ShapeDecoration(
          color: colors.surface,
          shape: shape,
          shadows: const [BoxShadow(color: Color(0x55000000), blurRadius: 12)],
        ),
        child: content,
      );
    }
    return Material(
      color: colors.surface,
      elevation: 3,
      shape: shape,
      child: content,
    );
  }
}
