/// The navigation screen: the map, full-bleed, following you along the trip.
///
/// It's a page of the sheet ([NavigationPage]), over the directions it started from,
/// which hides the sheet; what it shows over the map ([NavigationView]) is in the
/// sheet scaffold's floating layer, through [NavigationLayer]. So the map is the app's
/// one map, in its style, and going back to directions finds them as they were.
///
/// The layout follows Ferrostar's in each orientation (see `NavigationView`); for now
/// it holds the maneuver banner, which opens into the trip's steps and gives way to a
/// status card or the arrival, the camera and mute buttons with the GPS-lost
/// indicator under them, the road-name pill, and the trip-progress pill with End.
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../directions/route_source.dart';
import '../l10n/app_localizations.dart';
import '../map/map.dart';
import '../map/map_control.dart';
import '../sheet/sheet.dart';
import '../store/store.dart';
import '../ui/adaptive.dart';
import '../ui/glass.dart';
import '../ui/symbols.dart';
import '../units.dart';
import 'maneuver_banner.dart';
import 'navigation_camera.dart';
import 'navigation_controller.dart';
import 'navigation_location.dart';
import 'navigation_map.dart';
import 'navigation_scope.dart';
import 'navigation_status.dart';
import 'road_name.dart';
import 'step_list.dart';
import 'trip_progress.dart';

/// The navigation screen's page in the sheet: it hides the sheet and shows the
/// navigation screen over the map while it's open.
///
/// End, and back, end the trip, and the page goes back to the directions under it
/// whenever the trip ends, however it's ended: by End on the navigation notification
/// too, at once if the app is showing, or for when it's next opened. Opened with no
/// trip under way, as when the app is restored to it after being closed, it goes back
/// at once.
class NavigationPage extends StatefulWidget {
  const NavigationPage({super.key});

  @override
  State<NavigationPage> createState() => _NavigationPageState();
}

class _NavigationPageState extends State<NavigationPage> {
  VoidCallback? _hide;
  bool _checked = false;

  /// The controller whose trip ending takes the page back, while it's listened to.
  NavigationController? _controller;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_checked) return;
    _checked = true;
    final scope = NavigationScope.of(context);
    final idle = scope.controller.state == NavigationState.idle;
    if (!idle) _controller = scope.controller..addListener(_tripChanged);
    // After the frame: showing and hiding rebuild the floating layer, which can't be
    // changed while this page is built or taken away.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      if (idle) {
        _leave();
      } else {
        _hide = scope.screen.show();
      }
    });
  }

  /// The trip ended: back to directions, once, if this page is still the one showing.
  void _tripChanged() {
    final controller = _controller;
    if (controller == null || controller.state != NavigationState.idle) return;
    controller.removeListener(_tripChanged);
    _controller = null;
    if (mounted && (ModalRoute.of(context)?.isCurrent ?? true)) _leave();
  }

  @override
  void dispose() {
    _controller?.removeListener(_tripChanged);
    final hide = _hide;
    if (hide != null) {
      WidgetsBinding.instance
        ..addPostFrameCallback((_) => hide())
        ..scheduleFrame();
    }
    super.dispose();
  }

  void _leave() => context.canPop() ? context.pop() : context.go('/');

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: false,
    onPopInvokedWithResult: (didPop, _) {
      if (!didPop) endNavigation(context);
    },
    // Down out of sight, and held there.
    child: SheetContent(
      anchors: const [SheetAnchor.fraction(0)],
      draggable: false,
      slivers: const [],
    ),
  );
}

/// Ends the trip, which takes the navigation screen back to the directions under it
/// ([NavigationPage]).
void endNavigation(BuildContext context) =>
    unawaited(NavigationScope.of(context).controller.end());

