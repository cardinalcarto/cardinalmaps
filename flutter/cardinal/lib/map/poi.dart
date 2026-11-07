import 'package:sparkle/sparkle.dart';

import '../store/store.dart';
import 'iso_639.dart';

/// The layer of the map's tiles that POIs are in.
const poiLayer = 'poi';

/// The layer of the map's tiles naming the areas their POIs are in, by the areas' IDs.
const adminsLayer = 'admins';

/// The property of a feature in [adminsLayer] holding its [WofArea.fromNames].
const adminNames = 'names';

/// A POI's property holding the ID of its area of the placetype that follows.
const _adminPrefix = 'cardinal:admin:';

/// The placetypes POIs have areas of, smallest first.
const _placetypes = [
  'microhood',
  'neighbourhood',
  'macrohood',
  'borough',
  'locality',
  'localadmin',
  'county',
  'macrocounty',
  'region',
  'macroregion',
  'dependency',
  'country',
];

/// The placetypes named by the app's `wof_areas` asset rather than by tiles.
const namedInApp = {'region', 'macroregion', 'dependency', 'country'};

/// What tiles add to a POI's properties that isn't one of its OSM tags.
const _notTags = {'class', 'subclass', 'rank', 'agg_stop', 'indoor'};

/// A Who's On First area's names, as `wof_areas` and tiles have them.
class WofArea {
  const WofArea(this.names);

  /// `name` is its default name, `abbr` its abbreviation if it has one, `country` its
  /// ISO 3166-1 alpha-2 code if it's a country or dependency, and others its name in an
  /// ISO 639-3 language where that differs from the default.
  final Map<String, Object?> names;

  String? get defaultName => names['name'] as String?;
  String? get abbreviation => names['abbr'] as String?;
  String? get countryCode => names['country'] as String?;

  /// Its name in the ISO 639-1 language [lang], falling back to its default name.
  String? nameIn(String? lang) => names[iso639_3[lang]] as String? ?? defaultName;
}

/// The IDs of the areas a POI's properties say it's in, by placetype.
Map<String, int> poiAdmins(Map<String, String> properties) => {
  for (final placetype in _placetypes)
    placetype: ?int.tryParse(properties['$_adminPrefix$placetype'] ?? ''),
};

/// What the map's tiles say about the POI [feature], for its place card: the place the
/// geocoder calls `poi:<id>`, whose ID is the feature's.
///
/// [lang] is an ISO 639-1 code, for names in that language where there are any. [area]
/// finds an area the POI is in, if it's known.
Place placeFromPoi(
  ClickedFeature feature, {
  String? lang,
  WofArea? Function(String placetype, int id)? area,
}) {
  final p = feature.properties;
  final street = p['addr:street'];
  final housenumber = p['addr:housenumber'];
  final unit = p['addr:unit'];
  final postcode = p['addr:postcode'];
  final tagged = p['addr:country'];
  final admins = <AdminRef>[];
  String? country;
  for (final MapEntry(key: placetype, value: id) in poiAdmins(p).entries) {
    final found = area?.call(placetype, id);
    final name = found?.nameIn(lang);
    if (found == null || name == null) continue;
    admins.add(
      AdminRef(id: 'wof:$id', placetype: placetype, name: name, abbreviation: found.abbreviation),
    );
    // The smallest's, so places in Puerto Rico are in PR rather than US.
    country ??= found.countryCode;
  }
  return Place(
    id: 'poi:${feature.id}',
    kind: 'poi',
    version: int.tryParse(p['cardinal:version'] ?? ''),
    name: p['name:$lang'] ?? p['name'],
    countryCode: country ?? (tagged?.length == 2 ? tagged!.toUpperCase() : null),
    center: LngLat(feature.lng, feature.lat),
    address: street == null && housenumber == null && unit == null && postcode == null
        ? null
        : Address(housenumber: housenumber, street: street, unit: unit, postcode: postcode),
    admins: admins,
    tags: {
      for (final MapEntry(:key, :value) in p.entries)
        if (!_notTags.contains(key) && !key.startsWith('name_') && !key.startsWith('cardinal:'))
          key: value,
    },
  );
}
