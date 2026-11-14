import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoIcons;
import 'package:material_ui/material_ui.dart';

import 'adaptive.dart';

/// The icons for common actions: SF Symbols' look (`CupertinoIcons`) on iOS, Material's
/// elsewhere. The app's own icons, its navigation bar's, are `AppIcons`.
class AppSymbols {
  const AppSymbols._(this._ios);

  static const _cupertino = AppSymbols._(true);
  static const _material = AppSymbols._(false);

  static AppSymbols of(BuildContext context) =>
      isCupertinoOf(context) ? _cupertino : _material;

  final bool _ios;

  IconData _pick(IconData cupertino, IconData material) =>
      _ios ? cupertino : material;

  IconData get close => _pick(CupertinoIcons.xmark, Icons.close);
  IconData get clear => _pick(CupertinoIcons.xmark_circle_fill, Icons.cancel);
  IconData get back => _pick(CupertinoIcons.chevron_back, Icons.arrow_back);
  IconData get forward =>
      _pick(CupertinoIcons.chevron_forward, Icons.chevron_right);
  IconData get add => _pick(CupertinoIcons.add, Icons.add);
  IconData get share => _pick(CupertinoIcons.share, Icons.share);
  IconData get more => _pick(CupertinoIcons.ellipsis, Icons.more_vert);
  IconData get swap =>
      _pick(CupertinoIcons.arrow_up_arrow_down, Icons.swap_vert);
  IconData get refresh => _pick(CupertinoIcons.arrow_clockwise, Icons.refresh);
  IconData get save => _pick(CupertinoIcons.heart, Icons.favorite_border);
  IconData get saved => _pick(CupertinoIcons.heart_fill, Icons.favorite);
  IconData get delete => _pick(CupertinoIcons.trash, Icons.delete_outline);
  IconData get edit => _pick(CupertinoIcons.pencil, Icons.edit_outlined);
  IconData get check => _pick(CupertinoIcons.checkmark, Icons.check);
  IconData get search => _pick(CupertinoIcons.search, Icons.search);
  IconData get myLocation => _pick(CupertinoIcons.location, Icons.my_location);

  /// Back to following you while navigating: iOS's Current Location after a pan,
  /// Android's Recenter (Ferrostar's `location.fill` and `navigation_24px`).
  IconData get recenter =>
      _pick(CupertinoIcons.location_fill, Icons.navigation);

  /// Back to following you from the route overview: iOS's Recenter there
  /// (`location.north.fill`); Android has the one Recenter.
  IconData get recenterFromOverview =>
      _pick(CupertinoIcons.location_north_fill, Icons.navigation);

  /// The rest of the trip, while navigating: Ferrostar Android's `route_24px`, which
  /// it turns a quarter (see [routeOverviewQuarterTurns]). iOS's is an S-shaped path
  /// between two points, which Cupertino's icons have nothing like; a map stands in.
  IconData get routeOverview => _pick(CupertinoIcons.map, Icons.route);

  /// How many quarter turns clockwise [routeOverview] is drawn at: Material's route
  /// icon runs side to side, which Ferrostar turns to run up the screen.
  int get routeOverviewQuarterTurns => _ios ? 0 : 1;

  /// Spoken announcements on, and off: Ferrostar's `speaker.2.fill` and
  /// `speaker.slash.fill` on iOS, and Material's volume icons.
  IconData get speaking =>
      _pick(CupertinoIcons.speaker_2_fill, Icons.volume_up);
  IconData get muted =>
      _pick(CupertinoIcons.speaker_slash_fill, Icons.volume_off);

  /// Folds the navigation banner's list of steps back up: Ferrostar's `chevron.up` on
  /// iOS and `keyboard_arrow_up_24px` on Android.
  IconData get collapse =>
      _pick(CupertinoIcons.chevron_up, Icons.keyboard_arrow_up);

  /// The navigation screen's status cards (see `NavigationStatus`): location turned
  /// off or not allowed, location failing, waiting for a first fix, a fix gone stale,
  /// off the route, and a new route that couldn't be found.
  IconData get locationOff =>
      _pick(CupertinoIcons.location_slash, Icons.location_off);
  IconData get locationFailed =>
      _pick(CupertinoIcons.location_slash_fill, Icons.location_disabled);
  IconData get waitingForGps =>
      _pick(CupertinoIcons.location, Icons.location_searching);
  IconData get gpsLost => _pick(CupertinoIcons.location_slash, Icons.gps_off);
  IconData get offRoute => _pick(CupertinoIcons.arrow_branch, Icons.alt_route);
  IconData get warning =>
      _pick(CupertinoIcons.exclamationmark_triangle, Icons.warning_amber);
  IconData get layers =>
      _pick(CupertinoIcons.square_stack_3d_up, Icons.layers_outlined);
}