/// The sheet scaffold's floating layer: [child], the map's own buttons, and, while the
/// navigation screen shows, [NavigationView] instead. [child] stays built while hidden,
/// so the buttons come back as they were.
class NavigationLayer extends StatelessWidget {
  const NavigationLayer({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final screen = NavigationScope.of(context).screen;
    return ListenableBuilder(
      listenable: screen,
      builder: (context, _) {
        final shown = screen.shown;
        return Stack(
          fit: StackFit.expand,
          children: [
            Offstage(offstage: shown, child: child),
            if (shown) const NavigationView(),
          ],
        );
      },
    );
  }
}

/// What the navigation screen shows over the map, and the trip drawn on it (see
/// [NavigationMap]).
///
/// Laid out as Ferrostar lays out its navigation overlay (spec §1):
///
/// - in portrait, the maneuver banner across the top, the camera button at the end
///   16 px under it and the mute button 8 under that, trip progress along the bottom,
///   and the road-name pill just above it, in the middle;
/// - in landscape (wider than tall, as iOS tells it), the banner at the top of the start
///   half with trip progress at its bottom, as Android splits the screen, the camera and
///   mute buttons at the top of the end half, and the road-name pill at the bottom of
///   the end half, under the chevron.
///
/// The camera button is iOS's (spec §2.5): Route overview while following, which shows
/// the rest of the trip ([NavigationMap.overview]) clear of the banner, trip progress
/// and the buttons themselves, and Recenter once in overview or moved by hand. The mute button shows
/// whenever navigating, as on iOS, where Android hides it with the map moved; it mutes
/// the controller ([NavigationController.setMuted]), so the trip stays muted whatever
/// shows it. There are no zoom buttons, which Ferrostar has: the camera zooms with
/// your speed, which they'd fight.
///
/// The maneuver banner opens into the list of the trip's steps after the next one
/// ([ManeuverBanner], [StepList]). Open, it reaches down to the bottom inset, over the
/// buttons, the road name and trip progress, as iOS's does in both orientations (spec
/// §1.4 and §4), on every platform: the controls are laid out under the closed banner,
/// and stay there under the open one, rather than being pushed down as Android pushes
/// them. In landscape it stays in the start half. The trip carries on under it, the
/// camera following and the list keeping up with each step and each new route; it
/// stays open until closed, across new routes and turns of the screen.
///
/// ## Status and arrival
///
/// While something about the trip itself is wrong or pending, a status card takes the
/// banner's place, in its card, with no handle ([navigationStatusOf],
/// [NavigationStatusContent]): a location problem, with Open settings (the device's
/// location settings for location turned off, the app's for permission taken away) or
/// Try again ([NavigationController.resumeLocation]); finding a new route; a new route
/// that couldn't be found, while the controller keeps trying; off the route; and waiting
/// for GPS. The buttons stay laid out under it as under the banner. Once fixes stop
/// coming for [staleFixAge], a small "GPS signal lost" pill shows under the buttons
/// instead ([StaleFixIndicator]), as the trip carries on.
///
/// Once arrived, the banner's place says so, with the destination's name
/// ([ArrivalContent]); the camera stops following and eases to show the destination,
/// as the overview would, clear of the banner and trip progress
/// ([NavigationMap.overview]), and the camera button offers Recenter. The road name
/// goes, and trip progress keeps End, until you end the trip.
///
/// Content keeps at least 16 px from each edge, counting the system's bars, as iOS's
/// overlay does (spec §1.3).
class NavigationView extends StatefulWidget {
  const NavigationView({super.key});

  @override
  State<NavigationView> createState() => _NavigationViewState();
}

class _NavigationViewState extends State<NavigationView> {
  NavigationMap? _map;

  /// The sheet's controller, listened to for the sheet's moves (see [_viewChanged]).
  SheetController? _sheet;

  /// The trip's controller, listened to for its arrival (see [_tripChanged]).
  NavigationController? _controller;

  /// Whether the trip had arrived when [_tripChanged] last looked.
  bool _arrived = false;

