/// Where you are: a dot on the map, with a circle as wide as the position's accuracy, and
/// the button that moves the map to it.
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:geolocator/geolocator.dart';
import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../store/store.dart';
import 'map.dart';

/// The style's GeoJSON source for where you are.
const locationSource = 'location';

const _empty = {'type': 'FeatureCollection', 'features': <Object>[]};
const _color = '#1a73e8';

/// Adds the location source and the layers that draw it to [style], a MapLibre style:
/// under the selected place's pin, if it has one, and over everything else.
void addLocationLayers(Map<String, Object?> style) {
  (style['sources'] as Map<String, Object?>)[locationSource] = {
    'type': 'geojson',
    'data': _empty,
  };
  final layers = style['layers'] as List<Object?>;
  final pin = layers.indexWhere((l) => (l as Map)['id'] == 'selection-pin');
  layers.insertAll(pin < 0 ? layers.length : pin, [
    {
      'id': 'location-accuracy',
      'type': 'fill',
      'source': locationSource,
      'filter': [
        '==',
        ['geometry-type'],
        'Polygon',
      ],
      'paint': {'fill-color': _color, 'fill-opacity': 0.15},
    },
    {
      'id': 'location-dot',
      'type': 'circle',
      'source': locationSource,
      'filter': [
        '==',
        ['geometry-type'],
        'Point',
      ],
      'paint': {
        'circle-radius': 7,
        'circle-color': _color,
        'circle-stroke-width': 2.5,
        'circle-stroke-color': '#ffffff',
      },
    },
  ]);
}

/// A position from the location service.
@immutable
class Fix {
  const Fix({required this.lng, required this.lat, required this.accuracy});

  final double lng;
  final double lat;

  /// How far off the position may be, in meters.
  final double accuracy;
}

/// The dot and its accuracy circle for [fix], or nothing for null, as GeoJSON.
Map<String, Object?> locationGeoJson(Fix? fix) {
  if (fix == null) return _empty;
  return {
    'type': 'FeatureCollection',
    'features': [
      if (fix.accuracy > 0)
        {
          'type': 'Feature',
          'properties': <String, Object?>{},
          'geometry': {
            'type': 'Polygon',
            'coordinates': [_circle(fix.lng, fix.lat, fix.accuracy)],
          },
        },
      {
        'type': 'Feature',
        'properties': <String, Object?>{},
        'geometry': {
          'type': 'Point',
          'coordinates': [fix.lng, fix.lat],
        },
      },
    ],
  };
}

/// A ring of [radius] meters around a point, closed, as GeoJSON coordinates. Close
/// enough to a circle on the sphere for the radii a position's accuracy has.
List<List<double>> _circle(double lng, double lat, double radius) {
  const steps = 48;
  const earthRadius = 6371008.8;
  final dLat = radius / earthRadius * 180 / math.pi;
  final dLng = dLat / math.max(math.cos(lat * math.pi / 180), 1e-6);
  return [
    for (var i = 0; i <= steps; i++)
      [
        lng + dLng * math.cos(2 * math.pi * i / steps),
        lat + dLat * math.sin(2 * math.pi * i / steps),
      ],
  ];
}

/// Whether the map could be moved to you, and if not, why.
enum LocateResult { located, serviceOff, denied, deniedForever, unavailable }

/// The platform's location service, behind what the app needs of it, so tests can fake it.
abstract class LocationSource {
  Future<bool> serviceEnabled();
  Future<LocationPermission> checkPermission();
  Future<LocationPermission> requestPermission();

  /// Positions as they change.
  Stream<Fix> positions();

  /// A position now, or null if there's none to be had.
  Future<Fix?> current();
  Future<void> openAppSettings();
  Future<void> openLocationSettings();
}

/// [LocationSource] from `geolocator`.
class GeolocatorSource implements LocationSource {
  const GeolocatorSource();

  static Fix _fix(Position p) =>
      Fix(lng: p.longitude, lat: p.latitude, accuracy: p.accuracy);

  @override
  Future<bool> serviceEnabled() => Geolocator.isLocationServiceEnabled();

  @override
  Future<LocationPermission> checkPermission() => Geolocator.checkPermission();

  @override
  Future<LocationPermission> requestPermission() =>
      Geolocator.requestPermission();

  @override
  Stream<Fix> positions() => Geolocator.getPositionStream(
    locationSettings: const LocationSettings(distanceFilter: 2),
  ).map(_fix);

  /// How old a position can be and still count as where you are now.
  static const _recent = Duration(seconds: 30);

  /// A recent position if there is one, since a new one can take many seconds.
  @override
  Future<Fix?> current() async {
    final last = await _lastKnown();
    if (last != null && DateTime.now().difference(last.timestamp) < _recent) {
      return _fix(last);
    }
    try {
      return _fix(await Geolocator.getCurrentPosition());
    } on Object {
      return last == null ? null : _fix(last);
    }
  }

  static Future<Position?> _lastKnown() async {
    try {
      return await Geolocator.getLastKnownPosition();
    } on Object {
      return null;
    }
  }

  @override
  Future<void> openAppSettings() => Geolocator.openAppSettings();

  @override
  Future<void> openLocationSettings() => Geolocator.openLocationSettings();
}

/// Whether the app has offered, on its first launch, to show where you are, and you
/// answered.
final locationOffered = Setting.flag('location.offered', false);

/// Whether [p] lets the app have your location.
bool locationAllowed(LocationPermission p) =>
    p == LocationPermission.always || p == LocationPermission.whileInUse;

