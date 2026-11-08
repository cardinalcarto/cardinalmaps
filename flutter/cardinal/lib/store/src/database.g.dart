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
  static const VerificationMeta _versionMeta = const VerificationMeta(
    'version',
  );
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
  static const VerificationMeta _countryCodeMeta = const VerificationMeta(
    'countryCode',
  );
  @override
  late final GeneratedColumn<String> countryCode = GeneratedColumn<String>(
    'country_code',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _centerLonMeta = const VerificationMeta(
    'centerLon',
  );
  @override
  late final GeneratedColumn<double> centerLon = GeneratedColumn<double>(
    'center_lon',
    aliasedName,
    false,
    type: DriftSqlType.double,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _centerLatMeta = const VerificationMeta(
    'centerLat',
  );
  @override
  late final GeneratedColumn<double> centerLat = GeneratedColumn<double>(
    'center_lat',
    aliasedName,
    false,
    type: DriftSqlType.double,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _bboxWestMeta = const VerificationMeta(
    'bboxWest',
  );
  @override
  late final GeneratedColumn<double> bboxWest = GeneratedColumn<double>(
    'bbox_west',
    aliasedName,
    true,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _bboxSouthMeta = const VerificationMeta(
    'bboxSouth',
  );
  @override
  late final GeneratedColumn<double> bboxSouth = GeneratedColumn<double>(
    'bbox_south',
    aliasedName,
    true,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _bboxEastMeta = const VerificationMeta(
    'bboxEast',
  );
  @override
  late final GeneratedColumn<double> bboxEast = GeneratedColumn<double>(
    'bbox_east',
    aliasedName,
    true,
    type: DriftSqlType.double,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _bboxNorthMeta = const VerificationMeta(
    'bboxNorth',
  );
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
  static const VerificationMeta _addressMeta = const VerificationMeta(
    'address',
  );
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
  static const VerificationMeta _updatedAtMeta = const VerificationMeta(
    'updatedAt',
  );
  @override
  late final GeneratedColumn<DateTime> updatedAt = GeneratedColumn<DateTime>(
    'updated_at',
    aliasedName,
    false,
    type: DriftSqlType.dateTime,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _accessedAtMeta = const VerificationMeta(
    'accessedAt',
  );
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
  VerificationContext validateIntegrity(
    Insertable<PlaceRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('id')) {
      context.handle(_idMeta, id.isAcceptableOrUnknown(data['id']!, _idMeta));
    } else if (isInserting) {
      context.missing(_idMeta);
    }
    if (data.containsKey('kind')) {
      context.handle(
        _kindMeta,
        kind.isAcceptableOrUnknown(data['kind']!, _kindMeta),
      );
    } else if (isInserting) {
      context.missing(_kindMeta);
    }
    if (data.containsKey('version')) {
      context.handle(
        _versionMeta,
        version.isAcceptableOrUnknown(data['version']!, _versionMeta),
      );
    }
    if (data.containsKey('name')) {
      context.handle(
        _nameMeta,
        name.isAcceptableOrUnknown(data['name']!, _nameMeta),
      );
    }
    if (data.containsKey('country_code')) {
      context.handle(
        _countryCodeMeta,
        countryCode.isAcceptableOrUnknown(
          data['country_code']!,
          _countryCodeMeta,
        ),
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
      context.handle(
        _shapeMeta,
        shape.isAcceptableOrUnknown(data['shape']!, _shapeMeta),
      );
    }
    if (data.containsKey('address')) {
      context.handle(
        _addressMeta,
        address.isAcceptableOrUnknown(data['address']!, _addressMeta),
      );
    }
    if (data.containsKey('admins')) {
      context.handle(
        _adminsMeta,
        admins.isAcceptableOrUnknown(data['admins']!, _adminsMeta),
      );
    }
    if (data.containsKey('tags')) {
      context.handle(
        _tagsMeta,
        tags.isAcceptableOrUnknown(data['tags']!, _tagsMeta),
      );
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
      id: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}id'],
      )!,
      kind: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}kind'],
      )!,
      version: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}version'],
      ),
      name: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}name'],
      ),
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
      tags: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}tags'],
      )!,
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
      version: version == null && nullToAbsent
          ? const Value.absent()
          : Value(version),
      name: name == null && nullToAbsent ? const Value.absent() : Value(name),
      countryCode: countryCode == null && nullToAbsent
          ? const Value.absent()
          : Value(countryCode),
      centerLon: Value(centerLon),
      centerLat: Value(centerLat),
      bboxWest: bboxWest == null && nullToAbsent
          ? const Value.absent()
          : Value(bboxWest),
      bboxSouth: bboxSouth == null && nullToAbsent
          ? const Value.absent()
          : Value(bboxSouth),
      bboxEast: bboxEast == null && nullToAbsent
          ? const Value.absent()
          : Value(bboxEast),
      bboxNorth: bboxNorth == null && nullToAbsent
          ? const Value.absent()
          : Value(bboxNorth),
      shape: shape == null && nullToAbsent
          ? const Value.absent()
          : Value(shape),
      address: address == null && nullToAbsent
          ? const Value.absent()
          : Value(address),
      admins: admins == null && nullToAbsent
          ? const Value.absent()
          : Value(admins),
      tags: Value(tags),
      updatedAt: Value(updatedAt),
      accessedAt: Value(accessedAt),
    );
  }

  factory PlaceRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
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
      countryCode: data.countryCode.present
          ? data.countryCode.value
          : this.countryCode,
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
      accessedAt: data.accessedAt.present
          ? data.accessedAt.value
          : this.accessedAt,
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

class $SavedListsTable extends SavedLists
    with TableInfo<$SavedListsTable, SavedListRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $SavedListsTable(this.attachedDatabase, [this._alias]);
  static const VerificationMeta _idMeta = const VerificationMeta('id');
  @override
  late final GeneratedColumn<int> id = GeneratedColumn<int>(
    'id',
    aliasedName,
    false,
    hasAutoIncrement: true,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'PRIMARY KEY AUTOINCREMENT',
    ),
  );
  static const VerificationMeta _parentIdMeta = const VerificationMeta(
    'parentId',
  );
  @override
  late final GeneratedColumn<int> parentId = GeneratedColumn<int>(
    'parent_id',
    aliasedName,
    true,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'REFERENCES saved_lists (id) ON DELETE CASCADE',
    ),
  );
  static const VerificationMeta _nameMeta = const VerificationMeta('name');
  @override
  late final GeneratedColumn<String> name = GeneratedColumn<String>(
    'name',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _noteMeta = const VerificationMeta('note');
  @override
  late final GeneratedColumn<String> note = GeneratedColumn<String>(
    'note',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _positionMeta = const VerificationMeta(
    'position',
  );
  @override
  late final GeneratedColumn<int> position = GeneratedColumn<int>(
    'position',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _shownOnMapMeta = const VerificationMeta(
    'shownOnMap',
  );
  @override
  late final GeneratedColumn<bool> shownOnMap = GeneratedColumn<bool>(
    'shown_on_map',
    aliasedName,
    false,
    type: DriftSqlType.bool,
    requiredDuringInsert: false,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'CHECK ("shown_on_map" IN (0, 1))',
    ),
    defaultValue: const Constant(false),
  );
  @override
  List<GeneratedColumn> get $columns => [
    id,
    parentId,
    name,
    note,
    position,
    shownOnMap,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'saved_lists';
  @override
  VerificationContext validateIntegrity(
    Insertable<SavedListRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('id')) {
      context.handle(_idMeta, id.isAcceptableOrUnknown(data['id']!, _idMeta));
    }
    if (data.containsKey('parent_id')) {
      context.handle(
        _parentIdMeta,
        parentId.isAcceptableOrUnknown(data['parent_id']!, _parentIdMeta),
      );
    }
    if (data.containsKey('name')) {
      context.handle(
        _nameMeta,
        name.isAcceptableOrUnknown(data['name']!, _nameMeta),
      );
    } else if (isInserting) {
      context.missing(_nameMeta);
    }
    if (data.containsKey('note')) {
      context.handle(
        _noteMeta,
        note.isAcceptableOrUnknown(data['note']!, _noteMeta),
      );
    }
    if (data.containsKey('position')) {
      context.handle(
        _positionMeta,
        position.isAcceptableOrUnknown(data['position']!, _positionMeta),
      );
    } else if (isInserting) {
      context.missing(_positionMeta);
    }
    if (data.containsKey('shown_on_map')) {
      context.handle(
        _shownOnMapMeta,
        shownOnMap.isAcceptableOrUnknown(
          data['shown_on_map']!,
          _shownOnMapMeta,
        ),
      );
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {id};
  @override
  SavedListRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return SavedListRow(
      id: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}id'],
      )!,
      parentId: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}parent_id'],
      ),
      name: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}name'],
      )!,
      note: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}note'],
      ),
      position: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}position'],
      )!,
      shownOnMap: attachedDatabase.typeMapping.read(
        DriftSqlType.bool,
        data['${effectivePrefix}shown_on_map'],
      )!,
    );
  }

  @override
  $SavedListsTable createAlias(String alias) {
    return $SavedListsTable(attachedDatabase, alias);
  }
}

