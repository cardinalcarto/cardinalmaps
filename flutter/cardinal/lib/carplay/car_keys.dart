/// The keys of the messages the car's screen is sent (see `car_trip.dart`), which
/// `CarPlaySceneDelegate.swift` reads by the same names.
abstract final class CarKeys {
  static const state = 'state';
  static const mode = 'mode';
  static const route = 'route';
  static const muted = 'muted';
  static const end = 'end';
  static const destination = 'destination';
  static const to = 'to';
  static const status = 'status';
  static const instruction = 'instruction';
  static const step = 'step';
  static const glyph = 'glyph';
  static const toManeuver = 'toManeuver';
  static const toManeuverSeconds = 'toManeuverSeconds';
  static const remaining = 'remaining';
  static const remainingSeconds = 'remainingSeconds';
  static const value = 'value';
  static const unit = 'unit';
  static const at = 'at';
  static const snapped = 'snapped';
  static const index = 'index';
  static const lat = 'lat';
  static const lng = 'lng';
  static const bearing = 'bearing';
  static const speed = 'speed';
  static const id = 'id';
  static const line = 'line';

  /// Of the map's own message: where its tiles come from, where it opens, and, added
  /// by the app's CarPlay code, the folders its files go in.
  static const tileJson = 'tileJson';
  static const basemap = 'basemap';
  static const view = 'view';
  static const support = 'support';
  static const cache = 'cache';
}