  /// The trip changed: as it arrives, the camera stops following and shows the
  /// destination, once the frame has laid out the arrival in the banner's place.
  void _tripChanged() {
    final arrived = _controller!.state == NavigationState.arrived;
    if (arrived == _arrived) return;
    _arrived = arrived;
    if (!arrived) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _overview();
    });
  }

  /// The app's [unitSystem] setting, resolved for the device's region as each build
  /// shows a distance. The default stands until the setting is read.
  UnitSystem _units = UnitSystem.locale;
  StreamSubscription<UnitSystem>? _watchUnits;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final controller = NavigationScope.of(context).controller;
    if (controller != _controller) {
      _controller?.removeListener(_tripChanged);
      _controller = controller..addListener(_tripChanged);
      _arrived = controller.state == NavigationState.arrived;
    }
    _map ??= NavigationMap(map: MapScope.of(context), controller: controller)
      ..start();
    _watchUnits ??= StoreScope.of(context).settings
        .watch(unitSystem)
        .listen((units) {
          if (units != _units) setState(() => _units = units);
        });
    final sheet = SheetScaffold.controllerOf(context);
    if (sheet != _sheet) {
      _sheet?.removeListener(_viewChanged);
      _sheet = sheet..addListener(_viewChanged);
    }
    _follow();
    // The overview is fitted to how the screen's laid out, which the next frame shows
    // for whatever changed: the screen turned, say.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _viewChanged();
    });
  }

  /// The sheet moved, or the screen's layout changed: the camera keeps the chevron, or
  /// the overview keeps the rest of the trip, clear of what covers the map as it is
  /// now.
  ///
  /// The sheet is still on its way down out of sight as the screen opens, and the map's
  /// own padding follows it there; so the padding on top of it does too, rather than
  /// being read once mid-way and put the chevron where the sheet no longer is. In
  /// landscape, where the sheet is at the side, that would leave the chevron in the
  /// start half, under the banner.
  void _viewChanged() {
    if (!mounted) return;
    _follow();
    if (_map!.cameraMode.value == NavigationCameraMode.overview) _overview();
  }

  /// Where the follow camera puts the chevron (see [followPadding]): in landscape, its
  /// disk kept above the road-name pill under it. In portrait the pill is down on trip
  /// progress, far below the chevron.
  void _follow() {
    final layout = _Layout.of(context);
    _map!.padding = followPadding(
      size: layout.size,
      mapPadding: _mapPadding(),
      systemPadding: MediaQuery.paddingOf(context),
      direction: Directionality.of(context),
      under: layout.landscape
          ? layout.bottom + RoadNamePill.heightOf(context)
          : 0,
    );
  }

  /// What `CardinalMap` pads the map by, with the sheet down out of sight.
  EdgeInsets _mapPadding() =>
      EdgeInsets.only(top: MediaQuery.paddingOf(context).top) +
      _sheetCovers(context);

  /// Whether the banner's list of steps is open.
  final _stepsExpanded = ValueNotifier(false);

  /// How tall the banner is closed, which the buttons are laid out under in portrait,
  /// and the overview kept clear of; 0 without a banner. The overview is fitted again
  /// as it changes, as when a status card or the arrival takes the banner's place.
  late final _bannerHeight = ValueNotifier(0.0)..addListener(_viewChanged);

  /// Trip progress and the map buttons, measured for the overview to keep clear of.
  final _progressKey = GlobalKey();
  final _buttonsKey = GlobalKey();

  /// Shows the rest of the trip, clear of what the screen draws over the map.
  void _overview() => _map!.overview(
    overviewClear(covered: _covered(), mapPadding: _mapPadding()),
  );

  /// What the screen draws over the map, edge by edge, as it's laid out now: in
  /// portrait the top down to the closed banner's bottom and the bottom up to trip
  /// progress's top, the full width, and in landscape the start half, where both are.
  /// In either, the end side in to the far side of the map buttons, so the route
  /// isn't fitted under them: in portrait they hang under the banner, and in landscape
  /// they're at the top of the end half the route is fitted in. The road-name pill
  /// isn't counted: it's hidden in the overview.
  EdgeInsets _covered() {
    final layout = _Layout.of(context);
    final direction = Directionality.of(context);
    Rect? rect(GlobalKey key) {
      final box = key.currentContext?.findRenderObject() as RenderBox?;
      if (box == null || !box.hasSize || box.size.isEmpty) return null;
      return box.localToGlobal(Offset.zero) & box.size;
    }

    final buttons = rect(_buttonsKey);
    final end = buttons == null
        ? 0.0
        : direction == TextDirection.ltr
        ? layout.size.width - buttons.left
        : buttons.right;
    if (layout.landscape) {
      return EdgeInsetsDirectional.only(
        start: layout.start + layout.width / 2,
        end: end,
      ).resolve(direction);
    }
    final bannerBottom = layout.top + _bannerHeight.value;
    final progressTop =
        rect(_progressKey)?.top ?? layout.size.height - layout.bottom;
    return EdgeInsetsDirectional.only(
      top: bannerBottom,
      bottom: layout.size.height - progressTop,
      end: end,
    ).resolve(direction);
  }

  /// What the sheet covers of the map, out of sight as it is.
  static EdgeInsets _sheetCovers(BuildContext context) =>
      SheetScaffold.viewOf(context).obscured;

  @override
  void dispose() {
    _watchUnits?.cancel();
    _sheet?.removeListener(_viewChanged);
    _controller?.removeListener(_tripChanged);
    _map?.dispose();
    _stepsExpanded.dispose();
    _bannerHeight.dispose();
    super.dispose();
  }

  /// Between the banner and the controls under it, as in Ferrostar.
  static const _gap = 16.0;

  @override
  Widget build(BuildContext context) {
    final layout = _Layout.of(context);
    final _Layout(:start, :end, :top, :bottom, :landscape) = layout;
    // In landscape, the start half of what's between the insets ends where the end
    // half starts; else the start half is the full width.
    final startHalfEnd = landscape ? end + layout.width / 2 : end;
    final controller = NavigationScope.of(context).controller;
    final map = _map!;
    // The GPS-lost indicator hangs under the buttons, at the end side, outside their
    // key: the overview keeps the route clear of the buttons, not of it.
    final buttons = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.end,
      spacing: 8,
      children: [
        _MapButtons(
          key: _buttonsKey,
          map: map,
          controller: controller,
          onOverview: _overview,
        ),
        ListenableBuilder(
          listenable: controller,
          builder: (context, _) => StaleFixIndicator(
            lastFixAt: controller.lastFixAt,
            applies: staleFixApplies(controller),
          ),
        ),
      ],
    );
    final roadName = ListenableBuilder(
      listenable: Listenable.merge([controller, map.cameraMode]),
      builder: (context, _) => RoadNamePill(
        name: roadNameToShow(
          controller,
          following: map.cameraMode.value == NavigationCameraMode.following,
        ),
      ),
    );
    final progress = ListenableBuilder(
      key: _progressKey,
      listenable: controller,
      builder: (context, _) => TripProgressPill(
        progress: controller.update?.progress,
        arrived: controller.state == NavigationState.arrived,
        units: _units.onDevice,
        onEnd: () => endNavigation(context),
      ),
    );
    final banner = ListenableBuilder(
      listenable: controller,
      builder: (context, _) {
        final update = controller.update;
        return ManeuverBanner(
          status: _status(context, controller),
          instruction: update?.visualInstruction,
          distance: update?.progress?.distanceToNextManeuver,
          units: _units.onDevice,
          upcoming: update == null
              ? const []
              : upcomingSteps(controller.steps, update.stepIndex),
          expanded: _stepsExpanded,
          collapsedHeight: _bannerHeight,
        );
      },
    );
    return Stack(
      children: [
        if (landscape)
          PositionedDirectional(end: end, top: top, child: buttons)
        else
          // Under the closed banner, which covers them open.
          ValueListenableBuilder(
            valueListenable: _bannerHeight,
            builder: (context, height, child) => PositionedDirectional(
              end: end,
              top: top + (height > 0 ? height + _gap : 0),
              child: child!,
            ),
            child: buttons,
          ),
        if (landscape)
          // Under the chevron, in the end half, clear of the start half by the gap
          // between the halves.
          PositionedDirectional(
            start: start + layout.width / 2 + _gap,
            end: end,
            bottom: bottom,
            child: Center(child: roadName),
          ),
        PositionedDirectional(
          start: start,
          end: startHalfEnd,
          bottom: bottom,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (!landscape) ...[
                Center(child: roadName),
                const SizedBox(height: _roadNameGap),
              ],
              progress,
            ],
          ),
        ),
        // Over everything else, which it covers as it opens, down to the bottom inset.
        PositionedDirectional(
          start: start,
          end: startHalfEnd,
          top: top,
          bottom: bottom,
          child: Align(alignment: Alignment.topCenter, child: banner),
        ),
      ],
    );
  }

  /// Between the road-name pill and trip progress under it: iOS stacks them with
  /// SwiftUI's default spacing, 8.
  static const _roadNameGap = 8.0;

  /// What takes the banner's place, if anything: the arrival, or a status card.
  static Widget? _status(
    BuildContext context,
    NavigationController controller,
  ) {
    if (controller.state == NavigationState.arrived) {
      return ArrivalContent(
        destinationName: controller.destinationName,
        side: ArrivalContent.sideOf(controller.steps),
      );
    }
    final status = navigationStatusOf(controller);
    if (status == null) return null;
    final problem = controller.locationProblem;
    final location = DirectionsScope.of(context).location;
    return NavigationStatusContent(
      status: status,
      problem: problem,
      failure: controller.rerouteFailure,
      onAction: switch (problem) {
        null => null,
        LocationProblem.off => location.openLocationSettings,
        LocationProblem.denied => location.openAppSettings,
        LocationProblem.failed => controller.resumeLocation,
      },
    );
  }
}