class SavedListRow extends DataClass implements Insertable<SavedListRow> {
  final int id;

  /// The list it's in; null at the top level.
  final int? parentId;
  final String name;
  final String? note;

  /// Its place among the lists with the same parent, lowest first.
  final int position;

  /// Whether it's on in the map's Saved Places layer, for as long as it's empty; after
  /// that, it's on as much as what's in it is.
  final bool shownOnMap;
  const SavedListRow({
    required this.id,
    this.parentId,
    required this.name,
    this.note,
    required this.position,
    required this.shownOnMap,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['id'] = Variable<int>(id);
    if (!nullToAbsent || parentId != null) {
      map['parent_id'] = Variable<int>(parentId);
    }
    map['name'] = Variable<String>(name);
    if (!nullToAbsent || note != null) {
      map['note'] = Variable<String>(note);
    }
    map['position'] = Variable<int>(position);
    map['shown_on_map'] = Variable<bool>(shownOnMap);
    return map;
  }

  SavedListsCompanion toCompanion(bool nullToAbsent) {
    return SavedListsCompanion(
      id: Value(id),
      parentId: parentId == null && nullToAbsent
          ? const Value.absent()
          : Value(parentId),
      name: Value(name),
      note: note == null && nullToAbsent ? const Value.absent() : Value(note),
      position: Value(position),
      shownOnMap: Value(shownOnMap),
    );
  }

  factory SavedListRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return SavedListRow(
      id: serializer.fromJson<int>(json['id']),
      parentId: serializer.fromJson<int?>(json['parentId']),
      name: serializer.fromJson<String>(json['name']),
      note: serializer.fromJson<String?>(json['note']),
      position: serializer.fromJson<int>(json['position']),
      shownOnMap: serializer.fromJson<bool>(json['shownOnMap']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'id': serializer.toJson<int>(id),
      'parentId': serializer.toJson<int?>(parentId),
      'name': serializer.toJson<String>(name),
      'note': serializer.toJson<String?>(note),
      'position': serializer.toJson<int>(position),
      'shownOnMap': serializer.toJson<bool>(shownOnMap),
    };
  }

  SavedListRow copyWith({
    int? id,
    Value<int?> parentId = const Value.absent(),
    String? name,
    Value<String?> note = const Value.absent(),
    int? position,
    bool? shownOnMap,
  }) => SavedListRow(
    id: id ?? this.id,
    parentId: parentId.present ? parentId.value : this.parentId,
    name: name ?? this.name,
    note: note.present ? note.value : this.note,
    position: position ?? this.position,
    shownOnMap: shownOnMap ?? this.shownOnMap,
  );
  SavedListRow copyWithCompanion(SavedListsCompanion data) {
    return SavedListRow(
      id: data.id.present ? data.id.value : this.id,
      parentId: data.parentId.present ? data.parentId.value : this.parentId,
      name: data.name.present ? data.name.value : this.name,
      note: data.note.present ? data.note.value : this.note,
      position: data.position.present ? data.position.value : this.position,
      shownOnMap: data.shownOnMap.present
          ? data.shownOnMap.value
          : this.shownOnMap,
    );
  }

  @override
  String toString() {
    return (StringBuffer('SavedListRow(')
          ..write('id: $id, ')
          ..write('parentId: $parentId, ')
          ..write('name: $name, ')
          ..write('note: $note, ')
          ..write('position: $position, ')
          ..write('shownOnMap: $shownOnMap')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode =>
      Object.hash(id, parentId, name, note, position, shownOnMap);
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is SavedListRow &&
          other.id == this.id &&
          other.parentId == this.parentId &&
          other.name == this.name &&
          other.note == this.note &&
          other.position == this.position &&
          other.shownOnMap == this.shownOnMap);
}

class SavedListsCompanion extends UpdateCompanion<SavedListRow> {
  final Value<int> id;
  final Value<int?> parentId;
  final Value<String> name;
  final Value<String?> note;
  final Value<int> position;
  final Value<bool> shownOnMap;
  const SavedListsCompanion({
    this.id = const Value.absent(),
    this.parentId = const Value.absent(),
    this.name = const Value.absent(),
    this.note = const Value.absent(),
    this.position = const Value.absent(),
    this.shownOnMap = const Value.absent(),
  });
  SavedListsCompanion.insert({
    this.id = const Value.absent(),
    this.parentId = const Value.absent(),
    required String name,
    this.note = const Value.absent(),
    required int position,
    this.shownOnMap = const Value.absent(),
  }) : name = Value(name),
       position = Value(position);
  static Insertable<SavedListRow> custom({
    Expression<int>? id,
    Expression<int>? parentId,
    Expression<String>? name,
    Expression<String>? note,
    Expression<int>? position,
    Expression<bool>? shownOnMap,
  }) {
    return RawValuesInsertable({
      if (id != null) 'id': id,
      if (parentId != null) 'parent_id': parentId,
      if (name != null) 'name': name,
      if (note != null) 'note': note,
      if (position != null) 'position': position,
      if (shownOnMap != null) 'shown_on_map': shownOnMap,
    });
  }

  SavedListsCompanion copyWith({
    Value<int>? id,
    Value<int?>? parentId,
    Value<String>? name,
    Value<String?>? note,
    Value<int>? position,
    Value<bool>? shownOnMap,
  }) {
    return SavedListsCompanion(
      id: id ?? this.id,
      parentId: parentId ?? this.parentId,
      name: name ?? this.name,
      note: note ?? this.note,
      position: position ?? this.position,
      shownOnMap: shownOnMap ?? this.shownOnMap,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (id.present) {
      map['id'] = Variable<int>(id.value);
    }
    if (parentId.present) {
      map['parent_id'] = Variable<int>(parentId.value);
    }
    if (name.present) {
      map['name'] = Variable<String>(name.value);
    }
    if (note.present) {
      map['note'] = Variable<String>(note.value);
    }
    if (position.present) {
      map['position'] = Variable<int>(position.value);
    }
    if (shownOnMap.present) {
      map['shown_on_map'] = Variable<bool>(shownOnMap.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('SavedListsCompanion(')
          ..write('id: $id, ')
          ..write('parentId: $parentId, ')
          ..write('name: $name, ')
          ..write('note: $note, ')
          ..write('position: $position, ')
          ..write('shownOnMap: $shownOnMap')
          ..write(')'))
        .toString();
  }
}

class $SavedPlacesTable extends SavedPlaces
    with TableInfo<$SavedPlacesTable, SavedPlaceRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $SavedPlacesTable(this.attachedDatabase, [this._alias]);
  static const VerificationMeta _idMeta = const VerificationMeta('id');
  @override
  late final GeneratedColumn<int> id = GeneratedColumn<int>(
    'id',
    aliasedName,
    false,
    hasAutoIncrement: true,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'PRIMARY KEY AUTOINCREMENT',
    ),
  );
  static const VerificationMeta _placeIdMeta = const VerificationMeta(
    'placeId',
  );
  @override
  late final GeneratedColumn<String> placeId = GeneratedColumn<String>(
    'place_id',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'REFERENCES places (id)',
    ),
  );
  static const VerificationMeta _listIdMeta = const VerificationMeta('listId');
  @override
  late final GeneratedColumn<int> listId = GeneratedColumn<int>(
    'list_id',
    aliasedName,
    true,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'REFERENCES saved_lists (id) ON DELETE CASCADE',
    ),
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
  static const VerificationMeta _noteMeta = const VerificationMeta('note');
  @override
  late final GeneratedColumn<String> note = GeneratedColumn<String>(
    'note',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _pinnedMeta = const VerificationMeta('pinned');
  @override
  late final GeneratedColumn<bool> pinned = GeneratedColumn<bool>(
    'pinned',
    aliasedName,
    false,
    type: DriftSqlType.bool,
    requiredDuringInsert: false,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'CHECK ("pinned" IN (0, 1))',
    ),
    defaultValue: const Constant(false),
  );
  static const VerificationMeta _positionMeta = const VerificationMeta(
    'position',
  );
  @override
  late final GeneratedColumn<int> position = GeneratedColumn<int>(
    'position',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _savedAtMeta = const VerificationMeta(
    'savedAt',
  );
  @override
  late final GeneratedColumn<DateTime> savedAt = GeneratedColumn<DateTime>(
    'saved_at',
    aliasedName,
    false,
    type: DriftSqlType.dateTime,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _emojiMeta = const VerificationMeta('emoji');
  @override
  late final GeneratedColumn<String> emoji = GeneratedColumn<String>(
    'emoji',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _shownOnMapMeta = const VerificationMeta(
    'shownOnMap',
  );
  @override
  late final GeneratedColumn<bool> shownOnMap = GeneratedColumn<bool>(
    'shown_on_map',
    aliasedName,
    false,
    type: DriftSqlType.bool,
    requiredDuringInsert: false,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'CHECK ("shown_on_map" IN (0, 1))',
    ),
    defaultValue: const Constant(false),
  );
  @override
  List<GeneratedColumn> get $columns => [
    id,
    placeId,
    listId,
    name,
    note,
    pinned,
    position,
    savedAt,
    emoji,
    shownOnMap,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'saved_places';
  @override
  VerificationContext validateIntegrity(
    Insertable<SavedPlaceRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('id')) {
      context.handle(_idMeta, id.isAcceptableOrUnknown(data['id']!, _idMeta));
    }
    if (data.containsKey('place_id')) {
      context.handle(
        _placeIdMeta,
        placeId.isAcceptableOrUnknown(data['place_id']!, _placeIdMeta),
      );
    } else if (isInserting) {
      context.missing(_placeIdMeta);
    }
    if (data.containsKey('list_id')) {
      context.handle(
        _listIdMeta,
        listId.isAcceptableOrUnknown(data['list_id']!, _listIdMeta),
      );
    }
    if (data.containsKey('name')) {
      context.handle(
        _nameMeta,
        name.isAcceptableOrUnknown(data['name']!, _nameMeta),
      );
    }
    if (data.containsKey('note')) {
      context.handle(
        _noteMeta,
        note.isAcceptableOrUnknown(data['note']!, _noteMeta),
      );
    }
    if (data.containsKey('pinned')) {
      context.handle(
        _pinnedMeta,
        pinned.isAcceptableOrUnknown(data['pinned']!, _pinnedMeta),
      );
    }
    if (data.containsKey('position')) {
      context.handle(
        _positionMeta,
        position.isAcceptableOrUnknown(data['position']!, _positionMeta),
      );
    } else if (isInserting) {
      context.missing(_positionMeta);
    }
    if (data.containsKey('saved_at')) {
      context.handle(
        _savedAtMeta,
        savedAt.isAcceptableOrUnknown(data['saved_at']!, _savedAtMeta),
      );
    } else if (isInserting) {
      context.missing(_savedAtMeta);
    }
    if (data.containsKey('emoji')) {
      context.handle(
        _emojiMeta,
        emoji.isAcceptableOrUnknown(data['emoji']!, _emojiMeta),
      );
    }
    if (data.containsKey('shown_on_map')) {
      context.handle(
        _shownOnMapMeta,
        shownOnMap.isAcceptableOrUnknown(
          data['shown_on_map']!,
          _shownOnMapMeta,
        ),
      );
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {id};
  @override
  SavedPlaceRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return SavedPlaceRow(
      id: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}id'],
      )!,
      placeId: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}place_id'],
      )!,
      listId: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}list_id'],
      ),
      name: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}name'],
      ),
      note: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}note'],
      ),
      pinned: attachedDatabase.typeMapping.read(
        DriftSqlType.bool,
        data['${effectivePrefix}pinned'],
      )!,
      position: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}position'],
      )!,
      savedAt: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}saved_at'],
      )!,
      emoji: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}emoji'],
      ),
      shownOnMap: attachedDatabase.typeMapping.read(
        DriftSqlType.bool,
        data['${effectivePrefix}shown_on_map'],
      )!,
    );
  }

  @override
  $SavedPlacesTable createAlias(String alias) {
    return $SavedPlacesTable(attachedDatabase, alias);
  }
}

