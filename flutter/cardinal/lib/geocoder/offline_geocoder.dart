import '../store/store.dart';
import 'geocoder.dart';

/// The geocoder on the device, over the index built from the offline areas' map tiles.
///
/// A stand-in until the index (the `geocoder_flutter` crate) is in the app: it has no
/// index, so it finds nothing, as it will on a device with no offline area.
class OfflineGeocoder implements Geocoder {
  const OfflineGeocoder();

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
  }) async => const [];

  @override
  Future<Place?> place(String id, {String? lang, Future<void>? abort}) async =>
      null;
}
