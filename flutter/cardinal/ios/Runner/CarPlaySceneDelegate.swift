import CarPlay
import CoreText
import Flutter
import MapKit
import UIKit

/// A car's screen: the trip the phone is navigating, in CarPlay's map template, over
/// the app's own map.
///
/// The map is Flutter's, drawn by an engine of its own (`car_screen.dart`, started at
/// `carPlayMain`) in the window CarPlay gives navigation apps: Flutter on iOS shows one
/// view an engine, and the phone's engine has the phone's screen. That engine gets no
/// plugins: several of the app's keep one channel for the whole app, which a second
/// registration would take from the phone's engine.
///
/// Everything else is CarPlay's templates, filled from what the phone's engine says of
/// the trip (`car_trip.dart`, through `CarPlayBridge`):
///
/// - on the route, the next maneuver, with its icon from the app's icon font and the
///   distance to it, and the trip's distance and time left, as a navigation session,
///   which also feeds the instrument cluster and head-up display of cars that have
///   them;
/// - otherwise the session is paused with what the phone's status card says: waiting
///   for GPS, off the route, finding a new route, arrived;
/// - End in the navigation bar, and map buttons to mute and to show the rest of the
///   trip or follow again.
///
/// With no trip there's only the map: a trip is started on the phone.
final class CarPlaySceneDelegate: UIResponder, CPTemplateApplicationSceneDelegate {
  private let bridge = CarPlayBridge.shared
  private var interface: CPInterfaceController?
  private var window: CPWindow?
  private var engine: FlutterEngine?
  private var template: CPMapTemplate?

  private var session: CPNavigationSession?
  private var cpTrip: CPTrip?
  private var maneuver: CPManeuver?

  /// What the maneuver, its distance, the pause and the buttons were last made from,
  /// so each is told to CarPlay only when it changes.
  private var maneuverKey: String?
  private var maneuverDistanceKey: String?
  private var pauseKey: String?
  private var tripEstimateKey: String?
  private var buttonsKey: String?

  /// Whether the map shows the rest of the trip rather than following.
  private var overview = false

  func templateApplicationScene(
    _ templateApplicationScene: CPTemplateApplicationScene,
    didConnect interfaceController: CPInterfaceController,
    to window: CPWindow
  ) {
    interface = interfaceController
    self.window = window

    let engine = FlutterEngine(name: "cardinal.carplay")
    engine.run(withEntrypoint: "carPlayMain")
    bridge.attachScreen(messenger: engine.binaryMessenger)
    window.rootViewController = FlutterViewController(
      engine: engine, nibName: nil, bundle: nil)
    self.engine = engine

    let template = CPMapTemplate()
    template.automaticallyHidesNavigationBar = false
    self.template = template
    interfaceController.setRootTemplate(template, animated: false, completion: nil)

    bridge.onTrip = { [weak self] trip in self?.show(trip) }
    bridge.setConnected(true)
  }

  func templateApplicationScene(
    _ templateApplicationScene: CPTemplateApplicationScene,
    didDisconnect interfaceController: CPInterfaceController,
    from window: CPWindow
  ) {
    bridge.onTrip = nil
    bridge.setConnected(false)
    bridge.detachScreen()
    forgetSession()
    window.rootViewController = nil
    engine?.destroyContext()
    engine = nil
    template = nil
    interface = nil
    self.window = nil
  }

  // MARK: The trip

  private func show(_ trip: [String: Any]?) {
    guard let template else { return }
    guard let trip else {
      session?.finishTrip()
      forgetSession()
      showButtons(nil)
      return
    }
    if session == nil { begin(trip, in: template) }
    showButtons(trip)
    if let instruction = trip["instruction"] as? String {
      guide(trip, instruction: instruction)
    } else {
      pause(trip)
    }
    showTripEstimates(trip, in: template)
  }