/// Where the navigation screen's content goes: at least 16 px from each edge, counting
/// the system's bars, as iOS's overlay does (spec §1.3), and whether it's landscape,
/// wider than tall, as iOS tells it.
class _Layout {
  _Layout._({
    required this.size,
    required this.start,
    required this.end,
    required this.top,
    required this.bottom,
  });

  factory _Layout.of(BuildContext context) {
    final size = MediaQuery.sizeOf(context);
    final system = MediaQuery.paddingOf(context);
    final ltr = Directionality.of(context) == TextDirection.ltr;
    const least = 16.0;
    return _Layout._(
      size: size,
      start: math.max(least, ltr ? system.left : system.right),
      end: math.max(least, ltr ? system.right : system.left),
      top: math.max(least, system.top),
      bottom: math.max(least, system.bottom),
    );
  }

  final Size size;
  final double start;
  final double end;
  final double top;
  final double bottom;

  bool get landscape => size.width > size.height;

  /// What's between the insets, across.
  double get width => size.width - start - end;
}

/// The camera button and, 8 under it, the mute button, as both platforms stack them.
class _MapButtons extends StatelessWidget {
  const _MapButtons({
    super.key,
    required this.map,
    required this.controller,
    required this.onOverview,
  });

  final NavigationMap map;
  final NavigationController controller;
  final VoidCallback onOverview;

