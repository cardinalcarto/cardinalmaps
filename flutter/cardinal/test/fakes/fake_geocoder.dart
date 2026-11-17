import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/store/store.dart';

/// A geocoder that answers every search with [results], and records what it was asked.
class FakeGeocoder implements Geocoder {
  FakeGeocoder([this.results = const []]);

  /// What every search finds.
  List<Place> results;

  /// The queries searched, in order.
  final asked = <String>[];

  @override
  Future<List<Place>> search(
    String query, {
    LngLat? focus,
    Viewport? viewport,
    String? lang,
    bool prefix = false,
    bool typos = false,
    int? limit,
    Future<void>? abort,
  }) async {
    asked.add(query);
    return results;
  }

  @override
  Future<Place?> place(String id, {String? lang, Future<void>? abort}) async =>
      results.where((p) => p.id == id).firstOrNull;
}