class SavedPlaceRow extends DataClass implements Insertable<SavedPlaceRow> {
  final int id;
  final String placeId;

  /// The list it's in; null at the top level.
  final int? listId;

  /// The user's name for it, instead of the place's own.
  final String? name;
  final String? note;

  /// Whether this copy has a chip on the home sheet.
  final bool pinned;

  /// Its place among the places in the same list, lowest first.
  final int position;
  final DateTime savedAt;

  /// The emoji it's marked with on the map, one grapheme.
  final String? emoji;

  /// Whether this copy is on in the map's Saved Places layer.
  final bool shownOnMap;
  const SavedPlaceRow({
    required this.id,
    required this.placeId,
    this.listId,
    this.name,
    this.note,
    required this.pinned,
    required this.position,
    required this.savedAt,
    this.emoji,
    required this.shownOnMap,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['id'] = Variable<int>(id);
    map['place_id'] = Variable<String>(placeId);
    if (!nullToAbsent || listId != null) {
      map['list_id'] = Variable<int>(listId);
    }
    if (!nullToAbsent || name != null) {
      map['name'] = Variable<String>(name);
    }
    if (!nullToAbsent || note != null) {
      map['note'] = Variable<String>(note);
    }
    map['pinned'] = Variable<bool>(pinned);
    map['position'] = Variable<int>(position);
    map['saved_at'] = Variable<DateTime>(savedAt);
    if (!nullToAbsent || emoji != null) {
      map['emoji'] = Variable<String>(emoji);
    }
    map['shown_on_map'] = Variable<bool>(shownOnMap);
    return map;
  }

  SavedPlacesCompanion toCompanion(bool nullToAbsent) {
    return SavedPlacesCompanion(
      id: Value(id),
      placeId: Value(placeId),
      listId: listId == null && nullToAbsent
          ? const Value.absent()
          : Value(listId),
      name: name == null && nullToAbsent ? const Value.absent() : Value(name),
      note: note == null && nullToAbsent ? const Value.absent() : Value(note),
      pinned: Value(pinned),
      position: Value(position),
      savedAt: Value(savedAt),
      emoji: emoji == null && nullToAbsent
          ? const Value.absent()
          : Value(emoji),
      shownOnMap: Value(shownOnMap),
    );
  }

  factory SavedPlaceRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return SavedPlaceRow(
      id: serializer.fromJson<int>(json['id']),
      placeId: serializer.fromJson<String>(json['placeId']),
      listId: serializer.fromJson<int?>(json['listId']),
      name: serializer.fromJson<String?>(json['name']),
      note: serializer.fromJson<String?>(json['note']),
      pinned: serializer.fromJson<bool>(json['pinned']),
      position: serializer.fromJson<int>(json['position']),
      savedAt: serializer.fromJson<DateTime>(json['savedAt']),
      emoji: serializer.fromJson<String?>(json['emoji']),
      shownOnMap: serializer.fromJson<bool>(json['shownOnMap']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'id': serializer.toJson<int>(id),
      'placeId': serializer.toJson<String>(placeId),
      'listId': serializer.toJson<int?>(listId),
      'name': serializer.toJson<String?>(name),
      'note': serializer.toJson<String?>(note),
      'pinned': serializer.toJson<bool>(pinned),
      'position': serializer.toJson<int>(position),
      'savedAt': serializer.toJson<DateTime>(savedAt),
      'emoji': serializer.toJson<String?>(emoji),
      'shownOnMap': serializer.toJson<bool>(shownOnMap),
    };
  }

  SavedPlaceRow copyWith({
    int? id,
    String? placeId,
    Value<int?> listId = const Value.absent(),
    Value<String?> name = const Value.absent(),
    Value<String?> note = const Value.absent(),
    bool? pinned,
    int? position,
    DateTime? savedAt,
    Value<String?> emoji = const Value.absent(),
    bool? shownOnMap,
  }) => SavedPlaceRow(
    id: id ?? this.id,
    placeId: placeId ?? this.placeId,
    listId: listId.present ? listId.value : this.listId,
    name: name.present ? name.value : this.name,
    note: note.present ? note.value : this.note,
    pinned: pinned ?? this.pinned,
    position: position ?? this.position,
    savedAt: savedAt ?? this.savedAt,
    emoji: emoji.present ? emoji.value : this.emoji,
    shownOnMap: shownOnMap ?? this.shownOnMap,
  );
  SavedPlaceRow copyWithCompanion(SavedPlacesCompanion data) {
    return SavedPlaceRow(
      id: data.id.present ? data.id.value : this.id,
      placeId: data.placeId.present ? data.placeId.value : this.placeId,
      listId: data.listId.present ? data.listId.value : this.listId,
      name: data.name.present ? data.name.value : this.name,
      note: data.note.present ? data.note.value : this.note,
      pinned: data.pinned.present ? data.pinned.value : this.pinned,
      position: data.position.present ? data.position.value : this.position,
      savedAt: data.savedAt.present ? data.savedAt.value : this.savedAt,
      emoji: data.emoji.present ? data.emoji.value : this.emoji,
      shownOnMap: data.shownOnMap.present
          ? data.shownOnMap.value
          : this.shownOnMap,
    );
  }

  @override
  String toString() {
    return (StringBuffer('SavedPlaceRow(')
          ..write('id: $id, ')
          ..write('placeId: $placeId, ')
          ..write('listId: $listId, ')
          ..write('name: $name, ')
          ..write('note: $note, ')
          ..write('pinned: $pinned, ')
          ..write('position: $position, ')
          ..write('savedAt: $savedAt, ')
          ..write('emoji: $emoji, ')
          ..write('shownOnMap: $shownOnMap')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(
    id,
    placeId,
    listId,
    name,
    note,
    pinned,
    position,
    savedAt,
    emoji,
    shownOnMap,
  );
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is SavedPlaceRow &&
          other.id == this.id &&
          other.placeId == this.placeId &&
          other.listId == this.listId &&
          other.name == this.name &&
          other.note == this.note &&
          other.pinned == this.pinned &&
          other.position == this.position &&
          other.savedAt == this.savedAt &&
          other.emoji == this.emoji &&
          other.shownOnMap == this.shownOnMap);
}

class SavedPlacesCompanion extends UpdateCompanion<SavedPlaceRow> {
  final Value<int> id;
  final Value<String> placeId;
  final Value<int?> listId;
  final Value<String?> name;
  final Value<String?> note;
  final Value<bool> pinned;
  final Value<int> position;
  final Value<DateTime> savedAt;
  final Value<String?> emoji;
  final Value<bool> shownOnMap;
  const SavedPlacesCompanion({
    this.id = const Value.absent(),
    this.placeId = const Value.absent(),
    this.listId = const Value.absent(),
    this.name = const Value.absent(),
    this.note = const Value.absent(),
    this.pinned = const Value.absent(),
    this.position = const Value.absent(),
    this.savedAt = const Value.absent(),
    this.emoji = const Value.absent(),
    this.shownOnMap = const Value.absent(),
  });
  SavedPlacesCompanion.insert({
    this.id = const Value.absent(),
    required String placeId,
    this.listId = const Value.absent(),
    this.name = const Value.absent(),
    this.note = const Value.absent(),
    this.pinned = const Value.absent(),
    required int position,
    required DateTime savedAt,
    this.emoji = const Value.absent(),
    this.shownOnMap = const Value.absent(),
  }) : placeId = Value(placeId),
       position = Value(position),
       savedAt = Value(savedAt);
  static Insertable<SavedPlaceRow> custom({
    Expression<int>? id,
    Expression<String>? placeId,
    Expression<int>? listId,
    Expression<String>? name,
    Expression<String>? note,
    Expression<bool>? pinned,
    Expression<int>? position,
    Expression<DateTime>? savedAt,
    Expression<String>? emoji,
    Expression<bool>? shownOnMap,
  }) {
    return RawValuesInsertable({
      if (id != null) 'id': id,
      if (placeId != null) 'place_id': placeId,
      if (listId != null) 'list_id': listId,
      if (name != null) 'name': name,
      if (note != null) 'note': note,
      if (pinned != null) 'pinned': pinned,
      if (position != null) 'position': position,
      if (savedAt != null) 'saved_at': savedAt,
      if (emoji != null) 'emoji': emoji,
      if (shownOnMap != null) 'shown_on_map': shownOnMap,
    });
  }

  SavedPlacesCompanion copyWith({
    Value<int>? id,
    Value<String>? placeId,
    Value<int?>? listId,
    Value<String?>? name,
    Value<String?>? note,
    Value<bool>? pinned,
    Value<int>? position,
    Value<DateTime>? savedAt,
    Value<String?>? emoji,
    Value<bool>? shownOnMap,
  }) {
    return SavedPlacesCompanion(
      id: id ?? this.id,
      placeId: placeId ?? this.placeId,
      listId: listId ?? this.listId,
      name: name ?? this.name,
      note: note ?? this.note,
      pinned: pinned ?? this.pinned,
      position: position ?? this.position,
      savedAt: savedAt ?? this.savedAt,
      emoji: emoji ?? this.emoji,
      shownOnMap: shownOnMap ?? this.shownOnMap,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (id.present) {
      map['id'] = Variable<int>(id.value);
    }
    if (placeId.present) {
      map['place_id'] = Variable<String>(placeId.value);
    }
    if (listId.present) {
      map['list_id'] = Variable<int>(listId.value);
    }
    if (name.present) {
      map['name'] = Variable<String>(name.value);
    }
    if (note.present) {
      map['note'] = Variable<String>(note.value);
    }
    if (pinned.present) {
      map['pinned'] = Variable<bool>(pinned.value);
    }
    if (position.present) {
      map['position'] = Variable<int>(position.value);
    }
    if (savedAt.present) {
      map['saved_at'] = Variable<DateTime>(savedAt.value);
    }
    if (emoji.present) {
      map['emoji'] = Variable<String>(emoji.value);
    }
    if (shownOnMap.present) {
      map['shown_on_map'] = Variable<bool>(shownOnMap.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('SavedPlacesCompanion(')
          ..write('id: $id, ')
          ..write('placeId: $placeId, ')
          ..write('listId: $listId, ')
          ..write('name: $name, ')
          ..write('note: $note, ')
          ..write('pinned: $pinned, ')
          ..write('position: $position, ')
          ..write('savedAt: $savedAt, ')
          ..write('emoji: $emoji, ')
          ..write('shownOnMap: $shownOnMap')
          ..write(')'))
        .toString();
  }
}

class $SettingsTable extends Settings
    with TableInfo<$SettingsTable, SettingRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $SettingsTable(this.attachedDatabase, [this._alias]);
  static const VerificationMeta _keyMeta = const VerificationMeta('key');
  @override
  late final GeneratedColumn<String> key = GeneratedColumn<String>(
    'key',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _valueMeta = const VerificationMeta('value');
  @override
  late final GeneratedColumn<String> value = GeneratedColumn<String>(
    'value',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  @override
  List<GeneratedColumn> get $columns => [key, value];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'settings';
  @override
  VerificationContext validateIntegrity(
    Insertable<SettingRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('key')) {
      context.handle(
        _keyMeta,
        key.isAcceptableOrUnknown(data['key']!, _keyMeta),
      );
    } else if (isInserting) {
      context.missing(_keyMeta);
    }
    if (data.containsKey('value')) {
      context.handle(
        _valueMeta,
        value.isAcceptableOrUnknown(data['value']!, _valueMeta),
      );
    } else if (isInserting) {
      context.missing(_valueMeta);
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {key};
  @override
  SettingRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return SettingRow(
      key: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}key'],
      )!,
      value: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}value'],
      )!,
    );
  }

  @override
  $SettingsTable createAlias(String alias) {
    return $SettingsTable(attachedDatabase, alias);
  }
}

class SettingRow extends DataClass implements Insertable<SettingRow> {
  final String key;