  /// The camera button's labels, and its tooltips, are Ferrostar's; the mute
  /// button's say what a tap does.
  @override
  Widget build(BuildContext context) {
    final words = AppLocalizations.of(context);
    return Column(
      mainAxisSize: MainAxisSize.min,
      spacing: 8,
      children: [
        ValueListenableBuilder(
          valueListenable: map.cameraMode,
          builder: (context, mode, _) => switch (mode) {
            // Nothing to show the trip from yet.
            null => const SizedBox.shrink(),
            NavigationCameraMode.following => _NavigationButton(
              tooltip: words.navigationRouteOverview,
              icon: AppSymbols.of(context).routeOverview,
              quarterTurns: AppSymbols.of(context).routeOverviewQuarterTurns,
              onPressed: onOverview,
            ),
            NavigationCameraMode.overview => _NavigationButton(
              tooltip: words.navigationRecenter,
              icon: AppSymbols.of(context).recenterFromOverview,
              onPressed: map.recenter,
            ),
            NavigationCameraMode.free => _NavigationButton(
              tooltip: words.navigationRecenter,
              icon: AppSymbols.of(context).recenter,
              onPressed: map.recenter,
            ),
          },
        ),
        ListenableBuilder(
          listenable: controller,
          builder: (context, _) {
            final muted = controller.muted;
            return _NavigationButton(
              tooltip: muted ? words.navigationUnmute : words.navigationMute,
              icon: muted
                  ? AppSymbols.of(context).muted
                  : AppSymbols.of(context).speaking,
              onPressed: () => controller.setMuted(!muted),
            );
          },
        ),
      ],
    );
  }
}

/// One of the navigation screen's map buttons: a floating button, as Ferrostar
/// Android's, or on iOS the map's glass control, as the app's other map buttons are.
class _NavigationButton extends StatelessWidget {
  const _NavigationButton({
    required this.tooltip,
    required this.icon,
    required this.onPressed,
    this.quarterTurns = 0,
  });

  final String tooltip;
  final IconData icon;
  final VoidCallback onPressed;
  final int quarterTurns;

  @override
  Widget build(BuildContext context) {
    final control = MapControl(
      tooltip: tooltip,
      icon: icon,
      quarterTurns: quarterTurns,
      onPressed: onPressed,
    );
    if (!isCupertinoOf(context)) return control;
    return AppGlassLayer(
      // As tinted as the map's other buttons: the map behind it can be anything.
      tintAmount: 0.8,
      child: GlassSurface(
        radius: MapControl.size / 2,
        floating: true,
        child: control,
      ),
    );
  }
}
