// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'database.dart';

// ignore_for_file: type=lint
class $PlacesTable extends Places with TableInfo<$PlacesTable, PlaceRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $PlacesTable(this.attachedDatabase, [this._alias]);
  static const VerificationMeta _idMeta = const VerificationMeta('id');
  @override
  late final GeneratedColumn<String> id = GeneratedColumn<String>(
    'id',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _kindMeta = const VerificationMeta('kind');
  @override
  late final GeneratedColumn<String> kind = GeneratedColumn<String>(
    'kind',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _versionMeta = const VerificationMeta('version');
  @override
  late final GeneratedColumn<int> version = GeneratedColumn<int>(
    'version',
    aliasedName,
    true,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _nameMeta = const VerificationMeta('name');
  @override
  late final GeneratedColumn<String> name = GeneratedColumn<String>(
    'name',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _countryCodeMeta = const VerificationMeta('countryCode');
  @override
  late final GeneratedColumn<String> countryCode = GeneratedColumn<String>(
    'country_code',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _centerLonMeta = const VerificationMeta('centerLon');
  @override
  late final GeneratedColumn<double> centerLon = GeneratedColumn<double>(
    'center_lon',
    aliasedName,
    false,
    type: DriftSqlType.double,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _centerLatMeta = const VerificationMeta('centerLat');
  @override
  late final GeneratedColumn<double> centerLat = GeneratedColumn<double>(
    'center_lat',
    aliasedName,
    false,
    type: DriftSqlType.double,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _bboxWestMeta = const VerificationMeta('bboxWest');
  @override
  late final GeneratedColumn<double> bboxWest = GeneratedColumn<double>(
    'bbox_west',
    aliasedName,
    true,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _bboxSouthMeta = const VerificationMeta('bboxSouth');
  @override
  late final GeneratedColumn<double> bboxSouth = GeneratedColumn<double>(
    'bbox_south',
    aliasedName,
    true,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _bboxEastMeta = const VerificationMeta('bboxEast');
  @override
  late final GeneratedColumn<double> bboxEast = GeneratedColumn<double>(
    'bbox_east',
    aliasedName,
    true,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _bboxNorthMeta = const VerificationMeta('bboxNorth');
  @override
  late final GeneratedColumn<double> bboxNorth = GeneratedColumn<double>(
    'bbox_north',
    aliasedName,
    true,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _shapeMeta = const VerificationMeta('shape');
  @override
  late final GeneratedColumn<String> shape = GeneratedColumn<String>(
    'shape',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _addressMeta = const VerificationMeta('address');
  @override
  late final GeneratedColumn<String> address = GeneratedColumn<String>(
    'address',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _adminsMeta = const VerificationMeta('admins');
  @override
  late final GeneratedColumn<String> admins = GeneratedColumn<String>(
    'admins',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _tagsMeta = const VerificationMeta('tags');
  @override
  late final GeneratedColumn<String> tags = GeneratedColumn<String>(
    'tags',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _updatedAtMeta = const VerificationMeta('updatedAt');
  @override
  late final GeneratedColumn<DateTime> updatedAt = GeneratedColumn<DateTime>(
    'updated_at',
    aliasedName,
    false,
    type: DriftSqlType.dateTime,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _accessedAtMeta = const VerificationMeta('accessedAt');
  @override
  late final GeneratedColumn<DateTime> accessedAt = GeneratedColumn<DateTime>(
    'accessed_at',
    aliasedName,
    false,
    type: DriftSqlType.dateTime,
    requiredDuringInsert: true,
  );
  @override
  List<GeneratedColumn> get $columns => [
    id,
    kind,
    version,
    name,
    countryCode,
    centerLon,
    centerLat,
    bboxWest,
    bboxSouth,
    bboxEast,
    bboxNorth,
    shape,
    address,
    admins,
    tags,
    updatedAt,
    accessedAt,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'places';
  @override
  VerificationContext validateIntegrity(Insertable<PlaceRow> instance, {bool isInserting = false}) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('id')) {
      context.handle(_idMeta, id.isAcceptableOrUnknown(data['id']!, _idMeta));
    } else if (isInserting) {
      context.missing(_idMeta);
    }
    if (data.containsKey('kind')) {
      context.handle(_kindMeta, kind.isAcceptableOrUnknown(data['kind']!, _kindMeta));
    } else if (isInserting) {
      context.missing(_kindMeta);
    }
    if (data.containsKey('version')) {
      context.handle(_versionMeta, version.isAcceptableOrUnknown(data['version']!, _versionMeta));
    }
    if (data.containsKey('name')) {
      context.handle(_nameMeta, name.isAcceptableOrUnknown(data['name']!, _nameMeta));
    }
    if (data.containsKey('country_code')) {
      context.handle(
        _countryCodeMeta,
        countryCode.isAcceptableOrUnknown(data['country_code']!, _countryCodeMeta),
      );
    }
    if (data.containsKey('center_lon')) {
      context.handle(
        _centerLonMeta,
        centerLon.isAcceptableOrUnknown(data['center_lon']!, _centerLonMeta),
      );
    } else if (isInserting) {
      context.missing(_centerLonMeta);
    }
    if (data.containsKey('center_lat')) {
      context.handle(
        _centerLatMeta,
        centerLat.isAcceptableOrUnknown(data['center_lat']!, _centerLatMeta),
      );
    } else if (isInserting) {
      context.missing(_centerLatMeta);
    }
    if (data.containsKey('bbox_west')) {
      context.handle(
        _bboxWestMeta,
        bboxWest.isAcceptableOrUnknown(data['bbox_west']!, _bboxWestMeta),
      );
    }
    if (data.containsKey('bbox_south')) {
      context.handle(
        _bboxSouthMeta,
        bboxSouth.isAcceptableOrUnknown(data['bbox_south']!, _bboxSouthMeta),
      );
    }
    if (data.containsKey('bbox_east')) {
      context.handle(
        _bboxEastMeta,
        bboxEast.isAcceptableOrUnknown(data['bbox_east']!, _bboxEastMeta),
      );
    }
    if (data.containsKey('bbox_north')) {
      context.handle(
        _bboxNorthMeta,
        bboxNorth.isAcceptableOrUnknown(data['bbox_north']!, _bboxNorthMeta),
      );
    }
    if (data.containsKey('shape')) {
      context.handle(_shapeMeta, shape.isAcceptableOrUnknown(data['shape']!, _shapeMeta));
    }
    if (data.containsKey('address')) {
      context.handle(_addressMeta, address.isAcceptableOrUnknown(data['address']!, _addressMeta));
    }
    if (data.containsKey('admins')) {
      context.handle(_adminsMeta, admins.isAcceptableOrUnknown(data['admins']!, _adminsMeta));
    }
    if (data.containsKey('tags')) {
      context.handle(_tagsMeta, tags.isAcceptableOrUnknown(data['tags']!, _tagsMeta));
    } else if (isInserting) {
      context.missing(_tagsMeta);
    }
    if (data.containsKey('updated_at')) {
      context.handle(
        _updatedAtMeta,
        updatedAt.isAcceptableOrUnknown(data['updated_at']!, _updatedAtMeta),
      );
    } else if (isInserting) {
      context.missing(_updatedAtMeta);
    }
    if (data.containsKey('accessed_at')) {
      context.handle(
        _accessedAtMeta,
        accessedAt.isAcceptableOrUnknown(data['accessed_at']!, _accessedAtMeta),
      );
    } else if (isInserting) {
      context.missing(_accessedAtMeta);
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {id};
  @override
  PlaceRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return PlaceRow(
      id: attachedDatabase.typeMapping.read(DriftSqlType.string, data['${effectivePrefix}id'])!,
      kind: attachedDatabase.typeMapping.read(DriftSqlType.string, data['${effectivePrefix}kind'])!,
      version: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}version'],
      ),
      name: attachedDatabase.typeMapping.read(DriftSqlType.string, data['${effectivePrefix}name']),
      countryCode: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}country_code'],
      ),
      centerLon: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}center_lon'],
      )!,
      centerLat: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}center_lat'],
      )!,
      bboxWest: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}bbox_west'],
      ),
      bboxSouth: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}bbox_south'],
      ),
      bboxEast: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}bbox_east'],
      ),
      bboxNorth: attachedDatabase.typeMapping.read(
        DriftSqlType.double,
        data['${effectivePrefix}bbox_north'],
      ),
      shape: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}shape'],
      ),
      address: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}address'],
      ),
      admins: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}admins'],
      ),
      tags: attachedDatabase.typeMapping.read(DriftSqlType.string, data['${effectivePrefix}tags'])!,
      updatedAt: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}updated_at'],
      )!,
      accessedAt: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}accessed_at'],
      )!,
    );
  }

  @override
  $PlacesTable createAlias(String alias) {
    return $PlacesTable(attachedDatabase, alias);
  }
}

