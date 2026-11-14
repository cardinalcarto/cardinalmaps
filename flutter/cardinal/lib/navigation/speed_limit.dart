/// The speed-limit sign on the navigation screen.
library;

import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';
import 'package:routing/routing.dart';

import '../directions/route_source.dart';
import '../l10n/app_localizations.dart';
import 'navigation_controller.dart';
import 'navigation_status.dart';

/// Kilometers in a mile.
const _kmPerMile = 1.609344;

/// What the speed-limit sign shows: a limit on a [sign] of the kind posted where you
/// are, in its [unit], or, with no [speed], the end of all limits.
typedef SpeedLimitToShow = ({
  SpeedLimitSign sign,
  SpeedLimitUnit unit,
  int? speed,
});

/// What the speed-limit sign shows for [limit], on a step whose limits are posted on
/// [sign] signs in [unit]; null to show none.
///
/// - A limit shows in the step's unit, which is the country's, whatever unit the app
///   shows distances in: a limit in mph is km/h ÷ 1.609344, rounded, which gives back
///   the posted mph that Valhalla stored in km/h.
/// - No limit shows on a Vienna sign as the end-of-restrictions sign, as on a German
///   autobahn; on a MUTCD sign, as nothing, since the US posts no such sign.
/// - An unknown limit shows nothing.
///
/// Where the router doesn't say how the step's limits are signed, they're on Vienna
/// signs, as in most of the world; where it doesn't say their unit, it's the sign's:
/// mph for MUTCD, km/h for Vienna.
SpeedLimitToShow? speedLimitToShow(
  NavSpeedLimit limit, {
  SpeedLimitSign? sign,
  SpeedLimitUnit? unit,
}) {
  final shownSign = sign ?? SpeedLimitSign.vienna;
  final shownUnit =
      unit ??
      (shownSign == SpeedLimitSign.mutcd
          ? SpeedLimitUnit.milesPerHour
          : SpeedLimitUnit.kilometersPerHour);
  switch (limit.kind) {
    case SpeedLimitKind.unknown:
      return null;
    case SpeedLimitKind.unlimited:
      if (shownSign != SpeedLimitSign.vienna) return null;
      return (sign: shownSign, unit: shownUnit, speed: null);
    case SpeedLimitKind.limited:
      final kph = limit.kph;
      if (kph == null || kph <= 0) return null;
      final speed = shownUnit == SpeedLimitUnit.milesPerHour
          ? (kph / _kmPerMile).round()
          : kph.round();
      return (sign: shownSign, unit: shownUnit, speed: speed);
  }
}

/// The speed-limit sign [controller]'s trip calls for, or null for none.
///
/// It shows only while driving: on foot a limit means nothing, and by bike, where few
/// ride near one, it would be clutter, and a bike path's limit is the road's beside it
/// as often as not. And it shows only while following the route, with no status card in
/// the banner's place: not while waiting for GPS, off the route, finding a new route or
/// arrived, when where you are on the route, and so its limit, isn't known. A fix off
/// the current step but on the route ([NavDeviation.offStep]) still shows it, as it's
/// often only a fix's worth at a maneuver.
SpeedLimitToShow? speedLimitOf(NavigationController controller) {
  if (controller.mode != TravelMode.driving) return null;
  if (controller.state != NavigationState.onRoute) return null;
  if (navigationStatusOf(controller) != null) return null;
  final update = controller.update;
  if (update == null || update.deviation == NavDeviation.offRoute) return null;
  final steps = controller.steps;
  final index = update.stepIndex;
  final step = index >= 0 && index < steps.length ? steps[index] : null;
  return speedLimitToShow(
    update.speedLimit,
    sign: step?.speedLimitSign,
    unit: step?.speedLimitUnit,
  );
}

/// The speed-limit sign at the top start of the navigation screen, as Ferrostar draws
/// it (spec §2.6), in the style the country posts its limits in:
///
/// - MUTCD: a white rectangle with "SPEED LIMIT" over the number, black-edged inside a
///   white rim, Ferrostar's 60 × 84 with radii 8, 6 and 4. Where the country posts
///   them in km/h on MUTCD signs, as Canada does, "km/h" goes under the number.
/// - Vienna: a white disk in a red ring, 64 across with a 56 disk, the number alone.
///   With no limit, the end-of-restrictions sign: a white disk, gray-edged, crossed by
///   black stripes.
///
/// The number's size goes down with its digits, as Ferrostar's: 30, then 24 for three
/// digits, then 18, on MUTCD; 29, 24 and 18 on Vienna. Signs are white whatever the
/// theme, as real ones are, with a shadow to stand off the map.
class SpeedLimitSignView extends StatelessWidget {
  const SpeedLimitSignView({super.key, required this.limit});

  final SpeedLimitToShow limit;

  static const _ink = Color(0xFF000000);
  static const _white = Color(0xFFFFFFFF);
  static const _red = Color(0xFFD32F2F);
  static const _shadow = [BoxShadow(color: Color(0x40000000), blurRadius: 6)];

