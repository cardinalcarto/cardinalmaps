import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  /// Navigation's own location updates, kept for as long as the engine is.
  private var navigationFixes: FlutterEventChannel?
  private let navigationFixHandler = NavigationFixes()

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    if let registrar = engineBridge.pluginRegistry.registrar(forPlugin: "CardinalNavigation") {
      let channel = FlutterEventChannel(
        name: "cardinal/navigation/fixes", binaryMessenger: registrar.messenger())
      channel.setStreamHandler(navigationFixHandler)
      navigationFixes = channel
      // A car's screen is told of the trip from this engine (see `CarPlayBridge`).
      CarPlayBridge.shared.attachPhone(messenger: registrar.messenger())
    }
  }
}
