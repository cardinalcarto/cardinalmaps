/// What the car's screen is told of a trip: one message a fix, read by the app's
/// CarPlay code (`CarPlaySceneDelegate.swift`), which shows the maneuver, the distances
/// and the status in CarPlay's own templates, and by the car's map (`car_map.dart`),
/// which draws where you are.
library;

import 'dart:typed_data';

import 'package:routing/routing.dart';

import '../l10n/app_localizations.dart';
import '../navigation/navigation_controller.dart';
import '../navigation/navigation_format.dart';
import '../navigation/navigation_notification.dart';
import '../navigation/navigation_status.dart';
import '../units.dart';
import 'car_keys.dart';

/// What the car is told of [controller]'s trip now, in [words], with distances in
/// [units] (metric or imperial); null when there's no trip. [route] is the number the
/// trip's line was sent under ([carRouteOf]).
///
/// It says what the phone's navigation screen does, and words it as the notification
/// does ([navigationNoticeOf]):
///
/// - on the route, the next maneuver: its instruction, its icon in the app's icon
///   font, and how far and how long until it;
/// - otherwise a status: the title of the phone's status card, such as "Finding a new
///   route…", "You've arrived" once there (with the arrival's icon), or "Navigating"
///   before the route has a maneuver to show.
///
/// With either, while the session follows the route: how far and how long is left of
/// the trip, and where the map's chevron is, as the phone's map puts it (on the
/// route's line, or off the route where navigation put you), with where the traveled
/// part of the line ends.
///
/// Distances are rounded as the phone's are ([roundManeuverDistance]) and sent as a
/// number and a unit, since CarPlay words them itself.
Map<String, Object?>? carTripOf(
  NavigationController controller, {
  required AppLocalizations words,
  required UnitSystem units,
  required int route,
}) {
  final state = controller.state;
  final notice = navigationNoticeOf(controller, words: words, units: units);
  if (state == NavigationState.idle || notice == null) return null;
  final update = controller.update;
  final status = navigationStatusOf(controller);
  final guiding = state == NavigationState.onRoute && status == null;
  final instruction = guiding ? update?.visualInstruction : null;
  final progress = update?.progress;
  final end = controller.route?.geometry.lastOrNull;
  return {
    CarKeys.state: state.name,
    CarKeys.mode: controller.mode?.name,
    CarKeys.route: route,
    CarKeys.muted: controller.muted,
    CarKeys.end: words.navigationEnd,
    CarKeys.destination: controller.destinationName,
    if (end != null) CarKeys.to: {CarKeys.lat: end.lat, CarKeys.lng: end.lng},
    if (instruction == null) ...{
      CarKeys.status: notice.title,
    } else ...{
      CarKeys.instruction: instruction.primary.text,
      CarKeys.step: update!.stepIndex,
    },
    CarKeys.glyph: notice.glyph?.codePoint,
    if (progress != null) ...{
      if (instruction != null) ...{
        CarKeys.toManeuver: _distance(progress.distanceToNextManeuver, units),
        CarKeys.toManeuverSeconds: progress.durationToNextManeuver,
      },
      CarKeys.remaining: _distance(progress.distanceRemaining, units),
      CarKeys.remainingSeconds: progress.durationRemaining,
    },
    if (update != null) ..._position(state, update),
  };
}

/// [meters] as CarPlay is given it: rounded as the phone shows it, in that unit.
Map<String, Object?> _distance(double meters, UnitSystem units) {
  final rounded = roundManeuverDistance(meters, units: units);
  return {
    CarKeys.value: rounded.tenths
        ? (rounded.value * 10).roundToDouble() / 10
        : rounded.value.roundToDouble(),
    CarKeys.unit: rounded.unit.name,
  };
}

/// Where the chevron is and where the traveled line ends, as `NavigationMap` has them
/// on the phone; nothing before the session has put you on the route's line.
Map<String, Object?> _position(NavigationState state, NavUpdate update) {
  final snapped = update.snappedLocation;
  if (snapped == null) return const {};
  final at = switch (state) {
    NavigationState.offRoute ||
    NavigationState.rerouting => update.effectiveLocation ?? snapped,
    _ => snapped,
  };
  return {
    CarKeys.at: {
      CarKeys.lat: at.lat,
      CarKeys.lng: at.lng,
      CarKeys.bearing: at.course,
      CarKeys.speed: update.location?.speed ?? at.speed,
    },
    CarKeys.snapped: {CarKeys.lat: snapped.lat, CarKeys.lng: snapped.lng},
    CarKeys.index: update.geometryIndex,
  };
}

/// [line], a route's, as the car is sent it under the number [id], which each
/// [carTripOf] of a trip along it carries: its points' longitudes and latitudes in
/// turn, as one list of numbers, since a long route has tens of thousands.
Map<String, Object?> carRouteOf(int id, List<DirectionsPoint> line) {
  final numbers = Float64List(line.length * 2);
  for (var i = 0; i < line.length; i++) {
    numbers[2 * i] = line[i].lng;
    numbers[2 * i + 1] = line[i].lat;
  }
  return {CarKeys.id: id, CarKeys.line: numbers};
}

/// The line [carRouteOf] sent.
List<DirectionsPoint> carRouteLine(Object? numbers) {
  final list = numbers is List ? numbers : const <Object?>[];
  return [
    for (var i = 0; i + 1 < list.length; i += 2)
      DirectionsPoint(
        lat: (list[i + 1]! as num).toDouble(),
        lng: (list[i]! as num).toDouble(),
      ),
  ];
}
