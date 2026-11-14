/// The navigation screen: the map, full-bleed, following you along the trip.
///
/// It's a page of the sheet ([NavigationPage]), over the directions it started from,
/// which hides the sheet; what it shows over the map ([NavigationView]) is in the
/// sheet scaffold's floating layer, through [NavigationLayer]. So the map is the app's
/// one map, in its style, and going back to directions finds them as they were.
///
/// The layout follows Ferrostar's in each orientation (see `NavigationView`); for now
/// it holds only End, where Ferrostar's trip progress goes.
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../map/map.dart';
import '../sheet/sheet.dart';
import '../ui/adaptive.dart';
import 'navigation_camera.dart';
import 'navigation_controller.dart';
import 'navigation_map.dart';
import 'navigation_scope.dart';

/// The navigation screen's page in the sheet: it hides the sheet and shows the
/// navigation screen over the map while it's open.
///
/// End, and back, end the trip and go back to the directions under it. Opened with no
/// trip under way, as when the app is restored to it after being closed, it goes back at
/// once.
class NavigationPage extends StatefulWidget {
  const NavigationPage({super.key});

  @override
  State<NavigationPage> createState() => _NavigationPageState();
}

class _NavigationPageState extends State<NavigationPage> {
  VoidCallback? _hide;
  bool _checked = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_checked) return;
    _checked = true;
    final scope = NavigationScope.of(context);
    final idle = scope.controller.state == NavigationState.idle;
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

  @override
  void dispose() {
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

/// Ends the trip and goes back from the navigation screen to the directions under it.
void endNavigation(BuildContext context) {
  final router = GoRouter.of(context);
  unawaited(NavigationScope.of(context).controller.end());
  router.canPop() ? router.pop() : router.go('/');
}

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
/// Laid out as Ferrostar lays out its navigation overlay (spec §1): in portrait, trip
/// progress along the bottom; in landscape (wider than tall, as iOS tells it), the
/// instructions and trip progress down the start half, the map's controls in the end
/// half. Content keeps at least 16 px from each edge, counting the system's bars, as
/// iOS's overlay does (spec §1.3). For now only End is there, at the end of where trip
/// progress goes, where Ferrostar's trip progress has its own exit button.
class NavigationView extends StatefulWidget {
  const NavigationView({super.key});

  @override
  State<NavigationView> createState() => _NavigationViewState();
}

class _NavigationViewState extends State<NavigationView> {
  NavigationMap? _map;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _map ??= NavigationMap(
      map: MapScope.of(context),
      controller: NavigationScope.of(context).controller,
    )..start();
    final size = MediaQuery.sizeOf(context);
    final system = MediaQuery.paddingOf(context);
    _map!.padding = followPadding(
      size: size,
      // What `CardinalMap` pads the map by, with the sheet down out of sight.
      mapPadding: EdgeInsets.only(top: system.top) + _sheetCovers(context),
      systemPadding: system,
      direction: Directionality.of(context),
    );
  }

  /// What the sheet covers of the map, out of sight as it is.
  static EdgeInsets _sheetCovers(BuildContext context) =>
      SheetScaffold.viewOf(context).obscured;

  @override
  void dispose() {
    _map?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final size = MediaQuery.sizeOf(context);
    final system = MediaQuery.paddingOf(context);
    final ltr = Directionality.of(context) == TextDirection.ltr;
    const least = 16.0;
    final start = math.max(least, ltr ? system.left : system.right);
    final end = math.max(least, ltr ? system.right : system.left);
    final bottom = math.max(least, system.bottom);
    // In landscape, the start half of what's between the insets, as Android splits it;
    // else the full width.
    final width = size.width - start - end;
    final landscape = size.width > size.height;
    return Stack(
      children: [
        PositionedDirectional(
          start: start,
          end: landscape ? end + width / 2 : end,
          bottom: bottom,
          child: Align(
            alignment: AlignmentDirectional.centerEnd,
            child: AppButton(
              label: 'End',
              onPressed: () => endNavigation(context),
            ),
          ),
        ),
      ],
    );
  }
}