  /// The value as JSON.
  final String value;
  const SettingRow({required this.key, required this.value});
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['key'] = Variable<String>(key);
    map['value'] = Variable<String>(value);
    return map;
  }

  SettingsCompanion toCompanion(bool nullToAbsent) {
    return SettingsCompanion(key: Value(key), value: Value(value));
  }

  factory SettingRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return SettingRow(
      key: serializer.fromJson<String>(json['key']),
      value: serializer.fromJson<String>(json['value']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'key': serializer.toJson<String>(key),
      'value': serializer.toJson<String>(value),
    };
  }

  SettingRow copyWith({String? key, String? value}) =>
      SettingRow(key: key ?? this.key, value: value ?? this.value);
  SettingRow copyWithCompanion(SettingsCompanion data) {
    return SettingRow(
      key: data.key.present ? data.key.value : this.key,
      value: data.value.present ? data.value.value : this.value,
    );
  }

  @override
  String toString() {
    return (StringBuffer('SettingRow(')
          ..write('key: $key, ')
          ..write('value: $value')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(key, value);
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is SettingRow &&
          other.key == this.key &&
          other.value == this.value);
}

class SettingsCompanion extends UpdateCompanion<SettingRow> {
  final Value<String> key;
  final Value<String> value;
  final Value<int> rowid;
  const SettingsCompanion({
    this.key = const Value.absent(),
    this.value = const Value.absent(),
    this.rowid = const Value.absent(),
  });
  SettingsCompanion.insert({
    required String key,
    required String value,
    this.rowid = const Value.absent(),
  }) : key = Value(key),
       value = Value(value);
  static Insertable<SettingRow> custom({
    Expression<String>? key,
    Expression<String>? value,
    Expression<int>? rowid,
  }) {
    return RawValuesInsertable({
      if (key != null) 'key': key,
      if (value != null) 'value': value,
      if (rowid != null) 'rowid': rowid,
    });
  }

