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
  }

  @override
  void clearSubject() => subject = false;

  @override
  void setPadding(EdgeInsets padding) {}

  @override
  void setGeoJson(String source, Object geojson) => geoJson[source] = geojson;

  /// Without a map to draw, as if it had drawn everything at once.
  @override
  Future<void> get firstComplete async {}

  /// Where [flyTo] last sent the camera.
  ({double lng, double lat, double zoom})? flownTo;

  @override
  void flyTo({
    required double lng,
    required double lat,
    required double zoom,
  }) => flownTo = (lng: lng, lat: lat, zoom: zoom);
}