  private func begin(_ trip: [String: Any], in template: CPMapTemplate) {
    let name = trip["destination"] as? String
    let destination: MKMapItem
    if let to = trip["to"] as? [String: Any], let lat = number(to["lat"]),
      let lng = number(to["lng"])
    {
      destination = MKMapItem(
        placemark: MKPlacemark(coordinate: CLLocationCoordinate2D(latitude: lat, longitude: lng)))
    } else {
      destination = MKMapItem()
    }
    destination.name = name
    let choice = CPRouteChoice(
      summaryVariants: [name ?? ""], additionalInformationVariants: [],
      selectionSummaryVariants: [])
    let cpTrip = CPTrip(
      origin: MKMapItem.forCurrentLocation(), destination: destination, routeChoices: [choice])
    self.cpTrip = cpTrip
    session = template.startNavigationSession(for: cpTrip)
  }

  private func forgetSession() {
    session = nil
    cpTrip = nil
    maneuver = nil
    maneuverKey = nil
    maneuverDistanceKey = nil
    pauseKey = nil
    tripEstimateKey = nil
    overview = false
  }

  /// Shows the next maneuver, or just its distance if it's the one already showing.
  private func guide(_ trip: [String: Any], instruction: String) {
    guard let session else { return }
    let estimates = self.estimates(trip["toManeuver"], seconds: trip["toManeuverSeconds"])
    let distanceKey = String(describing: trip["toManeuver"])
    let key = [
      String(describing: trip["route"]), String(describing: trip["step"]), instruction,
      String(describing: trip["glyph"]),
    ].joined(separator: "|")
    // A maneuver set again also takes the session out of a pause.
    if key != maneuverKey || pauseKey != nil {
      let maneuver = CPManeuver()
      maneuver.instructionVariants = [instruction]
      if let glyph = number(trip["glyph"]) {
        maneuver.symbolImage = ManeuverGlyph.image(codePoint: UInt32(glyph))
      }
      maneuver.initialTravelEstimates = estimates
      session.upcomingManeuvers = [maneuver]
      self.maneuver = maneuver
      maneuverKey = key
      maneuverDistanceKey = distanceKey
      pauseKey = nil
    } else if distanceKey != maneuverDistanceKey, let maneuver, let estimates {
      session.updateEstimates(estimates, for: maneuver)
      maneuverDistanceKey = distanceKey
    }
  }

  /// Pauses the session with what the phone's status card says, in the maneuver's
  /// place.
  private func pause(_ trip: [String: Any]) {
    guard let session else { return }
    let status = trip["status"] as? String
    let state = trip["state"] as? String ?? ""
    let key = "\(state)|\(status ?? "")"
    guard key != pauseKey else { return }
    let reason: CPNavigationSession.PauseReason
    switch state {
    case "arrived": reason = .arrived
    case "acquiring": reason = .locating
    case "rerouting": reason = .rerouting
    case "offRoute": reason = .proceedToRoute
    default: reason = .loading
    }
    session.pauseTrip(for: reason, description: status)
    pauseKey = key
    maneuverKey = nil
    maneuverDistanceKey = nil
  }

  private func showTripEstimates(_ trip: [String: Any], in template: CPMapTemplate) {
    guard let cpTrip else { return }
    let key =
      "\(String(describing: trip["remaining"]))|\(Int(number(trip["remainingSeconds"]) ?? 0) / 60)"
    guard key != tripEstimateKey,
      let estimates = self.estimates(trip["remaining"], seconds: trip["remainingSeconds"])
    else { return }
    template.updateEstimates(estimates, for: cpTrip)
    tripEstimateKey = key
  }

  // MARK: Buttons

