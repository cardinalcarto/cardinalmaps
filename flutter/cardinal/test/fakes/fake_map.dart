import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

/// A map controller without a map, for widget tests (the map needs Flutter GPU): it
/// records the subject and GeoJSON it's given, and where it's flown.
class FakeMap extends SparkleMapController {
  /// Whether a subject is being kept in view; tests clear it to stand for the map being
  /// moved by hand.
  bool subject = false;
  int subjectsSet = 0;
  double? subjectMaxZoom;
  final geoJson = <String, Object>{};

  @override
  bool get hasSubject => subject;

  @override
  void setSubject({
    required double west,
    required double south,
    required double east,
    required double north,
    required double maxZoom,
    double margin = 0,
  }) {
    subject = true;
    subjectsSet++;
    subjectMaxZoom = maxZoom;
    subjectBounds = (west: west, south: south, east: east, north: north);
  }

  /// The bounds of the last subject set.
  ({double west, double south, double east, double north})? subjectBounds;

  @override
  void clearSubject() => subject = false;

  @override
  void setPadding(EdgeInsets padding) {}

  @override
  void setGeoJson(String source, Object geojson) {
    calls.add(source);
    geoJson[source] = geojson;
  }

  /// The images [addImages] was given, in order.
  final images = <SpriteImageInput>[];

  /// What's happened in order: `images` for [addImages], the source for [setGeoJson].
  final calls = <String>[];

  @override
  void addImages(List<SpriteImageInput> images) {
    calls.add('images');
    this.images.addAll(images);
  }

  /// Without a map to draw, as if it had drawn everything at once.
  @override
  Future<void> get firstComplete async {}

  /// The ground under the screen, if the test wants any: screen pixels as hundredths of
  /// a degree, from the origin.
  bool hasGround = false;

  @override
  ({double lng, double lat})? screenToLngLat(Offset position) =>
      hasGround ? (lng: position.dx / 100, lat: position.dy / 100) : null;

  /// Where [flyTo] last sent the camera.
  ({double lng, double lat, double zoom})? flownTo;

  @override
  void flyTo({
    required double lng,
    required double lat,
    required double zoom,
  }) => flownTo = (lng: lng, lat: lat, zoom: zoom);
}