  SettingsCompanion copyWith({
    Value<String>? key,
    Value<String>? value,
    Value<int>? rowid,
  }) {
    return SettingsCompanion(
      key: key ?? this.key,
      value: value ?? this.value,
      rowid: rowid ?? this.rowid,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (key.present) {
      map['key'] = Variable<String>(key.value);
    }
    if (value.present) {
      map['value'] = Variable<String>(value.value);
    }
    if (rowid.present) {
      map['rowid'] = Variable<int>(rowid.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('SettingsCompanion(')
          ..write('key: $key, ')
          ..write('value: $value, ')
          ..write('rowid: $rowid')
          ..write(')'))
        .toString();
  }
}

abstract class _$CardinalDatabase extends GeneratedDatabase {
  _$CardinalDatabase(QueryExecutor e) : super(e);
  $CardinalDatabaseManager get managers => $CardinalDatabaseManager(this);
  late final $PlacesTable places = $PlacesTable(this);
  late final $SavedListsTable savedLists = $SavedListsTable(this);
  late final $SavedPlacesTable savedPlaces = $SavedPlacesTable(this);
  late final Index savedPlacesOncePerList = Index(
    'saved_places_once_per_list',
    'CREATE UNIQUE INDEX saved_places_once_per_list ON saved_places (place_id, list_id)',
  );
  late final Index savedPlacesOnceAtTop = Index(
    'saved_places_once_at_top',
    'CREATE UNIQUE INDEX saved_places_once_at_top ON saved_places (place_id) WHERE list_id IS NULL',
  );
  late final Trigger savedListsNoCycleOnUpdate = Trigger(
    'CREATE TRIGGER saved_lists_no_cycle_on_update BEFORE UPDATE OF parent_id ON saved_lists WHEN NEW.parent_id IS NOT NULL BEGIN SELECT RAISE (ABORT, \'a saved list cannot be inside itself\') WHERE EXISTS (WITH RECURSIVE up (id) AS (SELECT NEW.parent_id UNION SELECT l.parent_id FROM saved_lists AS l JOIN up ON l.id = up.id WHERE l.parent_id IS NOT NULL) SELECT 1 FROM up WHERE up.id = NEW.id);END',
    'saved_lists_no_cycle_on_update',
  );
  late final Trigger savedListsNoCycleOnInsert = Trigger(
    'CREATE TRIGGER saved_lists_no_cycle_on_insert BEFORE INSERT ON saved_lists WHEN NEW.parent_id IS NOT NULL AND NEW.parent_id = NEW.id BEGIN SELECT RAISE (ABORT, \'a saved list cannot be inside itself\');END',
    'saved_lists_no_cycle_on_insert',
  );
  late final $SettingsTable settings = $SettingsTable(this);
  @override
  Iterable<TableInfo<Table, Object?>> get allTables =>
      allSchemaEntities.whereType<TableInfo<Table, Object?>>();
  @override
  List<DatabaseSchemaEntity> get allSchemaEntities => [
    places,
    savedLists,
    savedPlaces,
    savedPlacesOncePerList,
    savedPlacesOnceAtTop,
    savedListsNoCycleOnUpdate,
    savedListsNoCycleOnInsert,
    settings,
  ];
  @override
  StreamQueryUpdateRules get streamUpdateRules => const StreamQueryUpdateRules([
    WritePropagation(
      on: TableUpdateQuery.onTableName(
        'saved_lists',
        limitUpdateKind: UpdateKind.delete,
      ),
      result: [TableUpdate('saved_lists', kind: UpdateKind.delete)],
    ),
    WritePropagation(
      on: TableUpdateQuery.onTableName(
        'saved_lists',
        limitUpdateKind: UpdateKind.delete,
      ),
      result: [TableUpdate('saved_places', kind: UpdateKind.delete)],
    ),
    WritePropagation(
      on: TableUpdateQuery.onTableName(
        'saved_lists',
        limitUpdateKind: UpdateKind.update,
      ),
      result: [],
    ),
    WritePropagation(
      on: TableUpdateQuery.onTableName(
        'saved_lists',
        limitUpdateKind: UpdateKind.insert,
      ),
      result: [],
    ),
  ]);
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

final class $$PlacesTableReferences
    extends BaseReferences<_$CardinalDatabase, $PlacesTable, PlaceRow> {
  $$PlacesTableReferences(super.$_db, super.$_table, super.$_typedResult);

  static MultiTypedResultKey<$SavedPlacesTable, List<SavedPlaceRow>>
  _savedPlacesRefsTable(_$CardinalDatabase db) => MultiTypedResultKey.fromTable(
    db.savedPlaces,
    aliasName: 'places__id__saved_places__place_id',
  );

  $$SavedPlacesTableProcessedTableManager get savedPlacesRefs {
    final manager = $$SavedPlacesTableTableManager(
      $_db,
      $_db.savedPlaces,
    ).filter((f) => f.placeId.id.sqlEquals($_itemColumn<String>('id')!));

    final cache = $_typedResult.readTableOrNull(_savedPlacesRefsTable($_db));
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: cache),
    );
  }
}

class $$PlacesTableFilterComposer
    extends Composer<_$CardinalDatabase, $PlacesTable> {
  $$PlacesTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnFilters<String> get id => $composableBuilder(
    column: $table.id,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get kind => $composableBuilder(
    column: $table.kind,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get version => $composableBuilder(
    column: $table.version,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get countryCode => $composableBuilder(
    column: $table.countryCode,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get centerLon => $composableBuilder(
    column: $table.centerLon,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get centerLat => $composableBuilder(
    column: $table.centerLat,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get bboxWest => $composableBuilder(
    column: $table.bboxWest,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get bboxSouth => $composableBuilder(
    column: $table.bboxSouth,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get bboxEast => $composableBuilder(
    column: $table.bboxEast,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<double> get bboxNorth => $composableBuilder(
    column: $table.bboxNorth,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get shape => $composableBuilder(
    column: $table.shape,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get address => $composableBuilder(
    column: $table.address,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get admins => $composableBuilder(
    column: $table.admins,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get tags => $composableBuilder(
    column: $table.tags,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get updatedAt => $composableBuilder(
    column: $table.updatedAt,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get accessedAt => $composableBuilder(
    column: $table.accessedAt,
    builder: (column) => ColumnFilters(column),
  );

  Expression<bool> savedPlacesRefs(
    Expression<bool> Function($$SavedPlacesTableFilterComposer f) f,
  ) {
    final $$SavedPlacesTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.savedPlaces,
      getReferencedColumn: (t) => t.placeId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedPlacesTableFilterComposer(
            $db: $db,
            $table: $db.savedPlaces,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }
}

class $$PlacesTableOrderingComposer
    extends Composer<_$CardinalDatabase, $PlacesTable> {
  $$PlacesTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<String> get id => $composableBuilder(
    column: $table.id,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get kind => $composableBuilder(
    column: $table.kind,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get version => $composableBuilder(
    column: $table.version,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get countryCode => $composableBuilder(
    column: $table.countryCode,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get centerLon => $composableBuilder(
    column: $table.centerLon,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get centerLat => $composableBuilder(
    column: $table.centerLat,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get bboxWest => $composableBuilder(
    column: $table.bboxWest,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get bboxSouth => $composableBuilder(
    column: $table.bboxSouth,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get bboxEast => $composableBuilder(
    column: $table.bboxEast,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<double> get bboxNorth => $composableBuilder(
    column: $table.bboxNorth,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get shape => $composableBuilder(
    column: $table.shape,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get address => $composableBuilder(
    column: $table.address,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get admins => $composableBuilder(
    column: $table.admins,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get tags => $composableBuilder(
    column: $table.tags,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get updatedAt => $composableBuilder(
    column: $table.updatedAt,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get accessedAt => $composableBuilder(
    column: $table.accessedAt,
    builder: (column) => ColumnOrderings(column),
  );
}

class $$PlacesTableAnnotationComposer
    extends Composer<_$CardinalDatabase, $PlacesTable> {
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

  GeneratedColumn<String> get countryCode => $composableBuilder(
    column: $table.countryCode,
    builder: (column) => column,
  );

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

  GeneratedColumn<DateTime> get accessedAt => $composableBuilder(
    column: $table.accessedAt,
    builder: (column) => column,
  );

  Expression<T> savedPlacesRefs<T extends Object>(
    Expression<T> Function($$SavedPlacesTableAnnotationComposer a) f,
  ) {
    final $$SavedPlacesTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.savedPlaces,
      getReferencedColumn: (t) => t.placeId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedPlacesTableAnnotationComposer(
            $db: $db,
            $table: $db.savedPlaces,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }
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
          (PlaceRow, $$PlacesTableReferences),
          PlaceRow,
          PrefetchHooks Function({bool savedPlacesRefs})
        > {
  $$PlacesTableTableManager(_$CardinalDatabase db, $PlacesTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$PlacesTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$PlacesTableOrderingComposer($db: db, $table: table),
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
                  $$PlacesTableReferences(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: ({savedPlacesRefs = false}) {
            return PrefetchHooks(
              db: db,
              explicitlyWatchedTables: [if (savedPlacesRefs) db.savedPlaces],
              addJoins: null,
              getPrefetchedDataCallback: (items) async {
                return [
                  if (savedPlacesRefs)
                    await $_getPrefetchedDataByColumns<
                      PlaceRow,
                      $PlacesTable,
                      SavedPlaceRow,
                      $SavedPlacesTable
                    >(
                      currentTable: table,
                      referencedTable: $$PlacesTableReferences
                          ._savedPlacesRefsTable(db),
                      managerFromTypedResult: (p0) => $$PlacesTableReferences(
                        db,
                        table,
                        p0,
                      ).savedPlacesRefs,
                      currentColumn: 'id',
                      referencedColumn: 'place_id',
                      typedResults: items,
                    ),
                ];
              },
            );
          },
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
      (PlaceRow, $$PlacesTableReferences),
      PlaceRow,
      PrefetchHooks Function({bool savedPlacesRefs})
    >;
typedef $$SavedListsTableCreateCompanionBuilder = SavedListsCompanion Function({
  Value<int> id,
  Value<int?> parentId,
  required String name,
  Value<String?> note,
  required int position,
  Value<bool> shownOnMap,
});
typedef $$SavedListsTableUpdateCompanionBuilder = SavedListsCompanion Function({
  Value<int> id,
  Value<int?> parentId,
  Value<String> name,
  Value<String?> note,
  Value<int> position,
  Value<bool> shownOnMap,
});

final class $$SavedListsTableReferences
    extends BaseReferences<_$CardinalDatabase, $SavedListsTable, SavedListRow> {
  $$SavedListsTableReferences(super.$_db, super.$_table, super.$_typedResult);

  static $SavedListsTable _parentIdTable(_$CardinalDatabase db) =>
      db.savedLists.createAlias('saved_lists__parent_id__saved_lists__id');

  $$SavedListsTableProcessedTableManager? get parentId {
    final $_column = $_itemColumn<int>('parent_id');
    if ($_column == null) return null;
    final manager = $$SavedListsTableTableManager(
      $_db,
      $_db.savedLists,
    ).filter((f) => f.id.sqlEquals($_column));
    final item = $_typedResult.readTableOrNull(_parentIdTable($_db));
    if (item == null) return manager;
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: [item]),
    );
  }

  static MultiTypedResultKey<$SavedPlacesTable, List<SavedPlaceRow>>
  _savedPlacesRefsTable(_$CardinalDatabase db) => MultiTypedResultKey.fromTable(
    db.savedPlaces,
    aliasName: 'saved_lists__id__saved_places__list_id',
  );

  $$SavedPlacesTableProcessedTableManager get savedPlacesRefs {
    final manager = $$SavedPlacesTableTableManager(
      $_db,
      $_db.savedPlaces,
    ).filter((f) => f.listId.id.sqlEquals($_itemColumn<int>('id')!));

    final cache = $_typedResult.readTableOrNull(_savedPlacesRefsTable($_db));
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: cache),
    );
  }
}

class $$SavedListsTableFilterComposer
    extends Composer<_$CardinalDatabase, $SavedListsTable> {
  $$SavedListsTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnFilters<int> get id => $composableBuilder(
    column: $table.id,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get note => $composableBuilder(
    column: $table.note,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get position => $composableBuilder(
    column: $table.position,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<bool> get shownOnMap => $composableBuilder(
    column: $table.shownOnMap,
    builder: (column) => ColumnFilters(column),
  );

  $$SavedListsTableFilterComposer get parentId {
    final $$SavedListsTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.parentId,
      referencedTable: $db.savedLists,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedListsTableFilterComposer(
            $db: $db,
            $table: $db.savedLists,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }

  Expression<bool> savedPlacesRefs(
    Expression<bool> Function($$SavedPlacesTableFilterComposer f) f,
  ) {
    final $$SavedPlacesTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.savedPlaces,
      getReferencedColumn: (t) => t.listId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedPlacesTableFilterComposer(
            $db: $db,
            $table: $db.savedPlaces,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }
}

class $$SavedListsTableOrderingComposer
    extends Composer<_$CardinalDatabase, $SavedListsTable> {
  $$SavedListsTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<int> get id => $composableBuilder(
    column: $table.id,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get note => $composableBuilder(
    column: $table.note,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get position => $composableBuilder(
    column: $table.position,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<bool> get shownOnMap => $composableBuilder(
    column: $table.shownOnMap,
    builder: (column) => ColumnOrderings(column),
  );

  $$SavedListsTableOrderingComposer get parentId {
    final $$SavedListsTableOrderingComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.parentId,
      referencedTable: $db.savedLists,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedListsTableOrderingComposer(
            $db: $db,
            $table: $db.savedLists,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$SavedListsTableAnnotationComposer
    extends Composer<_$CardinalDatabase, $SavedListsTable> {
  $$SavedListsTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<int> get id =>
      $composableBuilder(column: $table.id, builder: (column) => column);

  GeneratedColumn<String> get name =>
      $composableBuilder(column: $table.name, builder: (column) => column);

  GeneratedColumn<String> get note =>
      $composableBuilder(column: $table.note, builder: (column) => column);

  GeneratedColumn<int> get position =>
      $composableBuilder(column: $table.position, builder: (column) => column);

  GeneratedColumn<bool> get shownOnMap => $composableBuilder(
    column: $table.shownOnMap,
    builder: (column) => column,
  );

  $$SavedListsTableAnnotationComposer get parentId {
    final $$SavedListsTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.parentId,
      referencedTable: $db.savedLists,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedListsTableAnnotationComposer(
            $db: $db,
            $table: $db.savedLists,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }

  Expression<T> savedPlacesRefs<T extends Object>(
    Expression<T> Function($$SavedPlacesTableAnnotationComposer a) f,
  ) {
    final $$SavedPlacesTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.savedPlaces,
      getReferencedColumn: (t) => t.listId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedPlacesTableAnnotationComposer(
            $db: $db,
            $table: $db.savedPlaces,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }
}

class $$SavedListsTableTableManager
    extends
        RootTableManager<
          _$CardinalDatabase,
          $SavedListsTable,
          SavedListRow,
          $$SavedListsTableFilterComposer,
          $$SavedListsTableOrderingComposer,
          $$SavedListsTableAnnotationComposer,
          $$SavedListsTableCreateCompanionBuilder,
          $$SavedListsTableUpdateCompanionBuilder,
          (SavedListRow, $$SavedListsTableReferences),
          SavedListRow,
          PrefetchHooks Function({bool parentId, bool savedPlacesRefs})
        > {
  $$SavedListsTableTableManager(_$CardinalDatabase db, $SavedListsTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$SavedListsTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$SavedListsTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$SavedListsTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                Value<int?> parentId = const Value.absent(),
                Value<String> name = const Value.absent(),
                Value<String?> note = const Value.absent(),
                Value<int> position = const Value.absent(),
                Value<bool> shownOnMap = const Value.absent(),
              }) => SavedListsCompanion(
                id: id,
                parentId: parentId,
                name: name,
                note: note,
                position: position,
                shownOnMap: shownOnMap,
              ),
          createCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                Value<int?> parentId = const Value.absent(),
                required String name,
                Value<String?> note = const Value.absent(),
                required int position,
                Value<bool> shownOnMap = const Value.absent(),
              }) => SavedListsCompanion.insert(
                id: id,
                parentId: parentId,
                name: name,
                note: note,
                position: position,
                shownOnMap: shownOnMap,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$SavedListsTable, SavedListRow>(table),
                  $$SavedListsTableReferences(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: ({parentId = false, savedPlacesRefs = false}) {
            return PrefetchHooks(
              db: db,
              explicitlyWatchedTables: [if (savedPlacesRefs) db.savedPlaces],
              addJoins:
                  <
                    T extends TableManagerState<
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic
                    >
                  >(state) {
                    if (parentId) {
                      state = state.withJoin(
                        currentTable: table,
                        currentColumn: table.parentId,
                        referencedTable: $$SavedListsTableReferences
                            ._parentIdTable(db),
                        referencedColumn: $$SavedListsTableReferences
                            ._parentIdTable(db)
                            .id,
                      ) as T;
                    }

                    return state;
                  },
              getPrefetchedDataCallback: (items) async {
                return [
                  if (savedPlacesRefs)
                    await $_getPrefetchedDataByColumns<
                      SavedListRow,
                      $SavedListsTable,
                      SavedPlaceRow,
                      $SavedPlacesTable
                    >(
                      currentTable: table,
                      referencedTable: $$SavedListsTableReferences
                          ._savedPlacesRefsTable(db),
                      managerFromTypedResult: (p0) =>
                          $$SavedListsTableReferences(
                            db,
                            table,
                            p0,
                          ).savedPlacesRefs,
                      currentColumn: 'id',
                      referencedColumn: 'list_id',
                      typedResults: items,
                    ),
                ];
              },
            );
          },
        ),
      );
}

typedef $$SavedListsTableProcessedTableManager =
    ProcessedTableManager<
      _$CardinalDatabase,
      $SavedListsTable,
      SavedListRow,
      $$SavedListsTableFilterComposer,
      $$SavedListsTableOrderingComposer,
      $$SavedListsTableAnnotationComposer,
      $$SavedListsTableCreateCompanionBuilder,
      $$SavedListsTableUpdateCompanionBuilder,
      (SavedListRow, $$SavedListsTableReferences),
      SavedListRow,
      PrefetchHooks Function({bool parentId, bool savedPlacesRefs})
    >;
typedef $$SavedPlacesTableCreateCompanionBuilder =
    SavedPlacesCompanion Function({
      Value<int> id,
      required String placeId,
      Value<int?> listId,
      Value<String?> name,
      Value<String?> note,
      Value<bool> pinned,
      required int position,
      required DateTime savedAt,
      Value<String?> emoji,
      Value<bool> shownOnMap,
    });
typedef $$SavedPlacesTableUpdateCompanionBuilder =
    SavedPlacesCompanion Function({
      Value<int> id,
      Value<String> placeId,
      Value<int?> listId,
      Value<String?> name,
      Value<String?> note,
      Value<bool> pinned,
      Value<int> position,
      Value<DateTime> savedAt,
      Value<String?> emoji,
      Value<bool> shownOnMap,
    });

final class $$SavedPlacesTableReferences
    extends
        BaseReferences<_$CardinalDatabase, $SavedPlacesTable, SavedPlaceRow> {
  $$SavedPlacesTableReferences(super.$_db, super.$_table, super.$_typedResult);

  static $PlacesTable _placeIdTable(_$CardinalDatabase db) =>
      db.places.createAlias('saved_places__place_id__places__id');

  $$PlacesTableProcessedTableManager get placeId {
    final $_column = $_itemColumn<String>('place_id')!;

    final manager = $$PlacesTableTableManager(
      $_db,
      $_db.places,
    ).filter((f) => f.id.sqlEquals($_column));
    final item = $_typedResult.readTableOrNull(_placeIdTable($_db));
    if (item == null) return manager;
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: [item]),
    );
  }

  static $SavedListsTable _listIdTable(_$CardinalDatabase db) =>
      db.savedLists.createAlias('saved_places__list_id__saved_lists__id');

  $$SavedListsTableProcessedTableManager? get listId {
    final $_column = $_itemColumn<int>('list_id');
    if ($_column == null) return null;
    final manager = $$SavedListsTableTableManager(
      $_db,
      $_db.savedLists,
    ).filter((f) => f.id.sqlEquals($_column));
    final item = $_typedResult.readTableOrNull(_listIdTable($_db));
    if (item == null) return manager;
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: [item]),
    );
  }
}

class $$SavedPlacesTableFilterComposer
    extends Composer<_$CardinalDatabase, $SavedPlacesTable> {
  $$SavedPlacesTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnFilters<int> get id => $composableBuilder(
    column: $table.id,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get note => $composableBuilder(
    column: $table.note,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<bool> get pinned => $composableBuilder(
    column: $table.pinned,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get position => $composableBuilder(
    column: $table.position,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get savedAt => $composableBuilder(
    column: $table.savedAt,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get emoji => $composableBuilder(
    column: $table.emoji,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<bool> get shownOnMap => $composableBuilder(
    column: $table.shownOnMap,
    builder: (column) => ColumnFilters(column),
  );

  $$PlacesTableFilterComposer get placeId {
    final $$PlacesTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.placeId,
      referencedTable: $db.places,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$PlacesTableFilterComposer(
            $db: $db,
            $table: $db.places,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }

  $$SavedListsTableFilterComposer get listId {
    final $$SavedListsTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.listId,
      referencedTable: $db.savedLists,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedListsTableFilterComposer(
            $db: $db,
            $table: $db.savedLists,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$SavedPlacesTableOrderingComposer
    extends Composer<_$CardinalDatabase, $SavedPlacesTable> {
  $$SavedPlacesTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<int> get id => $composableBuilder(
    column: $table.id,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get note => $composableBuilder(
    column: $table.note,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<bool> get pinned => $composableBuilder(
    column: $table.pinned,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get position => $composableBuilder(
    column: $table.position,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get savedAt => $composableBuilder(
    column: $table.savedAt,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get emoji => $composableBuilder(
    column: $table.emoji,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<bool> get shownOnMap => $composableBuilder(
    column: $table.shownOnMap,
    builder: (column) => ColumnOrderings(column),
  );

  $$PlacesTableOrderingComposer get placeId {
    final $$PlacesTableOrderingComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.placeId,
      referencedTable: $db.places,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$PlacesTableOrderingComposer(
            $db: $db,
            $table: $db.places,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }

  $$SavedListsTableOrderingComposer get listId {
    final $$SavedListsTableOrderingComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.listId,
      referencedTable: $db.savedLists,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedListsTableOrderingComposer(
            $db: $db,
            $table: $db.savedLists,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$SavedPlacesTableAnnotationComposer
    extends Composer<_$CardinalDatabase, $SavedPlacesTable> {
  $$SavedPlacesTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<int> get id =>
      $composableBuilder(column: $table.id, builder: (column) => column);

  GeneratedColumn<String> get name =>
      $composableBuilder(column: $table.name, builder: (column) => column);

  GeneratedColumn<String> get note =>
      $composableBuilder(column: $table.note, builder: (column) => column);

  GeneratedColumn<bool> get pinned =>
      $composableBuilder(column: $table.pinned, builder: (column) => column);

  GeneratedColumn<int> get position =>
      $composableBuilder(column: $table.position, builder: (column) => column);

  GeneratedColumn<DateTime> get savedAt =>
      $composableBuilder(column: $table.savedAt, builder: (column) => column);

  GeneratedColumn<String> get emoji =>
      $composableBuilder(column: $table.emoji, builder: (column) => column);

  GeneratedColumn<bool> get shownOnMap => $composableBuilder(
    column: $table.shownOnMap,
    builder: (column) => column,
  );

  $$PlacesTableAnnotationComposer get placeId {
    final $$PlacesTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.placeId,
      referencedTable: $db.places,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$PlacesTableAnnotationComposer(
            $db: $db,
            $table: $db.places,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }

  $$SavedListsTableAnnotationComposer get listId {
    final $$SavedListsTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.listId,
      referencedTable: $db.savedLists,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$SavedListsTableAnnotationComposer(
            $db: $db,
            $table: $db.savedLists,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$SavedPlacesTableTableManager
    extends
        RootTableManager<
          _$CardinalDatabase,
          $SavedPlacesTable,
          SavedPlaceRow,
          $$SavedPlacesTableFilterComposer,
          $$SavedPlacesTableOrderingComposer,
          $$SavedPlacesTableAnnotationComposer,
          $$SavedPlacesTableCreateCompanionBuilder,
          $$SavedPlacesTableUpdateCompanionBuilder,
          (SavedPlaceRow, $$SavedPlacesTableReferences),
          SavedPlaceRow,
          PrefetchHooks Function({bool placeId, bool listId})
        > {
  $$SavedPlacesTableTableManager(_$CardinalDatabase db, $SavedPlacesTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$SavedPlacesTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$SavedPlacesTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$SavedPlacesTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                Value<String> placeId = const Value.absent(),
                Value<int?> listId = const Value.absent(),
                Value<String?> name = const Value.absent(),
                Value<String?> note = const Value.absent(),
                Value<bool> pinned = const Value.absent(),
                Value<int> position = const Value.absent(),
                Value<DateTime> savedAt = const Value.absent(),
                Value<String?> emoji = const Value.absent(),
                Value<bool> shownOnMap = const Value.absent(),
              }) => SavedPlacesCompanion(
                id: id,
                placeId: placeId,
                listId: listId,
                name: name,
                note: note,
                pinned: pinned,
                position: position,
                savedAt: savedAt,
                emoji: emoji,
                shownOnMap: shownOnMap,
              ),
          createCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                required String placeId,
                Value<int?> listId = const Value.absent(),
                Value<String?> name = const Value.absent(),
                Value<String?> note = const Value.absent(),
                Value<bool> pinned = const Value.absent(),
                required int position,
                required DateTime savedAt,
                Value<String?> emoji = const Value.absent(),
                Value<bool> shownOnMap = const Value.absent(),
              }) => SavedPlacesCompanion.insert(
                id: id,
                placeId: placeId,
                listId: listId,
                name: name,
                note: note,
                pinned: pinned,
                position: position,
                savedAt: savedAt,
                emoji: emoji,
                shownOnMap: shownOnMap,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$SavedPlacesTable, SavedPlaceRow>(table),
                  $$SavedPlacesTableReferences(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: ({placeId = false, listId = false}) {
            return PrefetchHooks(
              db: db,
              explicitlyWatchedTables: [],
              addJoins:
                  <
                    T extends TableManagerState<
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic,
                      dynamic
                    >
                  >(state) {
                    if (placeId) {
                      state = state.withJoin(
                        currentTable: table,
                        currentColumn: table.placeId,
                        referencedTable: $$SavedPlacesTableReferences
                            ._placeIdTable(db),
                        referencedColumn: $$SavedPlacesTableReferences
                            ._placeIdTable(db)
                            .id,
                      ) as T;
                    }
                    if (listId) {
                      state = state.withJoin(
                        currentTable: table,
                        currentColumn: table.listId,
                        referencedTable: $$SavedPlacesTableReferences
                            ._listIdTable(db),
                        referencedColumn: $$SavedPlacesTableReferences
                            ._listIdTable(db)
                            .id,
                      ) as T;
                    }

                    return state;
                  },
              getPrefetchedDataCallback: (items) async {
                return [];
              },
            );
          },
        ),
      );
}

typedef $$SavedPlacesTableProcessedTableManager =
    ProcessedTableManager<
      _$CardinalDatabase,
      $SavedPlacesTable,
      SavedPlaceRow,
      $$SavedPlacesTableFilterComposer,
      $$SavedPlacesTableOrderingComposer,
      $$SavedPlacesTableAnnotationComposer,
      $$SavedPlacesTableCreateCompanionBuilder,
      $$SavedPlacesTableUpdateCompanionBuilder,
      (SavedPlaceRow, $$SavedPlacesTableReferences),
      SavedPlaceRow,
      PrefetchHooks Function({bool placeId, bool listId})
    >;
typedef $$SettingsTableCreateCompanionBuilder = SettingsCompanion Function({
  required String key,
  required String value,
  Value<int> rowid,
});
typedef $$SettingsTableUpdateCompanionBuilder = SettingsCompanion Function({
  Value<String> key,
  Value<String> value,
  Value<int> rowid,
});

class $$SettingsTableFilterComposer
    extends Composer<_$CardinalDatabase, $SettingsTable> {
  $$SettingsTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnFilters<String> get key => $composableBuilder(
    column: $table.key,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get value => $composableBuilder(
    column: $table.value,
    builder: (column) => ColumnFilters(column),
  );
}

class $$SettingsTableOrderingComposer
    extends Composer<_$CardinalDatabase, $SettingsTable> {
  $$SettingsTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<String> get key => $composableBuilder(
    column: $table.key,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get value => $composableBuilder(
    column: $table.value,
    builder: (column) => ColumnOrderings(column),
  );
}

class $$SettingsTableAnnotationComposer
    extends Composer<_$CardinalDatabase, $SettingsTable> {
  $$SettingsTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<String> get key =>
      $composableBuilder(column: $table.key, builder: (column) => column);

  GeneratedColumn<String> get value =>
      $composableBuilder(column: $table.value, builder: (column) => column);
}

class $$SettingsTableTableManager
    extends
        RootTableManager<
          _$CardinalDatabase,
          $SettingsTable,
          SettingRow,
          $$SettingsTableFilterComposer,
          $$SettingsTableOrderingComposer,
          $$SettingsTableAnnotationComposer,
          $$SettingsTableCreateCompanionBuilder,
          $$SettingsTableUpdateCompanionBuilder,
          (
            SettingRow,
            BaseReferences<_$CardinalDatabase, $SettingsTable, SettingRow>,
          ),
          SettingRow,
          PrefetchHooks Function()
        > {
  $$SettingsTableTableManager(_$CardinalDatabase db, $SettingsTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$SettingsTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$SettingsTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$SettingsTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback: ({
            Value<String> key = const Value.absent(),
            Value<String> value = const Value.absent(),
            Value<int> rowid = const Value.absent(),
          }) => SettingsCompanion(key: key, value: value, rowid: rowid),
          createCompanionCallback: ({
            required String key,
            required String value,
            Value<int> rowid = const Value.absent(),
          }) => SettingsCompanion.insert(key: key, value: value, rowid: rowid),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$SettingsTable, SettingRow>(table),
                  BaseReferences<
                    _$CardinalDatabase,
                    $SettingsTable,
                    SettingRow
                  >(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: null,
        ),
      );
}

typedef $$SettingsTableProcessedTableManager =
    ProcessedTableManager<
      _$CardinalDatabase,
      $SettingsTable,
      SettingRow,
      $$SettingsTableFilterComposer,
      $$SettingsTableOrderingComposer,
      $$SettingsTableAnnotationComposer,
      $$SettingsTableCreateCompanionBuilder,
      $$SettingsTableUpdateCompanionBuilder,
      (
        SettingRow,
        BaseReferences<_$CardinalDatabase, $SettingsTable, SettingRow>,
      ),
      SettingRow,
      PrefetchHooks Function()
    >;

class $CardinalDatabaseManager {
  final _$CardinalDatabase _db;
  $CardinalDatabaseManager(this._db);
  $$PlacesTableTableManager get places =>
      $$PlacesTableTableManager(_db, _db.places);
  $$SavedListsTableTableManager get savedLists =>
      $$SavedListsTableTableManager(_db, _db.savedLists);
  $$SavedPlacesTableTableManager get savedPlaces =>
      $$SavedPlacesTableTableManager(_db, _db.savedPlaces);
  $$SettingsTableTableManager get settings =>
      $$SettingsTableTableManager(_db, _db.settings);
}