class PlaceRow extends DataClass implements Insertable<PlaceRow> {
  /// The geocoder's identifier: `poi:<id>`, `wof:<id>` or `osm:<type>/<id>`.
  final String id;
  final String kind;
  final int? version;
  final String? name;

  /// ISO 3166-1 alpha-2, such as `US`.
  final String? countryCode;
  final double centerLon;
  final double centerLat;
  final double? bboxWest;
  final double? bboxSouth;
  final double? bboxEast;
  final double? bboxNorth;

  /// A GeoJSON geometry.
  final String? shape;

  /// JSON, as in [Place].
  final String? address;
  final String? admins;
  final String tags;
  final DateTime updatedAt;
  final DateTime accessedAt;
  const PlaceRow({
    required this.id,
    required this.kind,
    this.version,
    this.name,
    this.countryCode,
    required this.centerLon,
    required this.centerLat,
    this.bboxWest,
    this.bboxSouth,
    this.bboxEast,
    this.bboxNorth,
    this.shape,
    this.address,
    this.admins,
    required this.tags,
    required this.updatedAt,
    required this.accessedAt,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['id'] = Variable<String>(id);
    map['kind'] = Variable<String>(kind);
    if (!nullToAbsent || version != null) {
      map['version'] = Variable<int>(version);
    }
    if (!nullToAbsent || name != null) {
      map['name'] = Variable<String>(name);
    }
    if (!nullToAbsent || countryCode != null) {
      map['country_code'] = Variable<String>(countryCode);
    }
    map['center_lon'] = Variable<double>(centerLon);
    map['center_lat'] = Variable<double>(centerLat);
    if (!nullToAbsent || bboxWest != null) {
      map['bbox_west'] = Variable<double>(bboxWest);
    }
    if (!nullToAbsent || bboxSouth != null) {
      map['bbox_south'] = Variable<double>(bboxSouth);
    }
    if (!nullToAbsent || bboxEast != null) {
      map['bbox_east'] = Variable<double>(bboxEast);
    }
    if (!nullToAbsent || bboxNorth != null) {
      map['bbox_north'] = Variable<double>(bboxNorth);
    }
    if (!nullToAbsent || shape != null) {
      map['shape'] = Variable<String>(shape);
    }
    if (!nullToAbsent || address != null) {
      map['address'] = Variable<String>(address);
    }
    if (!nullToAbsent || admins != null) {
      map['admins'] = Variable<String>(admins);
    }
    map['tags'] = Variable<String>(tags);
    map['updated_at'] = Variable<DateTime>(updatedAt);
    map['accessed_at'] = Variable<DateTime>(accessedAt);
    return map;
  }

  PlacesCompanion toCompanion(bool nullToAbsent) {
    return PlacesCompanion(
      id: Value(id),
      kind: Value(kind),
      version: version == null && nullToAbsent ? const Value.absent() : Value(version),
      name: name == null && nullToAbsent ? const Value.absent() : Value(name),
      countryCode: countryCode == null && nullToAbsent ? const Value.absent() : Value(countryCode),
      centerLon: Value(centerLon),
      centerLat: Value(centerLat),
      bboxWest: bboxWest == null && nullToAbsent ? const Value.absent() : Value(bboxWest),
      bboxSouth: bboxSouth == null && nullToAbsent ? const Value.absent() : Value(bboxSouth),
      bboxEast: bboxEast == null && nullToAbsent ? const Value.absent() : Value(bboxEast),
      bboxNorth: bboxNorth == null && nullToAbsent ? const Value.absent() : Value(bboxNorth),
      shape: shape == null && nullToAbsent ? const Value.absent() : Value(shape),
      address: address == null && nullToAbsent ? const Value.absent() : Value(address),
      admins: admins == null && nullToAbsent ? const Value.absent() : Value(admins),
      tags: Value(tags),
      updatedAt: Value(updatedAt),
      accessedAt: Value(accessedAt),
    );
  }

  factory PlaceRow.fromJson(Map<String, dynamic> json, {ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return PlaceRow(
      id: serializer.fromJson<String>(json['id']),
      kind: serializer.fromJson<String>(json['kind']),
      version: serializer.fromJson<int?>(json['version']),
      name: serializer.fromJson<String?>(json['name']),
      countryCode: serializer.fromJson<String?>(json['countryCode']),
      centerLon: serializer.fromJson<double>(json['centerLon']),
      centerLat: serializer.fromJson<double>(json['centerLat']),
      bboxWest: serializer.fromJson<double?>(json['bboxWest']),
      bboxSouth: serializer.fromJson<double?>(json['bboxSouth']),
      bboxEast: serializer.fromJson<double?>(json['bboxEast']),
      bboxNorth: serializer.fromJson<double?>(json['bboxNorth']),
      shape: serializer.fromJson<String?>(json['shape']),
      address: serializer.fromJson<String?>(json['address']),
      admins: serializer.fromJson<String?>(json['admins']),
      tags: serializer.fromJson<String>(json['tags']),
      updatedAt: serializer.fromJson<DateTime>(json['updatedAt']),
      accessedAt: serializer.fromJson<DateTime>(json['accessedAt']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'id': serializer.toJson<String>(id),
      'kind': serializer.toJson<String>(kind),
      'version': serializer.toJson<int?>(version),
      'name': serializer.toJson<String?>(name),
      'countryCode': serializer.toJson<String?>(countryCode),
      'centerLon': serializer.toJson<double>(centerLon),
      'centerLat': serializer.toJson<double>(centerLat),
      'bboxWest': serializer.toJson<double?>(bboxWest),
      'bboxSouth': serializer.toJson<double?>(bboxSouth),
      'bboxEast': serializer.toJson<double?>(bboxEast),
      'bboxNorth': serializer.toJson<double?>(bboxNorth),
      'shape': serializer.toJson<String?>(shape),
      'address': serializer.toJson<String?>(address),
      'admins': serializer.toJson<String?>(admins),
      'tags': serializer.toJson<String>(tags),
      'updatedAt': serializer.toJson<DateTime>(updatedAt),
      'accessedAt': serializer.toJson<DateTime>(accessedAt),
    };
  }

  PlaceRow copyWith({
    String? id,
    String? kind,
    Value<int?> version = const Value.absent(),
    Value<String?> name = const Value.absent(),
    Value<String?> countryCode = const Value.absent(),
    double? centerLon,
    double? centerLat,
    Value<double?> bboxWest = const Value.absent(),
    Value<double?> bboxSouth = const Value.absent(),
    Value<double?> bboxEast = const Value.absent(),
    Value<double?> bboxNorth = const Value.absent(),
    Value<String?> shape = const Value.absent(),
    Value<String?> address = const Value.absent(),
    Value<String?> admins = const Value.absent(),
    String? tags,
    DateTime? updatedAt,
    DateTime? accessedAt,
  }) => PlaceRow(
    id: id ?? this.id,
    kind: kind ?? this.kind,
    version: version.present ? version.value : this.version,
    name: name.present ? name.value : this.name,
    countryCode: countryCode.present ? countryCode.value : this.countryCode,
    centerLon: centerLon ?? this.centerLon,
    centerLat: centerLat ?? this.centerLat,
    bboxWest: bboxWest.present ? bboxWest.value : this.bboxWest,
    bboxSouth: bboxSouth.present ? bboxSouth.value : this.bboxSouth,
    bboxEast: bboxEast.present ? bboxEast.value : this.bboxEast,
    bboxNorth: bboxNorth.present ? bboxNorth.value : this.bboxNorth,
    shape: shape.present ? shape.value : this.shape,
    address: address.present ? address.value : this.address,
    admins: admins.present ? admins.value : this.admins,
    tags: tags ?? this.tags,
    updatedAt: updatedAt ?? this.updatedAt,
    accessedAt: accessedAt ?? this.accessedAt,
  );
  PlaceRow copyWithCompanion(PlacesCompanion data) {
    return PlaceRow(
      id: data.id.present ? data.id.value : this.id,
      kind: data.kind.present ? data.kind.value : this.kind,
      version: data.version.present ? data.version.value : this.version,
      name: data.name.present ? data.name.value : this.name,
      countryCode: data.countryCode.present ? data.countryCode.value : this.countryCode,
      centerLon: data.centerLon.present ? data.centerLon.value : this.centerLon,
      centerLat: data.centerLat.present ? data.centerLat.value : this.centerLat,
      bboxWest: data.bboxWest.present ? data.bboxWest.value : this.bboxWest,
      bboxSouth: data.bboxSouth.present ? data.bboxSouth.value : this.bboxSouth,
      bboxEast: data.bboxEast.present ? data.bboxEast.value : this.bboxEast,
      bboxNorth: data.bboxNorth.present ? data.bboxNorth.value : this.bboxNorth,
      shape: data.shape.present ? data.shape.value : this.shape,
      address: data.address.present ? data.address.value : this.address,
      admins: data.admins.present ? data.admins.value : this.admins,
      tags: data.tags.present ? data.tags.value : this.tags,
      updatedAt: data.updatedAt.present ? data.updatedAt.value : this.updatedAt,
      accessedAt: data.accessedAt.present ? data.accessedAt.value : this.accessedAt,
    );
  }

  @override
  String toString() {
    return (StringBuffer('PlaceRow(')
          ..write('id: $id, ')
          ..write('kind: $kind, ')
          ..write('version: $version, ')
          ..write('name: $name, ')
          ..write('countryCode: $countryCode, ')
          ..write('centerLon: $centerLon, ')
          ..write('centerLat: $centerLat, ')
          ..write('bboxWest: $bboxWest, ')
          ..write('bboxSouth: $bboxSouth, ')
          ..write('bboxEast: $bboxEast, ')
          ..write('bboxNorth: $bboxNorth, ')
          ..write('shape: $shape, ')
          ..write('address: $address, ')
          ..write('admins: $admins, ')
          ..write('tags: $tags, ')
          ..write('updatedAt: $updatedAt, ')
          ..write('accessedAt: $accessedAt')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(
    id,
    kind,
    version,
    name,
    countryCode,
    centerLon,
    centerLat,
    bboxWest,
    bboxSouth,
    bboxEast,
    bboxNorth,
    shape,
    address,
    admins,
    tags,
    updatedAt,
    accessedAt,
  );
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is PlaceRow &&
          other.id == this.id &&
          other.kind == this.kind &&
          other.version == this.version &&
          other.name == this.name &&
          other.countryCode == this.countryCode &&
          other.centerLon == this.centerLon &&
          other.centerLat == this.centerLat &&
          other.bboxWest == this.bboxWest &&
          other.bboxSouth == this.bboxSouth &&
          other.bboxEast == this.bboxEast &&
          other.bboxNorth == this.bboxNorth &&
          other.shape == this.shape &&
          other.address == this.address &&
          other.admins == this.admins &&
          other.tags == this.tags &&
          other.updatedAt == this.updatedAt &&
          other.accessedAt == this.accessedAt);
}

class PlacesCompanion extends UpdateCompanion<PlaceRow> {
  final Value<String> id;
  final Value<String> kind;
  final Value<int?> version;
  final Value<String?> name;
  final Value<String?> countryCode;
  final Value<double> centerLon;
  final Value<double> centerLat;
  final Value<double?> bboxWest;
  final Value<double?> bboxSouth;
  final Value<double?> bboxEast;
  final Value<double?> bboxNorth;
  final Value<String?> shape;
  final Value<String?> address;
  final Value<String?> admins;
  final Value<String> tags;
  final Value<DateTime> updatedAt;
  final Value<DateTime> accessedAt;
  final Value<int> rowid;
  const PlacesCompanion({
    this.id = const Value.absent(),
    this.kind = const Value.absent(),
    this.version = const Value.absent(),
    this.name = const Value.absent(),
    this.countryCode = const Value.absent(),
    this.centerLon = const Value.absent(),
    this.centerLat = const Value.absent(),
    this.bboxWest = const Value.absent(),
    this.bboxSouth = const Value.absent(),
    this.bboxEast = const Value.absent(),
    this.bboxNorth = const Value.absent(),
    this.shape = const Value.absent(),
    this.address = const Value.absent(),
    this.admins = const Value.absent(),
    this.tags = const Value.absent(),
    this.updatedAt = const Value.absent(),
    this.accessedAt = const Value.absent(),
    this.rowid = const Value.absent(),
  });
  PlacesCompanion.insert({
    required String id,
    required String kind,
    this.version = const Value.absent(),
    this.name = const Value.absent(),
    this.countryCode = const Value.absent(),
    required double centerLon,
    required double centerLat,
    this.bboxWest = const Value.absent(),
    this.bboxSouth = const Value.absent(),
    this.bboxEast = const Value.absent(),
    this.bboxNorth = const Value.absent(),
    this.shape = const Value.absent(),
    this.address = const Value.absent(),
    this.admins = const Value.absent(),
    required String tags,
    required DateTime updatedAt,
    required DateTime accessedAt,
    this.rowid = const Value.absent(),
  }) : id = Value(id),
       kind = Value(kind),
       centerLon = Value(centerLon),
       centerLat = Value(centerLat),
       tags = Value(tags),
       updatedAt = Value(updatedAt),
       accessedAt = Value(accessedAt);
  static Insertable<PlaceRow> custom({
    Expression<String>? id,
    Expression<String>? kind,
    Expression<int>? version,
    Expression<String>? name,
    Expression<String>? countryCode,
    Expression<double>? centerLon,
    Expression<double>? centerLat,
    Expression<double>? bboxWest,
    Expression<double>? bboxSouth,
    Expression<double>? bboxEast,
    Expression<double>? bboxNorth,
    Expression<String>? shape,
    Expression<String>? address,
    Expression<String>? admins,
    Expression<String>? tags,
    Expression<DateTime>? updatedAt,
    Expression<DateTime>? accessedAt,
    Expression<int>? rowid,
  }) {
    return RawValuesInsertable({
      if (id != null) 'id': id,
      if (kind != null) 'kind': kind,
      if (version != null) 'version': version,
      if (name != null) 'name': name,
      if (countryCode != null) 'country_code': countryCode,
      if (centerLon != null) 'center_lon': centerLon,
      if (centerLat != null) 'center_lat': centerLat,
      if (bboxWest != null) 'bbox_west': bboxWest,
      if (bboxSouth != null) 'bbox_south': bboxSouth,
      if (bboxEast != null) 'bbox_east': bboxEast,
      if (bboxNorth != null) 'bbox_north': bboxNorth,
      if (shape != null) 'shape': shape,
      if (address != null) 'address': address,
      if (admins != null) 'admins': admins,
      if (tags != null) 'tags': tags,
      if (updatedAt != null) 'updated_at': updatedAt,
      if (accessedAt != null) 'accessed_at': accessedAt,
      if (rowid != null) 'rowid': rowid,
    });
  }

  PlacesCompanion copyWith({
    Value<String>? id,
    Value<String>? kind,
    Value<int?>? version,
    Value<String?>? name,
    Value<String?>? countryCode,
    Value<double>? centerLon,
    Value<double>? centerLat,
    Value<double?>? bboxWest,
    Value<double?>? bboxSouth,
    Value<double?>? bboxEast,
    Value<double?>? bboxNorth,
    Value<String?>? shape,
    Value<String?>? address,
    Value<String?>? admins,
    Value<String>? tags,
    Value<DateTime>? updatedAt,
    Value<DateTime>? accessedAt,
    Value<int>? rowid,
  }) {
    return PlacesCompanion(
      id: id ?? this.id,
      kind: kind ?? this.kind,
      version: version ?? this.version,
      name: name ?? this.name,
      countryCode: countryCode ?? this.countryCode,
      centerLon: centerLon ?? this.centerLon,
      centerLat: centerLat ?? this.centerLat,
      bboxWest: bboxWest ?? this.bboxWest,
      bboxSouth: bboxSouth ?? this.bboxSouth,
      bboxEast: bboxEast ?? this.bboxEast,
      bboxNorth: bboxNorth ?? this.bboxNorth,
      shape: shape ?? this.shape,
      address: address ?? this.address,
      admins: admins ?? this.admins,
      tags: tags ?? this.tags,
      updatedAt: updatedAt ?? this.updatedAt,
      accessedAt: accessedAt ?? this.accessedAt,
      rowid: rowid ?? this.rowid,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (id.present) {
      map['id'] = Variable<String>(id.value);
    }
    if (kind.present) {
      map['kind'] = Variable<String>(kind.value);
    }
    if (version.present) {
      map['version'] = Variable<int>(version.value);
    }
    if (name.present) {
      map['name'] = Variable<String>(name.value);
    }
    if (countryCode.present) {
      map['country_code'] = Variable<String>(countryCode.value);
    }
    if (centerLon.present) {
      map['center_lon'] = Variable<double>(centerLon.value);
    }
    if (centerLat.present) {
      map['center_lat'] = Variable<double>(centerLat.value);
    }
    if (bboxWest.present) {
      map['bbox_west'] = Variable<double>(bboxWest.value);
    }
    if (bboxSouth.present) {
      map['bbox_south'] = Variable<double>(bboxSouth.value);
    }
    if (bboxEast.present) {
      map['bbox_east'] = Variable<double>(bboxEast.value);
    }
    if (bboxNorth.present) {
      map['bbox_north'] = Variable<double>(bboxNorth.value);
    }
    if (shape.present) {
      map['shape'] = Variable<String>(shape.value);
    }
    if (address.present) {
      map['address'] = Variable<String>(address.value);
    }
    if (admins.present) {
      map['admins'] = Variable<String>(admins.value);
    }
    if (tags.present) {
      map['tags'] = Variable<String>(tags.value);
    }
    if (updatedAt.present) {
      map['updated_at'] = Variable<DateTime>(updatedAt.value);
    }
    if (accessedAt.present) {
      map['accessed_at'] = Variable<DateTime>(accessedAt.value);
    }
    if (rowid.present) {
      map['rowid'] = Variable<int>(rowid.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('PlacesCompanion(')
          ..write('id: $id, ')
          ..write('kind: $kind, ')
          ..write('version: $version, ')
          ..write('name: $name, ')
          ..write('countryCode: $countryCode, ')
          ..write('centerLon: $centerLon, ')
          ..write('centerLat: $centerLat, ')
          ..write('bboxWest: $bboxWest, ')
          ..write('bboxSouth: $bboxSouth, ')
          ..write('bboxEast: $bboxEast, ')
          ..write('bboxNorth: $bboxNorth, ')
          ..write('shape: $shape, ')
          ..write('address: $address, ')
          ..write('admins: $admins, ')
          ..write('tags: $tags, ')
          ..write('updatedAt: $updatedAt, ')
          ..write('accessedAt: $accessedAt, ')
          ..write('rowid: $rowid')
          ..write(')'))
        .toString();
  }
}

abstract class _$CardinalDatabase extends GeneratedDatabase {
  _$CardinalDatabase(QueryExecutor e) : super(e);
  $CardinalDatabaseManager get managers => $CardinalDatabaseManager(this);
  late final $PlacesTable places = $PlacesTable(this);
  @override
  Iterable<TableInfo<Table, Object?>> get allTables =>
      allSchemaEntities.whereType<TableInfo<Table, Object?>>();
  @override
  List<DatabaseSchemaEntity> get allSchemaEntities => [places];
}

typedef $$PlacesTableCreateCompanionBuilder = PlacesCompanion Function({
  required String id,
  required String kind,
  Value<int?> version,
  Value<String?> name,
  Value<String?> countryCode,
  required double centerLon,
  required double centerLat,
  Value<double?> bboxWest,
  Value<double?> bboxSouth,
  Value<double?> bboxEast,
  Value<double?> bboxNorth,
  Value<String?> shape,
  Value<String?> address,
  Value<String?> admins,
  required String tags,
  required DateTime updatedAt,
  required DateTime accessedAt,
  Value<int> rowid,
});
typedef $$PlacesTableUpdateCompanionBuilder = PlacesCompanion Function({
  Value<String> id,
  Value<String> kind,
  Value<int?> version,
  Value<String?> name,
  Value<String?> countryCode,
  Value<double> centerLon,
  Value<double> centerLat,
  Value<double?> bboxWest,
  Value<double?> bboxSouth,
  Value<double?> bboxEast,
  Value<double?> bboxNorth,
  Value<String?> shape,
  Value<String?> address,
  Value<String?> admins,
  Value<String> tags,
  Value<DateTime> updatedAt,
  Value<DateTime> accessedAt,
  Value<int> rowid,
});

class $$PlacesTableFilterComposer extends Composer<_$CardinalDatabase, $PlacesTable> {
  $$PlacesTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnFilters<String> get id =>
      $composableBuilder(column: $table.id, builder: (column) => ColumnFilters(column));

  ColumnFilters<String> get kind =>
      $composableBuilder(column: $table.kind, builder: (column) => ColumnFilters(column));

  ColumnFilters<int> get version =>
      $composableBuilder(column: $table.version, builder: (column) => ColumnFilters(column));

  ColumnFilters<String> get name =>
      $composableBuilder(column: $table.name, builder: (column) => ColumnFilters(column));

  ColumnFilters<String> get countryCode =>
      $composableBuilder(column: $table.countryCode, builder: (column) => ColumnFilters(column));

  ColumnFilters<double> get centerLon =>
      $composableBuilder(column: $table.centerLon, builder: (column) => ColumnFilters(column));

  ColumnFilters<double> get centerLat =>
      $composableBuilder(column: $table.centerLat, builder: (column) => ColumnFilters(column));

  ColumnFilters<double> get bboxWest =>
      $composableBuilder(column: $table.bboxWest, builder: (column) => ColumnFilters(column));

  ColumnFilters<double> get bboxSouth =>
      $composableBuilder(column: $table.bboxSouth, builder: (column) => ColumnFilters(column));

  ColumnFilters<double> get bboxEast =>
      $composableBuilder(column: $table.bboxEast, builder: (column) => ColumnFilters(column));

  ColumnFilters<double> get bboxNorth =>
      $composableBuilder(column: $table.bboxNorth, builder: (column) => ColumnFilters(column));

  ColumnFilters<String> get shape =>
      $composableBuilder(column: $table.shape, builder: (column) => ColumnFilters(column));

  ColumnFilters<String> get address =>
      $composableBuilder(column: $table.address, builder: (column) => ColumnFilters(column));

  ColumnFilters<String> get admins =>
      $composableBuilder(column: $table.admins, builder: (column) => ColumnFilters(column));

  ColumnFilters<String> get tags =>
      $composableBuilder(column: $table.tags, builder: (column) => ColumnFilters(column));

  ColumnFilters<DateTime> get updatedAt =>
      $composableBuilder(column: $table.updatedAt, builder: (column) => ColumnFilters(column));

  ColumnFilters<DateTime> get accessedAt =>
      $composableBuilder(column: $table.accessedAt, builder: (column) => ColumnFilters(column));
}

class $$PlacesTableOrderingComposer extends Composer<_$CardinalDatabase, $PlacesTable> {
  $$PlacesTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<String> get id =>
      $composableBuilder(column: $table.id, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<String> get kind =>
      $composableBuilder(column: $table.kind, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<int> get version =>
      $composableBuilder(column: $table.version, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<String> get name =>
      $composableBuilder(column: $table.name, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<String> get countryCode =>
      $composableBuilder(column: $table.countryCode, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<double> get centerLon =>
      $composableBuilder(column: $table.centerLon, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<double> get centerLat =>
      $composableBuilder(column: $table.centerLat, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<double> get bboxWest =>
      $composableBuilder(column: $table.bboxWest, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<double> get bboxSouth =>
      $composableBuilder(column: $table.bboxSouth, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<double> get bboxEast =>
      $composableBuilder(column: $table.bboxEast, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<double> get bboxNorth =>
      $composableBuilder(column: $table.bboxNorth, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<String> get shape =>
      $composableBuilder(column: $table.shape, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<String> get address =>
      $composableBuilder(column: $table.address, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<String> get admins =>
      $composableBuilder(column: $table.admins, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<String> get tags =>
      $composableBuilder(column: $table.tags, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<DateTime> get updatedAt =>
      $composableBuilder(column: $table.updatedAt, builder: (column) => ColumnOrderings(column));

  ColumnOrderings<DateTime> get accessedAt =>
      $composableBuilder(column: $table.accessedAt, builder: (column) => ColumnOrderings(column));
}

class $$PlacesTableAnnotationComposer extends Composer<_$CardinalDatabase, $PlacesTable> {
  $$PlacesTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<String> get id =>
      $composableBuilder(column: $table.id, builder: (column) => column);

  GeneratedColumn<String> get kind =>
      $composableBuilder(column: $table.kind, builder: (column) => column);

  GeneratedColumn<int> get version =>
      $composableBuilder(column: $table.version, builder: (column) => column);

  GeneratedColumn<String> get name =>
      $composableBuilder(column: $table.name, builder: (column) => column);

  GeneratedColumn<String> get countryCode =>
      $composableBuilder(column: $table.countryCode, builder: (column) => column);

  GeneratedColumn<double> get centerLon =>
      $composableBuilder(column: $table.centerLon, builder: (column) => column);

  GeneratedColumn<double> get centerLat =>
      $composableBuilder(column: $table.centerLat, builder: (column) => column);

  GeneratedColumn<double> get bboxWest =>
      $composableBuilder(column: $table.bboxWest, builder: (column) => column);

  GeneratedColumn<double> get bboxSouth =>
      $composableBuilder(column: $table.bboxSouth, builder: (column) => column);

  GeneratedColumn<double> get bboxEast =>
      $composableBuilder(column: $table.bboxEast, builder: (column) => column);

  GeneratedColumn<double> get bboxNorth =>
      $composableBuilder(column: $table.bboxNorth, builder: (column) => column);

  GeneratedColumn<String> get shape =>
      $composableBuilder(column: $table.shape, builder: (column) => column);

  GeneratedColumn<String> get address =>
      $composableBuilder(column: $table.address, builder: (column) => column);

  GeneratedColumn<String> get admins =>
      $composableBuilder(column: $table.admins, builder: (column) => column);

  GeneratedColumn<String> get tags =>
      $composableBuilder(column: $table.tags, builder: (column) => column);

  GeneratedColumn<DateTime> get updatedAt =>
      $composableBuilder(column: $table.updatedAt, builder: (column) => column);

  GeneratedColumn<DateTime> get accessedAt =>
      $composableBuilder(column: $table.accessedAt, builder: (column) => column);
}

class $$PlacesTableTableManager
    extends
        RootTableManager<
          _$CardinalDatabase,
          $PlacesTable,
          PlaceRow,
          $$PlacesTableFilterComposer,
          $$PlacesTableOrderingComposer,
          $$PlacesTableAnnotationComposer,
          $$PlacesTableCreateCompanionBuilder,
          $$PlacesTableUpdateCompanionBuilder,
          (PlaceRow, BaseReferences<_$CardinalDatabase, $PlacesTable, PlaceRow>),
          PlaceRow,
          PrefetchHooks Function()
        > {
  $$PlacesTableTableManager(_$CardinalDatabase db, $PlacesTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () => $$PlacesTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () => $$PlacesTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$PlacesTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<String> id = const Value.absent(),
                Value<String> kind = const Value.absent(),
                Value<int?> version = const Value.absent(),
                Value<String?> name = const Value.absent(),
                Value<String?> countryCode = const Value.absent(),
                Value<double> centerLon = const Value.absent(),
                Value<double> centerLat = const Value.absent(),
                Value<double?> bboxWest = const Value.absent(),
                Value<double?> bboxSouth = const Value.absent(),
                Value<double?> bboxEast = const Value.absent(),
                Value<double?> bboxNorth = const Value.absent(),
                Value<String?> shape = const Value.absent(),
                Value<String?> address = const Value.absent(),
                Value<String?> admins = const Value.absent(),
                Value<String> tags = const Value.absent(),
                Value<DateTime> updatedAt = const Value.absent(),
                Value<DateTime> accessedAt = const Value.absent(),
                Value<int> rowid = const Value.absent(),
              }) => PlacesCompanion(
                id: id,
                kind: kind,
                version: version,
                name: name,
                countryCode: countryCode,
                centerLon: centerLon,
                centerLat: centerLat,
                bboxWest: bboxWest,
                bboxSouth: bboxSouth,
                bboxEast: bboxEast,
                bboxNorth: bboxNorth,
                shape: shape,
                address: address,
                admins: admins,
                tags: tags,
                updatedAt: updatedAt,
                accessedAt: accessedAt,
                rowid: rowid,
              ),
          createCompanionCallback:
              ({
                required String id,
                required String kind,
                Value<int?> version = const Value.absent(),
                Value<String?> name = const Value.absent(),
                Value<String?> countryCode = const Value.absent(),
                required double centerLon,
                required double centerLat,
                Value<double?> bboxWest = const Value.absent(),
                Value<double?> bboxSouth = const Value.absent(),
                Value<double?> bboxEast = const Value.absent(),
                Value<double?> bboxNorth = const Value.absent(),
                Value<String?> shape = const Value.absent(),
                Value<String?> address = const Value.absent(),
                Value<String?> admins = const Value.absent(),
                required String tags,
                required DateTime updatedAt,
                required DateTime accessedAt,
                Value<int> rowid = const Value.absent(),
              }) => PlacesCompanion.insert(
                id: id,
                kind: kind,
                version: version,
                name: name,
                countryCode: countryCode,
                centerLon: centerLon,
                centerLat: centerLat,
                bboxWest: bboxWest,
                bboxSouth: bboxSouth,
                bboxEast: bboxEast,
                bboxNorth: bboxNorth,
                shape: shape,
                address: address,
                admins: admins,
                tags: tags,
                updatedAt: updatedAt,
                accessedAt: accessedAt,
                rowid: rowid,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$PlacesTable, PlaceRow>(table),
                  BaseReferences<_$CardinalDatabase, $PlacesTable, PlaceRow>(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: null,
        ),
      );
}

typedef $$PlacesTableProcessedTableManager =
    ProcessedTableManager<
      _$CardinalDatabase,
      $PlacesTable,
      PlaceRow,
      $$PlacesTableFilterComposer,
      $$PlacesTableOrderingComposer,
      $$PlacesTableAnnotationComposer,
      $$PlacesTableCreateCompanionBuilder,
      $$PlacesTableUpdateCompanionBuilder,
      (PlaceRow, BaseReferences<_$CardinalDatabase, $PlacesTable, PlaceRow>),
      PlaceRow,
      PrefetchHooks Function()
    >;

class $CardinalDatabaseManager {
  final _$CardinalDatabase _db;
  $CardinalDatabaseManager(this._db);
  $$PlacesTableTableManager get places => $$PlacesTableTableManager(_db, _db.places);
}