  @override
  Widget build(BuildContext context) {
    final words = AppLocalizations.of(context);
    final speed = limit.speed;
    final label = speed == null
        ? words.navigationNoSpeedLimit
        : words.navigationSpeedLimit(
            limit.unit == SpeedLimitUnit.milesPerHour
                ? words.navigationSpeedMph('$speed')
                : words.navigationSpeedKph('$speed'),
          );
    return Semantics(
      label: label,
      excludeSemantics: true,
      child: MediaQuery.withNoTextScaling(
        child: switch (limit.sign) {
          SpeedLimitSign.mutcd => _mutcd(context, speed),
          SpeedLimitSign.vienna =>
            speed == null ? _derestriction() : _vienna(speed),
        },
      ),
    );
  }

  static double _numberSize(int speed, List<double> sizes) {
    final digits = '$speed'.length;
    return digits < 3
        ? sizes[0]
        : digits == 3
        ? sizes[1]
        : sizes[2];
  }

  Widget _mutcd(BuildContext context, int? speed) {
    final words = AppLocalizations.of(context);
    BoxDecoration box(Color color, double radius) => BoxDecoration(
      color: color,
      borderRadius: BorderRadius.all(Radius.circular(radius)),
    );
    return Container(
      width: 60,
      height: 84,
      padding: const EdgeInsets.all(2),
      decoration: box(_white, 8).copyWith(boxShadow: _shadow),
      child: DecoratedBox(
        decoration: box(_ink, 6),
        child: Padding(
          padding: const EdgeInsets.all(2),
          child: DecoratedBox(
            decoration: box(_white, 4),
            child: Padding(
              padding: const EdgeInsets.fromLTRB(3, 5, 3, 4),
              child: Column(
                children: [
                  SizedBox(
                    height: 24,
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      child: Text(
                        // Each word on its own line, as on the sign.
                        words.navigationSpeedLimitSign.replaceFirst(' ', '\n'),
                        textAlign: TextAlign.center,
                        style: const TextStyle(
                          color: _ink,
                          fontSize: 10,
                          height: 1.1,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ),
                  ),
                  Expanded(
                    child: Center(
                      child: FittedBox(
                        fit: BoxFit.scaleDown,
                        child: Text(
                          '$speed',
                          style: TextStyle(
                            color: _ink,
                            fontSize: _numberSize(speed ?? 0, const [
                              30,
                              24,
                              18,
                            ]),
                            fontWeight: FontWeight.w800,
                            height: 1,
                          ),
                        ),
                      ),
                    ),
                  ),
                  if (limit.unit == SpeedLimitUnit.kilometersPerHour)
                    Text(
                      words.navigationKph,
                      style: const TextStyle(
                        color: Color(0xFF555555),
                        fontSize: 9,
                        height: 1,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _vienna(int speed) => Container(
    width: 64,
    height: 64,
    padding: const EdgeInsets.all(4),
    decoration: const BoxDecoration(
      color: _red,
      shape: BoxShape.circle,
      boxShadow: _shadow,
    ),
    child: DecoratedBox(
      decoration: const BoxDecoration(color: _white, shape: BoxShape.circle),
      child: Center(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 4),
          child: FittedBox(
            fit: BoxFit.scaleDown,
            child: Text(
              '$speed',
              style: TextStyle(
                color: _ink,
                fontSize: _numberSize(speed, const [29, 24, 18]),
                fontWeight: FontWeight.w800,
                height: 1,
              ),
            ),
          ),
        ),
      ),
    ),
  );

  Widget _derestriction() => const SizedBox(
    width: 64,
    height: 64,
    child: DecoratedBox(
      decoration: BoxDecoration(shape: BoxShape.circle, boxShadow: _shadow),
      child: CustomPaint(painter: _DerestrictionPainter()),
    ),
  );
}

/// The end-of-restrictions sign: a white disk with a thin gray edge, crossed from top
/// right to bottom left by five thin black stripes.
class _DerestrictionPainter extends CustomPainter {
  const _DerestrictionPainter();

  @override
  void paint(Canvas canvas, Size size) {
    final center = size.center(Offset.zero);
    final radius = size.shortestSide / 2;
    canvas.drawCircle(center, radius, Paint()..color = const Color(0xFFFFFFFF));
    canvas.drawCircle(
      center,
      radius - 1,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..color = const Color(0xFF9E9E9E),
    );
    canvas.save();
    canvas.clipPath(
      Path()..addOval(Rect.fromCircle(center: center, radius: radius - 6)),
    );
    final stripe = Paint()
      ..color = const Color(0xFF000000)
      ..strokeWidth = 2.5;
    // Perpendicular to the stripes, from the top left to the bottom right.
    const along = math.sqrt1_2;
    for (var i = -2; i <= 2; i++) {
      final offset = Offset(along, along) * (i * 5.0);
      final from = center + offset + const Offset(along, -along) * radius;
      final to = center + offset - const Offset(along, -along) * radius;
      canvas.drawLine(from, to, stripe);
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(_DerestrictionPainter old) => false;
}
