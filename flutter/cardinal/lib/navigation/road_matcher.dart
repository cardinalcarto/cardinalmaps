import 'package:flutter/foundation.dart';
import 'package:routing/routing.dart';

import '../directions/route_source.dart';

/// How far back the fixes matched with each new one go.
///
/// A fix alone can't tell two roads 15 m apart; the fixes before it can, since only one
/// of the roads goes where they went. 30 s is 400 m at city driving speed and 40 m on
/// foot: enough to tell a parallel street or a frontage road from the route, and short
/// enough that a turn a block back doesn't still pull the match. Matching 60 s took
/// about one and a half to two and a half times as long over Seattle's tiles
/// (`match_latency` in the routing library).
const matchWindow = Duration(seconds: 30);

/// The most fixes matched together: a second's apart over [matchWindow], as phones give
/// them while navigating, so a location service that gives them faster doesn't make each
/// match take longer.
const maxMatchFixes = 30;

/// How long fixes may stop before the window starts over.
///
/// After a tunnel, or with the phone's location off for a while, the fixes before the gap
/// say nothing about the road you're on now, and the matcher would route between the two
/// sides of the gap to join them.
const matchGap = Duration(seconds: 10);

/// Matches recent fixes to the roads, so tests can fake it.
abstract class RoadMatcher {
  /// Where on the roads the last of [fixes] (oldest first) was, matched together with
  /// those before it, for a trip traveling by [mode], by wheelchair if [mode] is walking
  /// and [wheelchair] is set; null if it couldn't be matched. Never throws.
  Future<MatchedPosition?> match(
    NavigationMode mode,
    List<NavLocation> fixes, {
    required bool wheelchair,
  });
}

/// [RoadMatcher] over the offline area's streets, with the area's router.
class OfflineRoadMatcher implements RoadMatcher {
  OfflineRoadMatcher(this._area);

  final OfflineAreaSource _area;

  @override
  Future<MatchedPosition?> match(
    NavigationMode mode,
    List<NavLocation> fixes, {
    required bool wheelchair,
  }) async {
    try {
      final area = await _area();
      if (area == null) return null;
      return await area.useRouter(
        (router) => router.matchPosition(
          mode: mode,
          wheelchair: wheelchair,
          fixes: fixes,
        ),
      );
    } on Object catch (e) {
      debugPrint('navigation: match: $e');
      return null;
    }
  }
}

/// The recent fixes, which each new one is matched with: those within [matchWindow] of
/// the newest, at most [maxMatchFixes], starting over after a [matchGap].
class MatchWindow {
  final _fixes = <NavLocation>[];

  /// The fixes in the window, oldest first.
  List<NavLocation> get fixes => List.unmodifiable(_fixes);

  void add(NavLocation fix) {
    final at = fix.timestampMs.toInt();
    if (_fixes.isNotEmpty &&
        at - _fixes.last.timestampMs.toInt() > matchGap.inMilliseconds) {
      _fixes.clear();
    }
    _fixes.add(fix);
    _fixes.removeWhere(
      (old) => at - old.timestampMs.toInt() > matchWindow.inMilliseconds,
    );
    if (_fixes.length > maxMatchFixes) {
      _fixes.removeRange(0, _fixes.length - maxMatchFixes);
    }
  }

  void clear() => _fixes.clear();
}