  /// End while there's a trip, with the mute and overview buttons; nothing otherwise.
  private func showButtons(_ trip: [String: Any]?) {
    guard let template else { return }
    guard let trip else {
      if buttonsKey != nil {
        template.leadingNavigationBarButtons = []
        template.mapButtons = []
        buttonsKey = nil
      }
      return
    }
    let muted = trip["muted"] as? Bool ?? false
    let end = trip["end"] as? String ?? "End"
    let arrived = trip["state"] as? String == "arrived"
    let key = "\(muted)|\(overview)|\(end)|\(arrived)"
    guard key != buttonsKey else { return }
    buttonsKey = key

    template.leadingNavigationBarButtons = [
      CPBarButton(title: end) { [weak self] _ in self?.bridge.endTrip() }
    ]

    let mute = CPMapButton { [weak self] _ in self?.bridge.setMuted(!muted) }
    mute.image = UIImage(systemName: muted ? "speaker.slash.fill" : "speaker.wave.2.fill")

    let camera = CPMapButton { [weak self] _ in
      guard let self else { return }
      self.overview.toggle()
      self.bridge.setOverview(self.overview)
      // The button changes with the next message of the trip, a second off at most;
      // sooner if it's made again now.
      self.buttonsKey = nil
      self.showButtons(self.bridge.trip)
    }
    camera.image = UIImage(systemName: overview ? "location.fill" : "map.fill")

    // Once arrived the map shows the destination, and there's nothing to follow.
    template.mapButtons = arrived ? [mute] : [mute, camera]
  }

  // MARK: Reading the trip

  private func number(_ value: Any?) -> Double? {
    (value as? NSNumber)?.doubleValue
  }

  /// A distance as the phone rounded it (`value` of `unit`) and a time in seconds, as
  /// CarPlay takes them.
  private func estimates(_ distance: Any?, seconds: Any?) -> CPTravelEstimates? {
    guard let distance = distance as? [String: Any], let value = number(distance["value"])
    else { return nil }
    let unit: UnitLength
    switch distance["unit"] as? String {
    case "feet": unit = .feet
    case "miles": unit = .miles
    case "kilometers": unit = .kilometers
    default: unit = .meters
    }
    return CPTravelEstimates(
      distanceRemaining: Measurement(value: value, unit: unit),
      timeRemaining: number(seconds) ?? 0)
  }
}

/// Maneuver icons for CarPlay, drawn from the app's icon font (`CardinalIcons.ttf`,
/// among Flutter's assets), so the car shows the icons the phone does.
enum ManeuverGlyph {
  /// The side of the square the icon is drawn in, in points: CarPlay scales it to
  /// what each screen shows.
  private static let side: CGFloat = 50

  private static let font: CTFont? = {
    let key = FlutterDartProject.lookupKey(forAsset: "assets/fonts/CardinalIcons.ttf")
    guard let path = Bundle.main.path(forResource: key, ofType: nil),
      let provider = CGDataProvider(url: URL(fileURLWithPath: path) as CFURL),
      let font = CGFont(provider)
    else {
      NSLog("CardinalCarPlay: the icon font wasn't found")
      return nil
    }
    return CTFontCreateWithGraphicsFont(font, side, nil, nil)
  }()

  private static var images: [UInt32: UIImage] = [:]

  /// The font's icon at `codePoint`, as a template image for CarPlay to tint; nil if
  /// the font has none.
  static func image(codePoint: UInt32) -> UIImage? {
    if let image = images[codePoint] { return image }
    guard let font = font, let scalar = Unicode.Scalar(codePoint) else { return nil }
    let characters = Array(String(Character(scalar)).utf16)
    var glyphs = [CGGlyph](repeating: 0, count: characters.count)
    guard CTFontGetGlyphsForCharacters(font, characters, &glyphs, characters.count),
      let path = CTFontCreatePathForGlyph(font, glyphs[0], nil)
    else { return nil }
    let box = path.boundingBoxOfPath
    guard box.width > 0, box.height > 0 else { return nil }
    let size = CGSize(width: side, height: side)
    let scale = min(side / box.width, side / box.height, 1)
    let image = UIGraphicsImageRenderer(size: size).image { context in
      let cg = context.cgContext
      // Glyph outlines have y up; the image has it down.
      cg.translateBy(x: 0, y: side)
      cg.scaleBy(x: 1, y: -1)
      cg.translateBy(
        x: (side - box.width * scale) / 2 - box.minX * scale,
        y: (side - box.height * scale) / 2 - box.minY * scale)
      cg.scaleBy(x: scale, y: scale)
      cg.addPath(path)
      cg.setFillColor(UIColor.black.cgColor)
      cg.fillPath()
    }.withRenderingMode(.alwaysTemplate)
    images[codePoint] = image
    return image
  }
}
