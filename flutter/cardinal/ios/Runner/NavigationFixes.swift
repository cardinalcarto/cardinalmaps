import CoreLocation
import Flutter

/// Navigation's location updates, on `cardinal/navigation/fixes`, from a Core Location
/// manager of navigation's own: as precise as the device gives them, with no distance
/// filter, tuned for the trip's mode (the `mode` listen argument: `driving`, `cycling`
/// or `walking`), and not paused at a long stop.
///
/// They're navigation's own, rather than geolocator's: geolocator has one position
/// stream per app, which the map's location dot already holds with its own, looser
/// settings, and hands to any later caller whatever settings it asks for. Those
/// settings don't allow background updates, so fixes would stop with the screen off.
///
/// Fixes keep coming with the screen off: navigation starts in the foreground, and
/// background updates are allowed (with the `location` background mode), so "while
/// using the app" is enough; the status bar shows that the app is using your location.
///
/// Errors, by code, as `NavigationFixes.kt` sends them: `off`, location services are off
/// (the stream stays open, and fixes come again once they're on); `denied`, the app isn't
/// allowed your location (the stream ends); `failed`, anything else (the stream ends).
final class NavigationFixes: NSObject, FlutterStreamHandler, CLLocationManagerDelegate {
  private let manager = CLLocationManager()
  private var sink: FlutterEventSink?

  override init() {
    super.init()
    manager.delegate = self
  }

  func onListen(withArguments arguments: Any?, eventSink events: @escaping FlutterEventSink)
    -> FlutterError?
  {
    _ = onCancel(withArguments: nil)
    let mode = (arguments as? [String: Any])?["mode"] as? String
    switch manager.authorizationStatus {
    case .denied, .restricted:
      // With location services off, the app's own permission reads as denied too.
      if !CLLocationManager.locationServicesEnabled() { break }
      events(FlutterError(code: "denied", message: "No location permission", details: nil))
      events(FlutterEndOfEventStream)
      return nil
    default:
      break
    }
    manager.desiredAccuracy = kCLLocationAccuracyBestForNavigation
    manager.distanceFilter = kCLDistanceFilterNone
    manager.activityType = Self.activity(mode)
    // Core Location would otherwise stop updates at a long red light, and only start
    // them again when the app comes to the foreground.
    manager.pausesLocationUpdatesAutomatically = false
    manager.allowsBackgroundLocationUpdates = true
    manager.showsBackgroundLocationIndicator = true
    sink = events
    manager.startUpdatingLocation()
    if !CLLocationManager.locationServicesEnabled() {
      events(FlutterError(code: "off", message: "Location is off", details: nil))
    }
    return nil
  }

  func onCancel(withArguments arguments: Any?) -> FlutterError? {
    guard sink != nil else { return nil }
    manager.stopUpdatingLocation()
    manager.allowsBackgroundLocationUpdates = false
    sink = nil
    return nil
  }

  /// What Core Location is told the fixes are for, which tunes its filtering: a car stays
  /// on roads, while a bicycle and a walker don't.
  private static func activity(_ mode: String?) -> CLActivityType {
    switch mode {
    case "driving": return .automotiveNavigation
    // Apple's category for navigating by any vehicle but a car.
    case "cycling": return .otherNavigation
    case "walking": return .fitness
    default: return .otherNavigation
    }
  }

  func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
    guard let sink else { return }
    for location in locations { sink(Self.toMap(location)) }
  }

  func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
    guard let sink else { return }
    switch (error as? CLError)?.code {
    case .locationUnknown:
      // Core Location keeps trying: there's just no fix yet.
      return
    case .denied:
      if !CLLocationManager.locationServicesEnabled() {
        sink(FlutterError(code: "off", message: "Location is off", details: nil))
        return
      }
      fail("denied", error)
    default:
      fail("failed", error)
    }
  }

  func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
    guard let sink else { return }
    if !CLLocationManager.locationServicesEnabled() {
      sink(FlutterError(code: "off", message: "Location is off", details: nil))
    } else if manager.authorizationStatus == .denied
      || manager.authorizationStatus == .restricted
    {
      fail("denied", nil)
    }
  }

  private func fail(_ code: String, _ error: Error?) {
    guard let sink else { return }
    NSLog("CardinalNavigation: location \(code): \(String(describing: error))")
    sink(FlutterError(code: code, message: error?.localizedDescription, details: nil))
    sink(FlutterEndOfEventStream)
    _ = onCancel(withArguments: nil)
  }

  /// What Dart's `navLocationFromMap` reads; what wasn't measured is left out, which
  /// Core Location marks with a negative value.
  private static func toMap(_ location: CLLocation) -> [String: Any] {
    var fix: [String: Any] = [
      "latitude": location.coordinate.latitude,
      "longitude": location.coordinate.longitude,
      "timestamp": Int64((location.timestamp.timeIntervalSince1970 * 1000).rounded()),
    ]
    if location.horizontalAccuracy >= 0 { fix["accuracy"] = location.horizontalAccuracy }
    if location.course >= 0 { fix["heading"] = location.course }
    if location.courseAccuracy >= 0 { fix["heading_accuracy"] = location.courseAccuracy }
    if location.speed >= 0 { fix["speed"] = location.speed }
    if location.speedAccuracy >= 0 { fix["speed_accuracy"] = location.speedAccuracy }
    return fix
  }
}
