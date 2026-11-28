import Flutter
import Foundation

/// Carries the trip from the phone's Flutter engine to a car's screen, and the car's
/// buttons back.
///
/// The phone's engine (`car_link.dart`, on `cardinal/carplay`) sends where the map's
/// tiles come from (`map`), the trip's line (`route`) and where the trip stands
/// (`trip`, null with no trip). The latest of each is kept and passed on to:
///
/// - `CarPlaySceneDelegate`, which shows the trip in CarPlay's templates (`onTrip`);
/// - the engine that draws the car's map (`car_screen.dart`, on
///   `cardinal/carplay/screen`), once it says it's `ready`, since it starts after the
///   car connects and may start in the middle of a trip. Its `map` message also says
///   where to keep the map's files, as it has no plugins to ask.
///
/// The phone's engine sends only while a car is connected: it's told as one connects
/// and goes (`connected`, `disconnected`), and asks when it starts (`hello`), in case
/// the car was there first.
///
/// Everything here is on the main thread, as Flutter's channels and CarPlay are.
final class CarPlayBridge {
  static let shared = CarPlayBridge()

  private var phone: FlutterMethodChannel?
  private var screen: FlutterMethodChannel?
  private var screenReady = false
  private var connected = false

  private var map: [String: Any]?
  private var route: Any?

  /// Where the trip stands, as the phone last said; nil with no trip.
  private(set) var trip: [String: Any]?

  /// Called with `trip` as it changes, while a car is connected.
  var onTrip: (([String: Any]?) -> Void)?

  /// Listens to the phone's engine, from when it's made.
  func attachPhone(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "cardinal/carplay", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      switch call.method {
      case "hello":
        result(self.connected)
      case "map":
        self.map = call.arguments as? [String: Any]
        self.sendMap()
        result(nil)
      case "route":
        self.route = call.arguments
        if self.screenReady { self.screen?.invokeMethod("route", arguments: self.route) }
        result(nil)
      case "trip":
        self.trip = call.arguments as? [String: Any]
        if self.screenReady { self.screen?.invokeMethod("trip", arguments: self.trip) }
        self.onTrip?(self.trip)
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    phone = channel
  }

  /// A car connected or went: the phone's engine starts or stops sending.
  func setConnected(_ connected: Bool) {
    self.connected = connected
    if !connected {
      // What's kept is of a trip the car no longer shows; a car that connects again
      // is sent it all anew.
      map = nil
      route = nil
      trip = nil
    }
    phone?.invokeMethod(connected ? "connected" : "disconnected", arguments: nil)
  }

  /// Listens to the engine that draws the car's map, and sends it what's kept once
  /// it's ready.
  func attachScreen(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(
      name: "cardinal/carplay/screen", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self, call.method == "ready" else { return result(FlutterMethodNotImplemented) }
      result(nil)
      self.screenReady = true
      self.sendMap()
      if let route = self.route { channel.invokeMethod("route", arguments: route) }
      channel.invokeMethod("trip", arguments: self.trip)
    }
    screen = channel
  }

  /// The car's map is gone, with its engine.
  func detachScreen() {
    screen?.setMethodCallHandler(nil)
    screen = nil
    screenReady = false
  }

  /// Tells the car's map to show the rest of the trip, or to follow it again.
  func setOverview(_ overview: Bool) {
    guard screenReady else { return }
    screen?.invokeMethod("camera", arguments: overview ? "overview" : "follow")
  }

  /// The car's End button.
  func endTrip() {
    phone?.invokeMethod("end", arguments: nil)
  }

  /// The car's mute button.
  func setMuted(_ muted: Bool) {
    phone?.invokeMethod("setMuted", arguments: muted)
  }

  private func sendMap() {
    guard screenReady, var config = map else { return }
    let files = FileManager.default
    config["support"] =
      files.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?.path
    config["cache"] = files.urls(for: .cachesDirectory, in: .userDomainMask).first?.path
    screen?.invokeMethod("map", arguments: config)
  }
}