/// Follows where you are once you've allowed it, and shows it on [map].
class LocationTracker {
  LocationTracker(this.map, {this.source = const GeolocatorSource()});

  final SparkleMapController map;
  final LocationSource source;

  /// The latest position, if any.
  Fix? get fix => _fix;
  Fix? _fix;
  StreamSubscription<Fix>? _positions;

  /// Starts showing where you are if you've already allowed it, without asking. A
  /// platform without a location service shows nothing.
  Future<void> startIfAllowed() async {
    try {
      if (locationAllowed(await source.checkPermission()) &&
          await source.serviceEnabled()) {
        _follow();
      }
    } on Object {
      return;
    }
  }

  void _follow() {
    _positions ??= source.positions().listen(_show, onError: (Object _) {});
  }

  void _show(Fix fix) {
    _fix = fix;
    map.setGeoJson(locationSource, locationGeoJson(fix));
  }

  /// Whether you've allowed the app your location, or null if the platform has no
  /// location service.
  Future<bool?> allowed() async {
    try {
      return locationAllowed(await source.checkPermission());
    } on Object {
      return null;
    }
  }

  /// Moves the map to where you are, asking for permission first if it hasn't been given.
  Future<LocateResult> locate() async {
    try {
      return await _locate();
    } on Object {
      // A platform without a location service.
      return LocateResult.unavailable;
    }
  }

  Future<LocateResult> _locate() async {
    if (!await source.serviceEnabled()) return LocateResult.serviceOff;
    var permission = await source.checkPermission();
    if (permission == LocationPermission.denied) {
      permission = await source.requestPermission();
    }
    if (permission == LocationPermission.deniedForever) {
      return LocateResult.deniedForever;
    }
    if (!locationAllowed(permission)) return LocateResult.denied;
    _follow();
    final fix = _fix ?? await source.current();
    if (fix == null) return LocateResult.unavailable;
    _show(fix);
    map.clearSubject();
    map.flyTo(
      lng: fix.lng,
      lat: fix.lat,
      zoom: math.max(locationZoom, map.camera?.zoom ?? 0),
    );
    return LocateResult.located;
  }

  void dispose() {
    _positions?.cancel();
    _positions = null;
  }
}

/// The zoom the map moves to you at, unless it's already closer.
const locationZoom = 15.0;

/// The button that moves the map to where you are, and keeps the dot there.
class MyLocationButton extends StatefulWidget {
  const MyLocationButton({super.key, this.source = const GeolocatorSource()});

  final LocationSource source;

  @override
  State<MyLocationButton> createState() => _MyLocationButtonState();
}

class _MyLocationButtonState extends State<MyLocationButton> {
  LocationTracker? _tracker;

  /// The dot appears as soon as you've allowed it, however that happened: directions can
  /// ask, and so can the system's settings, and the app resumes after either.
  late final AppLifecycleListener _lifecycle = AppLifecycleListener(
    onResume: () => _tracker?.startIfAllowed(),
  );

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_tracker != null) return;
    _tracker = LocationTracker(MapScope.of(context), source: widget.source)
      ..startIfAllowed();
    _lifecycle;
    final settings = StoreScope.of(context).settings;
    WidgetsBinding.instance.addPostFrameCallback((_) => _offer(settings));
  }

  /// How long the offer waits for the map to finish drawing, at most: offline, it may
  /// never.
  static const _mapWait = Duration(seconds: 10);

  /// On first launch, offers to show where you are, unless you already allowed it. Yes
  /// goes to you as the button does; either answer means it's never offered again, but
  /// dismissing it leaves it for next time. It waits for the map, so the map shows
  /// behind it.
  Future<void> _offer(SettingsStore settings) async {
    if (await settings.get(locationOffered)) return;
    final allowed = await _tracker!.allowed();
    if (allowed != false) {
      // Allowed already, or there's no location service to ask about.
      if (allowed == true) await settings.set(locationOffered, true);
      return;
    }
    await _tracker!.map.firstComplete.timeout(_mapWait, onTimeout: () {});
    if (!mounted) return;
    final yes = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Enable location?'),
        content: const Text(
          'Allow Cardinal Maps to access your location to show you where you are on the map '
          'and provide a better experience.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('No'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Yes'),
          ),
        ],
      ),
    );
    if (yes == null) return;
    await settings.set(locationOffered, true);
    if (yes && mounted) await _locate();
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    _tracker?.dispose();
    super.dispose();
  }

  Future<void> _locate() async {
    final tracker = _tracker!;
    final result = await tracker.locate();
    if (!mounted) return;
    final (String message, SnackBarAction? action) = switch (result) {
      LocateResult.located => ('', null),
      LocateResult.serviceOff => (
        'Location is off',
        SnackBarAction(
          label: 'Settings',
          onPressed: tracker.source.openLocationSettings,
        ),
      ),
      LocateResult.denied => (
        'Cardinal needs your location to show where you are',
        null,
      ),
      LocateResult.deniedForever => (
        'Location is turned off for Cardinal',
        SnackBarAction(
          label: 'Settings',
          onPressed: tracker.source.openAppSettings,
        ),
      ),
      LocateResult.unavailable => (
        "Your location isn't available right now",
        null,
      ),
    };
    if (message.isEmpty) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message), action: action));
  }

  @override
  Widget build(BuildContext context) => FloatingActionButton(
    heroTag: null,
    tooltip: 'My location',
    onPressed: _locate,
    child: const Icon(Icons.my_location),
  );
}
