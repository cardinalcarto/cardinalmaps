/// The buttons along the bottom of the directions sheet's pages, and what Start does.
library;

import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoButton;
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

import '../geocoder/address.dart';
import '../navigation/navigation_scope.dart';
import '../routes.dart';
import '../ui/adaptive.dart';
import 'directions.dart';

/// Starts navigating the route selected in [directions] to the place with [to], and
/// opens the navigation screen over the page of [context]. Does nothing with no route
/// selected.
void startNavigation(
  BuildContext context,
  Directions directions, {
  required String to,
}) {
  final route = directions.selectedRoute;
  if (route == null) return;
  NavigationScope.of(context).controller.start(
    route: route,
    mode: directions.mode,
    wheelchair: directions.wheelchair,
    destinationName: switch (directions.to) {
      RouteEndpoint(:final place?) => placeTitle(place),
      _ => null,
    },
  );
  unawaited(context.push(navigationLocation(to)));
}

/// Steps and Start along the bottom of the window, acting on the selected route:
/// Start the main action, twice as wide. Null disables either. [TripButtons.start] has
/// Start alone, the width of the window, and [TripButtons.details] has Trip details
/// alone, which acts on the selected transit journey.
///
/// Whichever they are, they're one height at a text size, so one set can take
/// another's place without the sheet's anchors moving.
///
/// They're as tall as their labels need at the system's text size, and keep clear of
/// the navigation bar. Nothing is painted behind them: the sheet clips its content out
/// from under them.
class TripButtons extends StatelessWidget {
  const TripButtons({super.key, required this.onSteps, required this.onStart})
    : _steps = true,
      _main = 'Start';

  const TripButtons.start({super.key, required this.onStart})
    : onSteps = null,
      _steps = false,
      _main = 'Start';

  const TripButtons.details({super.key, required VoidCallback? onDetails})
    : onSteps = null,
      onStart = onDetails,
      _steps = false,
      _main = 'Trip details';

  final VoidCallback? onSteps;

  /// What the main button does: Start, or Trip details.
  final VoidCallback? onStart;

  /// What the main button says.
  final String _main;

  /// Whether there's a Steps button.
  final bool _steps;

  @override
  Widget build(BuildContext context) {
    final cupertino = isCupertinoOf(context);
    // One line, shrunk to fit its button rather than cut off, in the room of a line
    // at its full size: a button is as tall whatever it says.
    Widget label(String text) => Stack(
      children: [
        const ExcludeSemantics(
          child: SizedBox(width: double.infinity, child: Text('', maxLines: 1)),
        ),
        Positioned.fill(
          child: FittedBox(
            fit: BoxFit.scaleDown,
            child: Text(text, maxLines: 1, softWrap: false),
          ),
        ),
      ],
    );
    final Widget steps, start;
    if (cupertino) {
      // Capsules, the lesser one a tint, as the sheet is glass while it's low.
      const size = Size(0, 50);
      final radius = BorderRadius.circular(100);
      steps = CupertinoButton.tinted(
        minimumSize: size,
        borderRadius: radius,
        onPressed: onSteps,
        child: label('Steps'),
      );
      start = CupertinoButton.filled(
        minimumSize: size,
        borderRadius: radius,
        onPressed: onStart,
        child: label(_main),
      );
    } else {
      final style = FilledButton.styleFrom(minimumSize: const Size(0, 48));
      steps = FilledButton.tonal(
        style: style,
        onPressed: onSteps,
        child: label('Steps'),
      );
      start = FilledButton(
        style: style,
        onPressed: onStart,
        child: label(_main),
      );
    }
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (!cupertino) const Divider(height: 1),
        SafeArea(
          top: false,
          minimum: EdgeInsets.only(bottom: cupertino ? 8 : 10),
          child: Padding(
            padding: EdgeInsets.fromLTRB(
              cupertino ? 16 : 12,
              10,
              cupertino ? 16 : 12,
              0,
            ),
            child: Row(
              spacing: 10,
              children: [
                if (_steps) Expanded(child: steps),
                Expanded(flex: 2, child: start),
              ],
            ),
          ),
        ),
      ],
    );
  }
}
