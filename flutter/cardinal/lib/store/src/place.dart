import 'package:collection/collection.dart';

/// A longitude and latitude, in degrees.
class LngLat {
  const LngLat(this.lon, this.lat);

  final double lon;
  final double lat;

  @override
  bool operator ==(Object other) => other is LngLat && other.lon == lon && other.lat == lat;

  @override
  int get hashCode => Object.hash(lon, lat);

  @override
  String toString() => 'LngLat($lon, $lat)';
}

/// A bounding box, in degrees.
class Bounds {
  const Bounds({required this.west, required this.south, required this.east, required this.north});

  final double west;
  final double south;
  final double east;
  final double north;

  @override
  bool operator ==(Object other) =>
      other is Bounds &&
      other.west == west &&
      other.south == south &&
      other.east == east &&
      other.north == north;

  @override
  int get hashCode => Object.hash(west, south, east, north);

  @override
  String toString() => 'Bounds($west, $south, $east, $north)';
}

/// A street address, from OSM's `addr:*` tags. Administrative areas are in [Place.admins].
class Address {
  const Address({this.housenumber, this.street, this.unit, this.postcode});

  factory Address.fromJson(Map<String, Object?> json) => Address(
    housenumber: json['housenumber'] as String?,
    street: json['street'] as String?,
    unit: json['unit'] as String?,
    postcode: json['postcode'] as String?,
  );

  final String? housenumber;
  final String? street;
  final String? unit;
  final String? postcode;

  Map<String, Object?> toJson() => {
    'housenumber': housenumber,
    'street': street,
    'unit': unit,
    'postcode': postcode,
  };

  @override
  bool operator ==(Object other) =>
      other is Address &&
      other.housenumber == housenumber &&
      other.street == street &&
      other.unit == unit &&
      other.postcode == postcode;

  @override
  int get hashCode => Object.hash(housenumber, street, unit, postcode);
}

/// An administrative area containing a place.
class AdminRef {
  const AdminRef({
    required this.id,
    required this.placetype,
    required this.name,
    this.abbreviation,
  });

  factory AdminRef.fromJson(Map<String, Object?> json) => AdminRef(
    id: json['id'] as String,
    placetype: json['placetype'] as String,
    name: json['name'] as String,
    abbreviation: json['abbreviation'] as String?,
  );

  /// `wof:<id>`.
  final String id;

  /// A Who's On First placetype, such as `locality`.
  final String placetype;
  final String name;

  /// A short form such as `WA`.
  final String? abbreviation;

  Map<String, Object?> toJson() => {
    'id': id,
    'placetype': placetype,
    'name': name,
    'abbreviation': abbreviation,
  };

  @override
  bool operator ==(Object other) =>
      other is AdminRef &&
      other.id == id &&
      other.placetype == placetype &&
      other.name == name &&
      other.abbreviation == abbreviation;

  @override
  int get hashCode => Object.hash(id, placetype, name, abbreviation);
}

/// A place with its own screen: a POI, an address or an administrative area.
///
/// What's known about a place depends on where it came from: a map pin knows its center,
/// address, admins and some tags, and a search result also knows its shape. Null fields
/// are unknown, and [merge] fills them in from what's already stored.
class Place {
  const Place({
    required this.id,
    required this.kind,
    this.version,
    this.name,
    this.countryCode,
    required this.center,
    this.bbox,
    this.shape,
    this.address,
    this.admins,
    this.tags = const {},
  });

  /// The geocoder's identifier: `poi:<id>` for POIs with a stable identifier, `wof:<id>`
  /// for administrative areas, and `osm:<type>/<id>` otherwise.
  final String id;

  /// `poi`, `address` or `admin`, as the geocoder says. Open: others may appear.
  final String kind;

  /// The OSM version of the element, for `poi:` and `osm:` places. Each `poi:` ID
  /// belongs to one element, so a higher version is fresher data.
  final int? version;

  /// The display name. Null for addresses, and if unknown.
  final String? name;

  /// The country it's in, as an ISO 3166-1 alpha-2 code such as `US`.
  final String? countryCode;

  /// Where to put a pin or label: a point inside the shape.
  final LngLat center;

  final Bounds? bbox;

  /// A GeoJSON geometry.
  final Map<String, Object?>? shape;

  final Address? address;

  /// The administrative areas containing the place, smallest first.
  final List<AdminRef>? admins;

  /// OSM tags. Empty for administrative areas, and possibly partial: map tiles carry only
  /// some.
  final Map<String, String> tags;

  /// What to store when [incoming] arrives and this is what's stored.
  ///
  /// If both versions are known and differ, the newer place wins whole: its shape or
  /// address may have changed. Otherwise they describe the same data (an OSM version has
  /// one set of tags), so [incoming]'s fields win, unknown fields keep what's stored, and
  /// tags are combined.
  Place merge(Place incoming) {
    assert(incoming.id == id);
    if (version != null && incoming.version != null && version != incoming.version) {
      return incoming.version! > version! ? incoming : this;
    }
    return Place(
      id: id,
      kind: incoming.kind,
      version: incoming.version ?? version,
      name: incoming.name ?? name,
      countryCode: incoming.countryCode ?? countryCode,
      center: incoming.center,
      bbox: incoming.bbox ?? bbox,
      shape: incoming.shape ?? shape,
      address: incoming.address ?? address,
      admins: incoming.admins ?? admins,
      tags: {...tags, ...incoming.tags},
    );
  }

  static const _deep = DeepCollectionEquality();

  @override
  bool operator ==(Object other) =>
      other is Place &&
      other.id == id &&
      other.kind == kind &&
      other.version == version &&
      other.name == name &&
      other.countryCode == countryCode &&
      other.center == center &&
      other.bbox == bbox &&
      _deep.equals(other.shape, shape) &&
      other.address == address &&
      _deep.equals(other.admins, admins) &&
      _deep.equals(other.tags, tags);

  @override
  int get hashCode => Object.hash(id, kind, version, name, center);

  @override
  String toString() => 'Place($id, $name, v$version)';
}
