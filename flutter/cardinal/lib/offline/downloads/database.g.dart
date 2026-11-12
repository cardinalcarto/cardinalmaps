// GENERATED CODE - DO NOT MODIFY BY HAND

part of 'database.dart';

// ignore_for_file: type=lint
class $ReleasesTable extends Releases
    with TableInfo<$ReleasesTable, ReleaseRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $ReleasesTable(this.attachedDatabase, [this._alias]);
  @override
  late final GeneratedColumnWithTypeConverter<ReleaseKind, String> kind =
      GeneratedColumn<String>(
        'kind',
        aliasedName,
        false,
        type: DriftSqlType.string,
        requiredDuringInsert: true,
      ).withConverter<ReleaseKind>($ReleasesTable.$converterkind);
  static const VerificationMeta _dateMeta = const VerificationMeta('date');
  @override
  late final GeneratedColumn<String> date = GeneratedColumn<String>(
    'date',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _valhallaBuildIdMeta = const VerificationMeta(
    'valhallaBuildId',
  );
  @override
  late final GeneratedColumn<int> valhallaBuildId = GeneratedColumn<int>(
    'valhalla_build_id',
    aliasedName,
    true,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _etagMeta = const VerificationMeta('etag');
  @override
  late final GeneratedColumn<String> etag = GeneratedColumn<String>(
    'etag',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _manifestIdMeta = const VerificationMeta(
    'manifestId',
  );
  @override
  late final GeneratedColumn<String> manifestId = GeneratedColumn<String>(
    'manifest_id',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _pinnedAtMeta = const VerificationMeta(
    'pinnedAt',
  );
  @override
  late final GeneratedColumn<DateTime> pinnedAt = GeneratedColumn<DateTime>(
    'pinned_at',
    aliasedName,
    false,
    type: DriftSqlType.dateTime,
    requiredDuringInsert: true,
  );
  @override
  List<GeneratedColumn> get $columns => [
    kind,
    date,
    valhallaBuildId,
    etag,
    manifestId,
    pinnedAt,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'releases';
  @override
  VerificationContext validateIntegrity(
    Insertable<ReleaseRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('date')) {
      context.handle(
        _dateMeta,
        date.isAcceptableOrUnknown(data['date']!, _dateMeta),
      );
    } else if (isInserting) {
      context.missing(_dateMeta);
    }
    if (data.containsKey('valhalla_build_id')) {
      context.handle(
        _valhallaBuildIdMeta,
        valhallaBuildId.isAcceptableOrUnknown(
          data['valhalla_build_id']!,
          _valhallaBuildIdMeta,
        ),
      );
    }
    if (data.containsKey('etag')) {
      context.handle(
        _etagMeta,
        etag.isAcceptableOrUnknown(data['etag']!, _etagMeta),
      );
    }
    if (data.containsKey('manifest_id')) {
      context.handle(
        _manifestIdMeta,
        manifestId.isAcceptableOrUnknown(data['manifest_id']!, _manifestIdMeta),
      );
    }
    if (data.containsKey('pinned_at')) {
      context.handle(
        _pinnedAtMeta,
        pinnedAt.isAcceptableOrUnknown(data['pinned_at']!, _pinnedAtMeta),
      );
    } else if (isInserting) {
      context.missing(_pinnedAtMeta);
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {kind};
  @override
  ReleaseRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return ReleaseRow(
      kind: $ReleasesTable.$converterkind.fromSql(
        attachedDatabase.typeMapping.read(
          DriftSqlType.string,
          data['${effectivePrefix}kind'],
        )!,
      ),
      date: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}date'],
      )!,
      valhallaBuildId: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}valhalla_build_id'],
      ),
      etag: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}etag'],
      ),
      manifestId: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}manifest_id'],
      ),
      pinnedAt: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}pinned_at'],
      )!,
    );
  }

  @override
  $ReleasesTable createAlias(String alias) {
    return $ReleasesTable(attachedDatabase, alias);
  }

  static JsonTypeConverter2<ReleaseKind, String, String> $converterkind =
      const EnumNameConverter<ReleaseKind>(ReleaseKind.values);
}

class ReleaseRow extends DataClass implements Insertable<ReleaseRow> {
  final ReleaseKind kind;

  /// As in the bucket's paths: `YYYY-MM-DD`, or `YYYY-MM-DD.N`.
  final String date;

  /// Routing and transit: the Valhalla build their tiles and walks are from.
  final int? valhallaBuildId;

  /// Map: the archive's ETag, which every range request must match (`If-Match`).
  final String? etag;

  /// Transit: the build's manifest ID, which its transfer tiles carry.
  final String? manifestId;
  final DateTime pinnedAt;
  const ReleaseRow({
    required this.kind,
    required this.date,
    this.valhallaBuildId,
    this.etag,
    this.manifestId,
    required this.pinnedAt,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    {
      map['kind'] = Variable<String>($ReleasesTable.$converterkind.toSql(kind));
    }
    map['date'] = Variable<String>(date);
    if (!nullToAbsent || valhallaBuildId != null) {
      map['valhalla_build_id'] = Variable<int>(valhallaBuildId);
    }
    if (!nullToAbsent || etag != null) {
      map['etag'] = Variable<String>(etag);
    }
    if (!nullToAbsent || manifestId != null) {
      map['manifest_id'] = Variable<String>(manifestId);
    }
    map['pinned_at'] = Variable<DateTime>(pinnedAt);
    return map;
  }

  ReleasesCompanion toCompanion(bool nullToAbsent) {
    return ReleasesCompanion(
      kind: Value(kind),
      date: Value(date),
      valhallaBuildId: valhallaBuildId == null && nullToAbsent
          ? const Value.absent()
          : Value(valhallaBuildId),
      etag: etag == null && nullToAbsent ? const Value.absent() : Value(etag),
      manifestId: manifestId == null && nullToAbsent
          ? const Value.absent()
          : Value(manifestId),
      pinnedAt: Value(pinnedAt),
    );
  }

  factory ReleaseRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return ReleaseRow(
      kind: $ReleasesTable.$converterkind.fromJson(
        serializer.fromJson<String>(json['kind']),
      ),
      date: serializer.fromJson<String>(json['date']),
      valhallaBuildId: serializer.fromJson<int?>(json['valhallaBuildId']),
      etag: serializer.fromJson<String?>(json['etag']),
      manifestId: serializer.fromJson<String?>(json['manifestId']),
      pinnedAt: serializer.fromJson<DateTime>(json['pinnedAt']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'kind': serializer.toJson<String>(
        $ReleasesTable.$converterkind.toJson(kind),
      ),
      'date': serializer.toJson<String>(date),
      'valhallaBuildId': serializer.toJson<int?>(valhallaBuildId),
      'etag': serializer.toJson<String?>(etag),
      'manifestId': serializer.toJson<String?>(manifestId),
      'pinnedAt': serializer.toJson<DateTime>(pinnedAt),
    };
  }

  ReleaseRow copyWith({
    ReleaseKind? kind,
    String? date,
    Value<int?> valhallaBuildId = const Value.absent(),
    Value<String?> etag = const Value.absent(),
    Value<String?> manifestId = const Value.absent(),
    DateTime? pinnedAt,
  }) => ReleaseRow(
    kind: kind ?? this.kind,
    date: date ?? this.date,
    valhallaBuildId: valhallaBuildId.present
        ? valhallaBuildId.value
        : this.valhallaBuildId,
    etag: etag.present ? etag.value : this.etag,
    manifestId: manifestId.present ? manifestId.value : this.manifestId,
    pinnedAt: pinnedAt ?? this.pinnedAt,
  );
  ReleaseRow copyWithCompanion(ReleasesCompanion data) {
    return ReleaseRow(
      kind: data.kind.present ? data.kind.value : this.kind,
      date: data.date.present ? data.date.value : this.date,
      valhallaBuildId: data.valhallaBuildId.present
          ? data.valhallaBuildId.value
          : this.valhallaBuildId,
      etag: data.etag.present ? data.etag.value : this.etag,
      manifestId: data.manifestId.present
          ? data.manifestId.value
          : this.manifestId,
      pinnedAt: data.pinnedAt.present ? data.pinnedAt.value : this.pinnedAt,
    );
  }

  @override
  String toString() {
    return (StringBuffer('ReleaseRow(')
          ..write('kind: $kind, ')
          ..write('date: $date, ')
          ..write('valhallaBuildId: $valhallaBuildId, ')
          ..write('etag: $etag, ')
          ..write('manifestId: $manifestId, ')
          ..write('pinnedAt: $pinnedAt')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode =>
      Object.hash(kind, date, valhallaBuildId, etag, manifestId, pinnedAt);
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is ReleaseRow &&
          other.kind == this.kind &&
          other.date == this.date &&
          other.valhallaBuildId == this.valhallaBuildId &&
          other.etag == this.etag &&
          other.manifestId == this.manifestId &&
          other.pinnedAt == this.pinnedAt);
}

class ReleasesCompanion extends UpdateCompanion<ReleaseRow> {
  final Value<ReleaseKind> kind;
  final Value<String> date;
  final Value<int?> valhallaBuildId;
  final Value<String?> etag;
  final Value<String?> manifestId;
  final Value<DateTime> pinnedAt;
  final Value<int> rowid;
  const ReleasesCompanion({
    this.kind = const Value.absent(),
    this.date = const Value.absent(),
    this.valhallaBuildId = const Value.absent(),
    this.etag = const Value.absent(),
    this.manifestId = const Value.absent(),
    this.pinnedAt = const Value.absent(),
    this.rowid = const Value.absent(),
  });
  ReleasesCompanion.insert({
    required ReleaseKind kind,
    required String date,
    this.valhallaBuildId = const Value.absent(),
    this.etag = const Value.absent(),
    this.manifestId = const Value.absent(),
    required DateTime pinnedAt,
    this.rowid = const Value.absent(),
  }) : kind = Value(kind),
       date = Value(date),
       pinnedAt = Value(pinnedAt);
  static Insertable<ReleaseRow> custom({
    Expression<String>? kind,
    Expression<String>? date,
    Expression<int>? valhallaBuildId,
    Expression<String>? etag,
    Expression<String>? manifestId,
    Expression<DateTime>? pinnedAt,
    Expression<int>? rowid,
  }) {
    return RawValuesInsertable({
      if (kind != null) 'kind': kind,
      if (date != null) 'date': date,
      if (valhallaBuildId != null) 'valhalla_build_id': valhallaBuildId,
      if (etag != null) 'etag': etag,
      if (manifestId != null) 'manifest_id': manifestId,
      if (pinnedAt != null) 'pinned_at': pinnedAt,
      if (rowid != null) 'rowid': rowid,
    });
  }

  ReleasesCompanion copyWith({
    Value<ReleaseKind>? kind,
    Value<String>? date,
    Value<int?>? valhallaBuildId,
    Value<String?>? etag,
    Value<String?>? manifestId,
    Value<DateTime>? pinnedAt,
    Value<int>? rowid,
  }) {
    return ReleasesCompanion(
      kind: kind ?? this.kind,
      date: date ?? this.date,
      valhallaBuildId: valhallaBuildId ?? this.valhallaBuildId,
      etag: etag ?? this.etag,
      manifestId: manifestId ?? this.manifestId,
      pinnedAt: pinnedAt ?? this.pinnedAt,
      rowid: rowid ?? this.rowid,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (kind.present) {
      map['kind'] = Variable<String>(
        $ReleasesTable.$converterkind.toSql(kind.value),
      );
    }
    if (date.present) {
      map['date'] = Variable<String>(date.value);
    }
    if (valhallaBuildId.present) {
      map['valhalla_build_id'] = Variable<int>(valhallaBuildId.value);
    }
    if (etag.present) {
      map['etag'] = Variable<String>(etag.value);
    }
    if (manifestId.present) {
      map['manifest_id'] = Variable<String>(manifestId.value);
    }
    if (pinnedAt.present) {
      map['pinned_at'] = Variable<DateTime>(pinnedAt.value);
    }
    if (rowid.present) {
      map['rowid'] = Variable<int>(rowid.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('ReleasesCompanion(')
          ..write('kind: $kind, ')
          ..write('date: $date, ')
          ..write('valhallaBuildId: $valhallaBuildId, ')
          ..write('etag: $etag, ')
          ..write('manifestId: $manifestId, ')
          ..write('pinnedAt: $pinnedAt, ')
          ..write('rowid: $rowid')
          ..write(')'))
        .toString();
  }
}

class $RegionsTable extends Regions with TableInfo<$RegionsTable, RegionRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $RegionsTable(this.attachedDatabase, [this._alias]);
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
  @override
  late final GeneratedColumnWithTypeConverter<RegionKind, String> kind =
      GeneratedColumn<String>(
        'kind',
        aliasedName,
        false,
        type: DriftSqlType.string,
        requiredDuringInsert: true,
      ).withConverter<RegionKind>($RegionsTable.$converterkind);
  static const VerificationMeta _nameMeta = const VerificationMeta('name');
  @override
  late final GeneratedColumn<String> name = GeneratedColumn<String>(
    'name',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _polygonMeta = const VerificationMeta(
    'polygon',
  );
  @override
  late final GeneratedColumn<String> polygon = GeneratedColumn<String>(
    'polygon',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _mapReleaseMeta = const VerificationMeta(
    'mapRelease',
  );
  @override
  late final GeneratedColumn<String> mapRelease = GeneratedColumn<String>(
    'map_release',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _routingReleaseMeta = const VerificationMeta(
    'routingRelease',
  );
  @override
  late final GeneratedColumn<String> routingRelease = GeneratedColumn<String>(
    'routing_release',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _transitReleaseMeta = const VerificationMeta(
    'transitRelease',
  );
  @override
  late final GeneratedColumn<String> transitRelease = GeneratedColumn<String>(
    'transit_release',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  @override
  late final GeneratedColumnWithTypeConverter<RegionState, String> state =
      GeneratedColumn<String>(
        'state',
        aliasedName,
        false,
        type: DriftSqlType.string,
        requiredDuringInsert: true,
      ).withConverter<RegionState>($RegionsTable.$converterstate);
  static const VerificationMeta _errorMeta = const VerificationMeta('error');
  @override
  late final GeneratedColumn<String> error = GeneratedColumn<String>(
    'error',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _mapTilesMeta = const VerificationMeta(
    'mapTiles',
  );
  @override
  late final GeneratedColumn<Uint8List> mapTiles = GeneratedColumn<Uint8List>(
    'map_tiles',
    aliasedName,
    true,
    type: DriftSqlType.blob,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _mapBytesMeta = const VerificationMeta(
    'mapBytes',
  );
  @override
  late final GeneratedColumn<int> mapBytes = GeneratedColumn<int>(
    'map_bytes',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultValue: const Constant(0),
  );
  static const VerificationMeta _createdAtMeta = const VerificationMeta(
    'createdAt',
  );
  @override
  late final GeneratedColumn<DateTime> createdAt = GeneratedColumn<DateTime>(
    'created_at',
    aliasedName,
    false,
    type: DriftSqlType.dateTime,
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
  @override
  List<GeneratedColumn> get $columns => [
    id,
    kind,
    name,
    polygon,
    mapRelease,
    routingRelease,
    transitRelease,
    state,
    error,
    mapTiles,
    mapBytes,
    createdAt,
    updatedAt,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'regions';
  @override
  VerificationContext validateIntegrity(
    Insertable<RegionRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('id')) {
      context.handle(_idMeta, id.isAcceptableOrUnknown(data['id']!, _idMeta));
    }
    if (data.containsKey('name')) {
      context.handle(
        _nameMeta,
        name.isAcceptableOrUnknown(data['name']!, _nameMeta),
      );
    }
    if (data.containsKey('polygon')) {
      context.handle(
        _polygonMeta,
        polygon.isAcceptableOrUnknown(data['polygon']!, _polygonMeta),
      );
    }
    if (data.containsKey('map_release')) {
      context.handle(
        _mapReleaseMeta,
        mapRelease.isAcceptableOrUnknown(data['map_release']!, _mapReleaseMeta),
      );
    }
    if (data.containsKey('routing_release')) {
      context.handle(
        _routingReleaseMeta,
        routingRelease.isAcceptableOrUnknown(
          data['routing_release']!,
          _routingReleaseMeta,
        ),
      );
    }
    if (data.containsKey('transit_release')) {
      context.handle(
        _transitReleaseMeta,
        transitRelease.isAcceptableOrUnknown(
          data['transit_release']!,
          _transitReleaseMeta,
        ),
      );
    }
    if (data.containsKey('error')) {
      context.handle(
        _errorMeta,
        error.isAcceptableOrUnknown(data['error']!, _errorMeta),
      );
    }
    if (data.containsKey('map_tiles')) {
      context.handle(
        _mapTilesMeta,
        mapTiles.isAcceptableOrUnknown(data['map_tiles']!, _mapTilesMeta),
      );
    }
    if (data.containsKey('map_bytes')) {
      context.handle(
        _mapBytesMeta,
        mapBytes.isAcceptableOrUnknown(data['map_bytes']!, _mapBytesMeta),
      );
    }
    if (data.containsKey('created_at')) {
      context.handle(
        _createdAtMeta,
        createdAt.isAcceptableOrUnknown(data['created_at']!, _createdAtMeta),
      );
    } else if (isInserting) {
      context.missing(_createdAtMeta);
    }
    if (data.containsKey('updated_at')) {
      context.handle(
        _updatedAtMeta,
        updatedAt.isAcceptableOrUnknown(data['updated_at']!, _updatedAtMeta),
      );
    } else if (isInserting) {
      context.missing(_updatedAtMeta);
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {id};
  @override
  RegionRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return RegionRow(
      id: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}id'],
      )!,
      kind: $RegionsTable.$converterkind.fromSql(
        attachedDatabase.typeMapping.read(
          DriftSqlType.string,
          data['${effectivePrefix}kind'],
        )!,
      ),
      name: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}name'],
      ),
      polygon: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}polygon'],
      ),
      mapRelease: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}map_release'],
      ),
      routingRelease: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}routing_release'],
      ),
      transitRelease: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}transit_release'],
      ),
      state: $RegionsTable.$converterstate.fromSql(
        attachedDatabase.typeMapping.read(
          DriftSqlType.string,
          data['${effectivePrefix}state'],
        )!,
      ),
      error: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}error'],
      ),
      mapTiles: attachedDatabase.typeMapping.read(
        DriftSqlType.blob,
        data['${effectivePrefix}map_tiles'],
      ),
      mapBytes: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}map_bytes'],
      )!,
      createdAt: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}created_at'],
      )!,
      updatedAt: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}updated_at'],
      )!,
    );
  }

  @override
  $RegionsTable createAlias(String alias) {
    return $RegionsTable(attachedDatabase, alias);
  }

  static JsonTypeConverter2<RegionKind, String, String> $converterkind =
      const EnumNameConverter<RegionKind>(RegionKind.values);
  static JsonTypeConverter2<RegionState, String, String> $converterstate =
      const EnumNameConverter<RegionState>(RegionState.values);
}

class RegionRow extends DataClass implements Insertable<RegionRow> {
  final int id;
  final RegionKind kind;
  final String? name;

  /// `[[lng, lat], ...]` as JSON; null for the on-demand region.
  final String? polygon;

  /// The releases its files are from (the pinned ones when it was planned).
  final String? mapRelease;
  final String? routingRelease;
  final String? transitRelease;
  final RegionState state;

  /// Why it failed, for logs.
  final String? error;

  /// Every map tile of the region the archive has (opaque bytes from the planner), for
  /// claiming tiles another region brought.
  final Uint8List? mapTiles;

  /// The map's bytes and tile count, as planned.
  final int mapBytes;
  final DateTime createdAt;
  final DateTime updatedAt;
  const RegionRow({
    required this.id,
    required this.kind,
    this.name,
    this.polygon,
    this.mapRelease,
    this.routingRelease,
    this.transitRelease,
    required this.state,
    this.error,
    this.mapTiles,
    required this.mapBytes,
    required this.createdAt,
    required this.updatedAt,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['id'] = Variable<int>(id);
    {
      map['kind'] = Variable<String>($RegionsTable.$converterkind.toSql(kind));
    }
    if (!nullToAbsent || name != null) {
      map['name'] = Variable<String>(name);
    }
    if (!nullToAbsent || polygon != null) {
      map['polygon'] = Variable<String>(polygon);
    }
    if (!nullToAbsent || mapRelease != null) {
      map['map_release'] = Variable<String>(mapRelease);
    }
    if (!nullToAbsent || routingRelease != null) {
      map['routing_release'] = Variable<String>(routingRelease);
    }
    if (!nullToAbsent || transitRelease != null) {
      map['transit_release'] = Variable<String>(transitRelease);
    }
    {
      map['state'] = Variable<String>(
        $RegionsTable.$converterstate.toSql(state),
      );
    }
    if (!nullToAbsent || error != null) {
      map['error'] = Variable<String>(error);
    }
    if (!nullToAbsent || mapTiles != null) {
      map['map_tiles'] = Variable<Uint8List>(mapTiles);
    }
    map['map_bytes'] = Variable<int>(mapBytes);
    map['created_at'] = Variable<DateTime>(createdAt);
    map['updated_at'] = Variable<DateTime>(updatedAt);
    return map;
  }

  RegionsCompanion toCompanion(bool nullToAbsent) {
    return RegionsCompanion(
      id: Value(id),
      kind: Value(kind),
      name: name == null && nullToAbsent ? const Value.absent() : Value(name),
      polygon: polygon == null && nullToAbsent
          ? const Value.absent()
          : Value(polygon),
      mapRelease: mapRelease == null && nullToAbsent
          ? const Value.absent()
          : Value(mapRelease),
      routingRelease: routingRelease == null && nullToAbsent
          ? const Value.absent()
          : Value(routingRelease),
      transitRelease: transitRelease == null && nullToAbsent
          ? const Value.absent()
          : Value(transitRelease),
      state: Value(state),
      error: error == null && nullToAbsent
          ? const Value.absent()
          : Value(error),
      mapTiles: mapTiles == null && nullToAbsent
          ? const Value.absent()
          : Value(mapTiles),
      mapBytes: Value(mapBytes),
      createdAt: Value(createdAt),
      updatedAt: Value(updatedAt),
    );
  }

  factory RegionRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return RegionRow(
      id: serializer.fromJson<int>(json['id']),
      kind: $RegionsTable.$converterkind.fromJson(
        serializer.fromJson<String>(json['kind']),
      ),
      name: serializer.fromJson<String?>(json['name']),
      polygon: serializer.fromJson<String?>(json['polygon']),
      mapRelease: serializer.fromJson<String?>(json['mapRelease']),
      routingRelease: serializer.fromJson<String?>(json['routingRelease']),
      transitRelease: serializer.fromJson<String?>(json['transitRelease']),
      state: $RegionsTable.$converterstate.fromJson(
        serializer.fromJson<String>(json['state']),
      ),
      error: serializer.fromJson<String?>(json['error']),
      mapTiles: serializer.fromJson<Uint8List?>(json['mapTiles']),
      mapBytes: serializer.fromJson<int>(json['mapBytes']),
      createdAt: serializer.fromJson<DateTime>(json['createdAt']),
      updatedAt: serializer.fromJson<DateTime>(json['updatedAt']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'id': serializer.toJson<int>(id),
      'kind': serializer.toJson<String>(
        $RegionsTable.$converterkind.toJson(kind),
      ),
      'name': serializer.toJson<String?>(name),
      'polygon': serializer.toJson<String?>(polygon),
      'mapRelease': serializer.toJson<String?>(mapRelease),
      'routingRelease': serializer.toJson<String?>(routingRelease),
      'transitRelease': serializer.toJson<String?>(transitRelease),
      'state': serializer.toJson<String>(
        $RegionsTable.$converterstate.toJson(state),
      ),
      'error': serializer.toJson<String?>(error),
      'mapTiles': serializer.toJson<Uint8List?>(mapTiles),
      'mapBytes': serializer.toJson<int>(mapBytes),
      'createdAt': serializer.toJson<DateTime>(createdAt),
      'updatedAt': serializer.toJson<DateTime>(updatedAt),
    };
  }

  RegionRow copyWith({
    int? id,
    RegionKind? kind,
    Value<String?> name = const Value.absent(),
    Value<String?> polygon = const Value.absent(),
    Value<String?> mapRelease = const Value.absent(),
    Value<String?> routingRelease = const Value.absent(),
    Value<String?> transitRelease = const Value.absent(),
    RegionState? state,
    Value<String?> error = const Value.absent(),
    Value<Uint8List?> mapTiles = const Value.absent(),
    int? mapBytes,
    DateTime? createdAt,
    DateTime? updatedAt,
  }) => RegionRow(
    id: id ?? this.id,
    kind: kind ?? this.kind,
    name: name.present ? name.value : this.name,
    polygon: polygon.present ? polygon.value : this.polygon,
    mapRelease: mapRelease.present ? mapRelease.value : this.mapRelease,
    routingRelease: routingRelease.present
        ? routingRelease.value
        : this.routingRelease,
    transitRelease: transitRelease.present
        ? transitRelease.value
        : this.transitRelease,
    state: state ?? this.state,
    error: error.present ? error.value : this.error,
    mapTiles: mapTiles.present ? mapTiles.value : this.mapTiles,
    mapBytes: mapBytes ?? this.mapBytes,
    createdAt: createdAt ?? this.createdAt,
    updatedAt: updatedAt ?? this.updatedAt,
  );
  RegionRow copyWithCompanion(RegionsCompanion data) {
    return RegionRow(
      id: data.id.present ? data.id.value : this.id,
      kind: data.kind.present ? data.kind.value : this.kind,
      name: data.name.present ? data.name.value : this.name,
      polygon: data.polygon.present ? data.polygon.value : this.polygon,
      mapRelease: data.mapRelease.present
          ? data.mapRelease.value
          : this.mapRelease,
      routingRelease: data.routingRelease.present
          ? data.routingRelease.value
          : this.routingRelease,
      transitRelease: data.transitRelease.present
          ? data.transitRelease.value
          : this.transitRelease,
      state: data.state.present ? data.state.value : this.state,
      error: data.error.present ? data.error.value : this.error,
      mapTiles: data.mapTiles.present ? data.mapTiles.value : this.mapTiles,
      mapBytes: data.mapBytes.present ? data.mapBytes.value : this.mapBytes,
      createdAt: data.createdAt.present ? data.createdAt.value : this.createdAt,
      updatedAt: data.updatedAt.present ? data.updatedAt.value : this.updatedAt,
    );
  }

  @override
  String toString() {
    return (StringBuffer('RegionRow(')
          ..write('id: $id, ')
          ..write('kind: $kind, ')
          ..write('name: $name, ')
          ..write('polygon: $polygon, ')
          ..write('mapRelease: $mapRelease, ')
          ..write('routingRelease: $routingRelease, ')
          ..write('transitRelease: $transitRelease, ')
          ..write('state: $state, ')
          ..write('error: $error, ')
          ..write('mapTiles: $mapTiles, ')
          ..write('mapBytes: $mapBytes, ')
          ..write('createdAt: $createdAt, ')
          ..write('updatedAt: $updatedAt')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(
    id,
    kind,
    name,
    polygon,
    mapRelease,
    routingRelease,
    transitRelease,
    state,
    error,
    $driftBlobEquality.hash(mapTiles),
    mapBytes,
    createdAt,
    updatedAt,
  );
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is RegionRow &&
          other.id == this.id &&
          other.kind == this.kind &&
          other.name == this.name &&
          other.polygon == this.polygon &&
          other.mapRelease == this.mapRelease &&
          other.routingRelease == this.routingRelease &&
          other.transitRelease == this.transitRelease &&
          other.state == this.state &&
          other.error == this.error &&
          $driftBlobEquality.equals(other.mapTiles, this.mapTiles) &&
          other.mapBytes == this.mapBytes &&
          other.createdAt == this.createdAt &&
          other.updatedAt == this.updatedAt);
}

class RegionsCompanion extends UpdateCompanion<RegionRow> {
  final Value<int> id;
  final Value<RegionKind> kind;
  final Value<String?> name;
  final Value<String?> polygon;
  final Value<String?> mapRelease;
  final Value<String?> routingRelease;
  final Value<String?> transitRelease;
  final Value<RegionState> state;
  final Value<String?> error;
  final Value<Uint8List?> mapTiles;
  final Value<int> mapBytes;
  final Value<DateTime> createdAt;
  final Value<DateTime> updatedAt;
  const RegionsCompanion({
    this.id = const Value.absent(),
    this.kind = const Value.absent(),
    this.name = const Value.absent(),
    this.polygon = const Value.absent(),
    this.mapRelease = const Value.absent(),
    this.routingRelease = const Value.absent(),
    this.transitRelease = const Value.absent(),
    this.state = const Value.absent(),
    this.error = const Value.absent(),
    this.mapTiles = const Value.absent(),
    this.mapBytes = const Value.absent(),
    this.createdAt = const Value.absent(),
    this.updatedAt = const Value.absent(),
  });
  RegionsCompanion.insert({
    this.id = const Value.absent(),
    required RegionKind kind,
    this.name = const Value.absent(),
    this.polygon = const Value.absent(),
    this.mapRelease = const Value.absent(),
    this.routingRelease = const Value.absent(),
    this.transitRelease = const Value.absent(),
    required RegionState state,
    this.error = const Value.absent(),
    this.mapTiles = const Value.absent(),
    this.mapBytes = const Value.absent(),
    required DateTime createdAt,
    required DateTime updatedAt,
  }) : kind = Value(kind),
       state = Value(state),
       createdAt = Value(createdAt),
       updatedAt = Value(updatedAt);
  static Insertable<RegionRow> custom({
    Expression<int>? id,
    Expression<String>? kind,
    Expression<String>? name,
    Expression<String>? polygon,
    Expression<String>? mapRelease,
    Expression<String>? routingRelease,
    Expression<String>? transitRelease,
    Expression<String>? state,
    Expression<String>? error,
    Expression<Uint8List>? mapTiles,
    Expression<int>? mapBytes,
    Expression<DateTime>? createdAt,
    Expression<DateTime>? updatedAt,
  }) {
    return RawValuesInsertable({
      if (id != null) 'id': id,
      if (kind != null) 'kind': kind,
      if (name != null) 'name': name,
      if (polygon != null) 'polygon': polygon,
      if (mapRelease != null) 'map_release': mapRelease,
      if (routingRelease != null) 'routing_release': routingRelease,
      if (transitRelease != null) 'transit_release': transitRelease,
      if (state != null) 'state': state,
      if (error != null) 'error': error,
      if (mapTiles != null) 'map_tiles': mapTiles,
      if (mapBytes != null) 'map_bytes': mapBytes,
      if (createdAt != null) 'created_at': createdAt,
      if (updatedAt != null) 'updated_at': updatedAt,
    });
  }

  RegionsCompanion copyWith({
    Value<int>? id,
    Value<RegionKind>? kind,
    Value<String?>? name,
    Value<String?>? polygon,
    Value<String?>? mapRelease,
    Value<String?>? routingRelease,
    Value<String?>? transitRelease,
    Value<RegionState>? state,
    Value<String?>? error,
    Value<Uint8List?>? mapTiles,
    Value<int>? mapBytes,
    Value<DateTime>? createdAt,
    Value<DateTime>? updatedAt,
  }) {
    return RegionsCompanion(
      id: id ?? this.id,
      kind: kind ?? this.kind,
      name: name ?? this.name,
      polygon: polygon ?? this.polygon,
      mapRelease: mapRelease ?? this.mapRelease,
      routingRelease: routingRelease ?? this.routingRelease,
      transitRelease: transitRelease ?? this.transitRelease,
      state: state ?? this.state,
      error: error ?? this.error,
      mapTiles: mapTiles ?? this.mapTiles,
      mapBytes: mapBytes ?? this.mapBytes,
      createdAt: createdAt ?? this.createdAt,
      updatedAt: updatedAt ?? this.updatedAt,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (id.present) {
      map['id'] = Variable<int>(id.value);
    }
    if (kind.present) {
      map['kind'] = Variable<String>(
        $RegionsTable.$converterkind.toSql(kind.value),
      );
    }
    if (name.present) {
      map['name'] = Variable<String>(name.value);
    }
    if (polygon.present) {
      map['polygon'] = Variable<String>(polygon.value);
    }
    if (mapRelease.present) {
      map['map_release'] = Variable<String>(mapRelease.value);
    }
    if (routingRelease.present) {
      map['routing_release'] = Variable<String>(routingRelease.value);
    }
    if (transitRelease.present) {
      map['transit_release'] = Variable<String>(transitRelease.value);
    }
    if (state.present) {
      map['state'] = Variable<String>(
        $RegionsTable.$converterstate.toSql(state.value),
      );
    }
    if (error.present) {
      map['error'] = Variable<String>(error.value);
    }
    if (mapTiles.present) {
      map['map_tiles'] = Variable<Uint8List>(mapTiles.value);
    }
    if (mapBytes.present) {
      map['map_bytes'] = Variable<int>(mapBytes.value);
    }
    if (createdAt.present) {
      map['created_at'] = Variable<DateTime>(createdAt.value);
    }
    if (updatedAt.present) {
      map['updated_at'] = Variable<DateTime>(updatedAt.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('RegionsCompanion(')
          ..write('id: $id, ')
          ..write('kind: $kind, ')
          ..write('name: $name, ')
          ..write('polygon: $polygon, ')
          ..write('mapRelease: $mapRelease, ')
          ..write('routingRelease: $routingRelease, ')
          ..write('transitRelease: $transitRelease, ')
          ..write('state: $state, ')
          ..write('error: $error, ')
          ..write('mapTiles: $mapTiles, ')
          ..write('mapBytes: $mapBytes, ')
          ..write('createdAt: $createdAt, ')
          ..write('updatedAt: $updatedAt')
          ..write(')'))
        .toString();
  }
}

class $FilesTable extends Files with TableInfo<$FilesTable, FileRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $FilesTable(this.attachedDatabase, [this._alias]);
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
  @override
  late final GeneratedColumnWithTypeConverter<ReleaseKind, String> kind =
      GeneratedColumn<String>(
        'kind',
        aliasedName,
        false,
        type: DriftSqlType.string,
        requiredDuringInsert: true,
      ).withConverter<ReleaseKind>($FilesTable.$converterkind);
  static const VerificationMeta _releaseMeta = const VerificationMeta(
    'release',
  );
  @override
  late final GeneratedColumn<String> release = GeneratedColumn<String>(
    'release',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _pathMeta = const VerificationMeta('path');
  @override
  late final GeneratedColumn<String> path = GeneratedColumn<String>(
    'path',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _installPathMeta = const VerificationMeta(
    'installPath',
  );
  @override
  late final GeneratedColumn<String> installPath = GeneratedColumn<String>(
    'install_path',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _bytesMeta = const VerificationMeta('bytes');
  @override
  late final GeneratedColumn<int> bytes = GeneratedColumn<int>(
    'bytes',
    aliasedName,
    true,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _expectedSha256Meta = const VerificationMeta(
    'expectedSha256',
  );
  @override
  late final GeneratedColumn<String> expectedSha256 = GeneratedColumn<String>(
    'expected_sha256',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _verifiedSha256Meta = const VerificationMeta(
    'verifiedSha256',
  );
  @override
  late final GeneratedColumn<String> verifiedSha256 = GeneratedColumn<String>(
    'verified_sha256',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  @override
  late final GeneratedColumnWithTypeConverter<FileState, String> state =
      GeneratedColumn<String>(
        'state',
        aliasedName,
        false,
        type: DriftSqlType.string,
        requiredDuringInsert: true,
      ).withConverter<FileState>($FilesTable.$converterstate);
  static const VerificationMeta _attemptsMeta = const VerificationMeta(
    'attempts',
  );
  @override
  late final GeneratedColumn<int> attempts = GeneratedColumn<int>(
    'attempts',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultValue: const Constant(0),
  );
  static const VerificationMeta _errorMeta = const VerificationMeta('error');
  @override
  late final GeneratedColumn<String> error = GeneratedColumn<String>(
    'error',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  static const VerificationMeta _lastUsedMeta = const VerificationMeta(
    'lastUsed',
  );
  @override
  late final GeneratedColumn<DateTime> lastUsed = GeneratedColumn<DateTime>(
    'last_used',
    aliasedName,
    true,
    type: DriftSqlType.dateTime,
    requiredDuringInsert: false,
  );
  @override
  List<GeneratedColumn> get $columns => [
    id,
    kind,
    release,
    path,
    installPath,
    bytes,
    expectedSha256,
    verifiedSha256,
    state,
    attempts,
    error,
    lastUsed,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'files';
  @override
  VerificationContext validateIntegrity(
    Insertable<FileRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('id')) {
      context.handle(_idMeta, id.isAcceptableOrUnknown(data['id']!, _idMeta));
    }
    if (data.containsKey('release')) {
      context.handle(
        _releaseMeta,
        release.isAcceptableOrUnknown(data['release']!, _releaseMeta),
      );
    } else if (isInserting) {
      context.missing(_releaseMeta);
    }
    if (data.containsKey('path')) {
      context.handle(
        _pathMeta,
        path.isAcceptableOrUnknown(data['path']!, _pathMeta),
      );
    } else if (isInserting) {
      context.missing(_pathMeta);
    }
    if (data.containsKey('install_path')) {
      context.handle(
        _installPathMeta,
        installPath.isAcceptableOrUnknown(
          data['install_path']!,
          _installPathMeta,
        ),
      );
    } else if (isInserting) {
      context.missing(_installPathMeta);
    }
    if (data.containsKey('bytes')) {
      context.handle(
        _bytesMeta,
        bytes.isAcceptableOrUnknown(data['bytes']!, _bytesMeta),
      );
    }
    if (data.containsKey('expected_sha256')) {
      context.handle(
        _expectedSha256Meta,
        expectedSha256.isAcceptableOrUnknown(
          data['expected_sha256']!,
          _expectedSha256Meta,
        ),
      );
    }
    if (data.containsKey('verified_sha256')) {
      context.handle(
        _verifiedSha256Meta,
        verifiedSha256.isAcceptableOrUnknown(
          data['verified_sha256']!,
          _verifiedSha256Meta,
        ),
      );
    }
    if (data.containsKey('attempts')) {
      context.handle(
        _attemptsMeta,
        attempts.isAcceptableOrUnknown(data['attempts']!, _attemptsMeta),
      );
    }
    if (data.containsKey('error')) {
      context.handle(
        _errorMeta,
        error.isAcceptableOrUnknown(data['error']!, _errorMeta),
      );
    }
    if (data.containsKey('last_used')) {
      context.handle(
        _lastUsedMeta,
        lastUsed.isAcceptableOrUnknown(data['last_used']!, _lastUsedMeta),
      );
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {id};
  @override
  List<Set<GeneratedColumn>> get uniqueKeys => [
    {kind, release, path},
  ];
  @override
  FileRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return FileRow(
      id: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}id'],
      )!,
      kind: $FilesTable.$converterkind.fromSql(
        attachedDatabase.typeMapping.read(
          DriftSqlType.string,
          data['${effectivePrefix}kind'],
        )!,
      ),
      release: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}release'],
      )!,
      path: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}path'],
      )!,
      installPath: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}install_path'],
      )!,
      bytes: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}bytes'],
      ),
      expectedSha256: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}expected_sha256'],
      ),
      verifiedSha256: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}verified_sha256'],
      ),
      state: $FilesTable.$converterstate.fromSql(
        attachedDatabase.typeMapping.read(
          DriftSqlType.string,
          data['${effectivePrefix}state'],
        )!,
      ),
      attempts: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}attempts'],
      )!,
      error: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}error'],
      ),
      lastUsed: attachedDatabase.typeMapping.read(
        DriftSqlType.dateTime,
        data['${effectivePrefix}last_used'],
      ),
    );
  }

  @override
  $FilesTable createAlias(String alias) {
    return $FilesTable(attachedDatabase, alias);
  }

  static JsonTypeConverter2<ReleaseKind, String, String> $converterkind =
      const EnumNameConverter<ReleaseKind>(ReleaseKind.values);
  static JsonTypeConverter2<FileState, String, String> $converterstate =
      const EnumNameConverter<FileState>(FileState.values);
}

class FileRow extends DataClass implements Insertable<FileRow> {
  final int id;
  final ReleaseKind kind;
  final String release;

  /// Where it is under the release in the bucket, such as `tiles/2/000/792/230.gph`.
  final String path;

  /// Where it goes under the offline folder, such as `routing/<date>/tiles/...`.
  final String installPath;

  /// Its size, when the index says (transfer tiles' sizes aren't published).
  final int? bytes;

  /// The SHA-256 to check it against, from the index; for files whose index gives none,
  /// the bucket's `x-amz-meta-sha256` for the object, taken when it downloads.
  final String? expectedSha256;

  /// What it hashed to, once it matched.
  final String? verifiedSha256;
  final FileState state;
  final int attempts;
  final String? error;

  /// When a route last needed it (on-demand eviction goes by this), or when it arrived.
  final DateTime? lastUsed;
  const FileRow({
    required this.id,
    required this.kind,
    required this.release,
    required this.path,
    required this.installPath,
    this.bytes,
    this.expectedSha256,
    this.verifiedSha256,
    required this.state,
    required this.attempts,
    this.error,
    this.lastUsed,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['id'] = Variable<int>(id);
    {
      map['kind'] = Variable<String>($FilesTable.$converterkind.toSql(kind));
    }
    map['release'] = Variable<String>(release);
    map['path'] = Variable<String>(path);
    map['install_path'] = Variable<String>(installPath);
    if (!nullToAbsent || bytes != null) {
      map['bytes'] = Variable<int>(bytes);
    }
    if (!nullToAbsent || expectedSha256 != null) {
      map['expected_sha256'] = Variable<String>(expectedSha256);
    }
    if (!nullToAbsent || verifiedSha256 != null) {
      map['verified_sha256'] = Variable<String>(verifiedSha256);
    }
    {
      map['state'] = Variable<String>($FilesTable.$converterstate.toSql(state));
    }
    map['attempts'] = Variable<int>(attempts);
    if (!nullToAbsent || error != null) {
      map['error'] = Variable<String>(error);
    }
    if (!nullToAbsent || lastUsed != null) {
      map['last_used'] = Variable<DateTime>(lastUsed);
    }
    return map;
  }

  FilesCompanion toCompanion(bool nullToAbsent) {
    return FilesCompanion(
      id: Value(id),
      kind: Value(kind),
      release: Value(release),
      path: Value(path),
      installPath: Value(installPath),
      bytes: bytes == null && nullToAbsent
          ? const Value.absent()
          : Value(bytes),
      expectedSha256: expectedSha256 == null && nullToAbsent
          ? const Value.absent()
          : Value(expectedSha256),
      verifiedSha256: verifiedSha256 == null && nullToAbsent
          ? const Value.absent()
          : Value(verifiedSha256),
      state: Value(state),
      attempts: Value(attempts),
      error: error == null && nullToAbsent
          ? const Value.absent()
          : Value(error),
      lastUsed: lastUsed == null && nullToAbsent
          ? const Value.absent()
          : Value(lastUsed),
    );
  }

  factory FileRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return FileRow(
      id: serializer.fromJson<int>(json['id']),
      kind: $FilesTable.$converterkind.fromJson(
        serializer.fromJson<String>(json['kind']),
      ),
      release: serializer.fromJson<String>(json['release']),
      path: serializer.fromJson<String>(json['path']),
      installPath: serializer.fromJson<String>(json['installPath']),
      bytes: serializer.fromJson<int?>(json['bytes']),
      expectedSha256: serializer.fromJson<String?>(json['expectedSha256']),
      verifiedSha256: serializer.fromJson<String?>(json['verifiedSha256']),
      state: $FilesTable.$converterstate.fromJson(
        serializer.fromJson<String>(json['state']),
      ),
      attempts: serializer.fromJson<int>(json['attempts']),
      error: serializer.fromJson<String?>(json['error']),
      lastUsed: serializer.fromJson<DateTime?>(json['lastUsed']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'id': serializer.toJson<int>(id),
      'kind': serializer.toJson<String>(
        $FilesTable.$converterkind.toJson(kind),
      ),
      'release': serializer.toJson<String>(release),
      'path': serializer.toJson<String>(path),
      'installPath': serializer.toJson<String>(installPath),
      'bytes': serializer.toJson<int?>(bytes),
      'expectedSha256': serializer.toJson<String?>(expectedSha256),
      'verifiedSha256': serializer.toJson<String?>(verifiedSha256),
      'state': serializer.toJson<String>(
        $FilesTable.$converterstate.toJson(state),
      ),
      'attempts': serializer.toJson<int>(attempts),
      'error': serializer.toJson<String?>(error),
      'lastUsed': serializer.toJson<DateTime?>(lastUsed),
    };
  }

  FileRow copyWith({
    int? id,
    ReleaseKind? kind,
    String? release,
    String? path,
    String? installPath,
    Value<int?> bytes = const Value.absent(),
    Value<String?> expectedSha256 = const Value.absent(),
    Value<String?> verifiedSha256 = const Value.absent(),
    FileState? state,
    int? attempts,
    Value<String?> error = const Value.absent(),
    Value<DateTime?> lastUsed = const Value.absent(),
  }) => FileRow(
    id: id ?? this.id,
    kind: kind ?? this.kind,
    release: release ?? this.release,
    path: path ?? this.path,
    installPath: installPath ?? this.installPath,
    bytes: bytes.present ? bytes.value : this.bytes,
    expectedSha256: expectedSha256.present
        ? expectedSha256.value
        : this.expectedSha256,
    verifiedSha256: verifiedSha256.present
        ? verifiedSha256.value
        : this.verifiedSha256,
    state: state ?? this.state,
    attempts: attempts ?? this.attempts,
    error: error.present ? error.value : this.error,
    lastUsed: lastUsed.present ? lastUsed.value : this.lastUsed,
  );
  FileRow copyWithCompanion(FilesCompanion data) {
    return FileRow(
      id: data.id.present ? data.id.value : this.id,
      kind: data.kind.present ? data.kind.value : this.kind,
      release: data.release.present ? data.release.value : this.release,
      path: data.path.present ? data.path.value : this.path,
      installPath: data.installPath.present
          ? data.installPath.value
          : this.installPath,
      bytes: data.bytes.present ? data.bytes.value : this.bytes,
      expectedSha256: data.expectedSha256.present
          ? data.expectedSha256.value
          : this.expectedSha256,
      verifiedSha256: data.verifiedSha256.present
          ? data.verifiedSha256.value
          : this.verifiedSha256,
      state: data.state.present ? data.state.value : this.state,
      attempts: data.attempts.present ? data.attempts.value : this.attempts,
      error: data.error.present ? data.error.value : this.error,
      lastUsed: data.lastUsed.present ? data.lastUsed.value : this.lastUsed,
    );
  }

  @override
  String toString() {
    return (StringBuffer('FileRow(')
          ..write('id: $id, ')
          ..write('kind: $kind, ')
          ..write('release: $release, ')
          ..write('path: $path, ')
          ..write('installPath: $installPath, ')
          ..write('bytes: $bytes, ')
          ..write('expectedSha256: $expectedSha256, ')
          ..write('verifiedSha256: $verifiedSha256, ')
          ..write('state: $state, ')
          ..write('attempts: $attempts, ')
          ..write('error: $error, ')
          ..write('lastUsed: $lastUsed')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(
    id,
    kind,
    release,
    path,
    installPath,
    bytes,
    expectedSha256,
    verifiedSha256,
    state,
    attempts,
    error,
    lastUsed,
  );
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is FileRow &&
          other.id == this.id &&
          other.kind == this.kind &&
          other.release == this.release &&
          other.path == this.path &&
          other.installPath == this.installPath &&
          other.bytes == this.bytes &&
          other.expectedSha256 == this.expectedSha256 &&
          other.verifiedSha256 == this.verifiedSha256 &&
          other.state == this.state &&
          other.attempts == this.attempts &&
          other.error == this.error &&
          other.lastUsed == this.lastUsed);
}

class FilesCompanion extends UpdateCompanion<FileRow> {
  final Value<int> id;
  final Value<ReleaseKind> kind;
  final Value<String> release;
  final Value<String> path;
  final Value<String> installPath;
  final Value<int?> bytes;
  final Value<String?> expectedSha256;
  final Value<String?> verifiedSha256;
  final Value<FileState> state;
  final Value<int> attempts;
  final Value<String?> error;
  final Value<DateTime?> lastUsed;
  const FilesCompanion({
    this.id = const Value.absent(),
    this.kind = const Value.absent(),
    this.release = const Value.absent(),
    this.path = const Value.absent(),
    this.installPath = const Value.absent(),
    this.bytes = const Value.absent(),
    this.expectedSha256 = const Value.absent(),
    this.verifiedSha256 = const Value.absent(),
    this.state = const Value.absent(),
    this.attempts = const Value.absent(),
    this.error = const Value.absent(),
    this.lastUsed = const Value.absent(),
  });
  FilesCompanion.insert({
    this.id = const Value.absent(),
    required ReleaseKind kind,
    required String release,
    required String path,
    required String installPath,
    this.bytes = const Value.absent(),
    this.expectedSha256 = const Value.absent(),
    this.verifiedSha256 = const Value.absent(),
    required FileState state,
    this.attempts = const Value.absent(),
    this.error = const Value.absent(),
    this.lastUsed = const Value.absent(),
  }) : kind = Value(kind),
       release = Value(release),
       path = Value(path),
       installPath = Value(installPath),
       state = Value(state);
  static Insertable<FileRow> custom({
    Expression<int>? id,
    Expression<String>? kind,
    Expression<String>? release,
    Expression<String>? path,
    Expression<String>? installPath,
    Expression<int>? bytes,
    Expression<String>? expectedSha256,
    Expression<String>? verifiedSha256,
    Expression<String>? state,
    Expression<int>? attempts,
    Expression<String>? error,
    Expression<DateTime>? lastUsed,
  }) {
    return RawValuesInsertable({
      if (id != null) 'id': id,
      if (kind != null) 'kind': kind,
      if (release != null) 'release': release,
      if (path != null) 'path': path,
      if (installPath != null) 'install_path': installPath,
      if (bytes != null) 'bytes': bytes,
      if (expectedSha256 != null) 'expected_sha256': expectedSha256,
      if (verifiedSha256 != null) 'verified_sha256': verifiedSha256,
      if (state != null) 'state': state,
      if (attempts != null) 'attempts': attempts,
      if (error != null) 'error': error,
      if (lastUsed != null) 'last_used': lastUsed,
    });
  }

  FilesCompanion copyWith({
    Value<int>? id,
    Value<ReleaseKind>? kind,
    Value<String>? release,
    Value<String>? path,
    Value<String>? installPath,
    Value<int?>? bytes,
    Value<String?>? expectedSha256,
    Value<String?>? verifiedSha256,
    Value<FileState>? state,
    Value<int>? attempts,
    Value<String?>? error,
    Value<DateTime?>? lastUsed,
  }) {
    return FilesCompanion(
      id: id ?? this.id,
      kind: kind ?? this.kind,
      release: release ?? this.release,
      path: path ?? this.path,
      installPath: installPath ?? this.installPath,
      bytes: bytes ?? this.bytes,
      expectedSha256: expectedSha256 ?? this.expectedSha256,
      verifiedSha256: verifiedSha256 ?? this.verifiedSha256,
      state: state ?? this.state,
      attempts: attempts ?? this.attempts,
      error: error ?? this.error,
      lastUsed: lastUsed ?? this.lastUsed,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (id.present) {
      map['id'] = Variable<int>(id.value);
    }
    if (kind.present) {
      map['kind'] = Variable<String>(
        $FilesTable.$converterkind.toSql(kind.value),
      );
    }
    if (release.present) {
      map['release'] = Variable<String>(release.value);
    }
    if (path.present) {
      map['path'] = Variable<String>(path.value);
    }
    if (installPath.present) {
      map['install_path'] = Variable<String>(installPath.value);
    }
    if (bytes.present) {
      map['bytes'] = Variable<int>(bytes.value);
    }
    if (expectedSha256.present) {
      map['expected_sha256'] = Variable<String>(expectedSha256.value);
    }
    if (verifiedSha256.present) {
      map['verified_sha256'] = Variable<String>(verifiedSha256.value);
    }
    if (state.present) {
      map['state'] = Variable<String>(
        $FilesTable.$converterstate.toSql(state.value),
      );
    }
    if (attempts.present) {
      map['attempts'] = Variable<int>(attempts.value);
    }
    if (error.present) {
      map['error'] = Variable<String>(error.value);
    }
    if (lastUsed.present) {
      map['last_used'] = Variable<DateTime>(lastUsed.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('FilesCompanion(')
          ..write('id: $id, ')
          ..write('kind: $kind, ')
          ..write('release: $release, ')
          ..write('path: $path, ')
          ..write('installPath: $installPath, ')
          ..write('bytes: $bytes, ')
          ..write('expectedSha256: $expectedSha256, ')
          ..write('verifiedSha256: $verifiedSha256, ')
          ..write('state: $state, ')
          ..write('attempts: $attempts, ')
          ..write('error: $error, ')
          ..write('lastUsed: $lastUsed')
          ..write(')'))
        .toString();
  }
}

class $RegionFilesTable extends RegionFiles
    with TableInfo<$RegionFilesTable, RegionFileRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $RegionFilesTable(this.attachedDatabase, [this._alias]);
  static const VerificationMeta _regionIdMeta = const VerificationMeta(
    'regionId',
  );
  @override
  late final GeneratedColumn<int> regionId = GeneratedColumn<int>(
    'region_id',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'REFERENCES regions (id) ON DELETE CASCADE',
    ),
  );
  static const VerificationMeta _fileIdMeta = const VerificationMeta('fileId');
  @override
  late final GeneratedColumn<int> fileId = GeneratedColumn<int>(
    'file_id',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'REFERENCES files (id) ON DELETE CASCADE',
    ),
  );
  @override
  List<GeneratedColumn> get $columns => [regionId, fileId];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'region_files';
  @override
  VerificationContext validateIntegrity(
    Insertable<RegionFileRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('region_id')) {
      context.handle(
        _regionIdMeta,
        regionId.isAcceptableOrUnknown(data['region_id']!, _regionIdMeta),
      );
    } else if (isInserting) {
      context.missing(_regionIdMeta);
    }
    if (data.containsKey('file_id')) {
      context.handle(
        _fileIdMeta,
        fileId.isAcceptableOrUnknown(data['file_id']!, _fileIdMeta),
      );
    } else if (isInserting) {
      context.missing(_fileIdMeta);
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {regionId, fileId};
  @override
  RegionFileRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return RegionFileRow(
      regionId: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}region_id'],
      )!,
      fileId: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}file_id'],
      )!,
    );
  }

  @override
  $RegionFilesTable createAlias(String alias) {
    return $RegionFilesTable(attachedDatabase, alias);
  }
}

class RegionFileRow extends DataClass implements Insertable<RegionFileRow> {
  final int regionId;
  final int fileId;
  const RegionFileRow({required this.regionId, required this.fileId});
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['region_id'] = Variable<int>(regionId);
    map['file_id'] = Variable<int>(fileId);
    return map;
  }

  RegionFilesCompanion toCompanion(bool nullToAbsent) {
    return RegionFilesCompanion(
      regionId: Value(regionId),
      fileId: Value(fileId),
    );
  }

  factory RegionFileRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return RegionFileRow(
      regionId: serializer.fromJson<int>(json['regionId']),
      fileId: serializer.fromJson<int>(json['fileId']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'regionId': serializer.toJson<int>(regionId),
      'fileId': serializer.toJson<int>(fileId),
    };
  }

  RegionFileRow copyWith({int? regionId, int? fileId}) => RegionFileRow(
    regionId: regionId ?? this.regionId,
    fileId: fileId ?? this.fileId,
  );
  RegionFileRow copyWithCompanion(RegionFilesCompanion data) {
    return RegionFileRow(
      regionId: data.regionId.present ? data.regionId.value : this.regionId,
      fileId: data.fileId.present ? data.fileId.value : this.fileId,
    );
  }

  @override
  String toString() {
    return (StringBuffer('RegionFileRow(')
          ..write('regionId: $regionId, ')
          ..write('fileId: $fileId')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(regionId, fileId);
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is RegionFileRow &&
          other.regionId == this.regionId &&
          other.fileId == this.fileId);
}

class RegionFilesCompanion extends UpdateCompanion<RegionFileRow> {
  final Value<int> regionId;
  final Value<int> fileId;
  final Value<int> rowid;
  const RegionFilesCompanion({
    this.regionId = const Value.absent(),
    this.fileId = const Value.absent(),
    this.rowid = const Value.absent(),
  });
  RegionFilesCompanion.insert({
    required int regionId,
    required int fileId,
    this.rowid = const Value.absent(),
  }) : regionId = Value(regionId),
       fileId = Value(fileId);
  static Insertable<RegionFileRow> custom({
    Expression<int>? regionId,
    Expression<int>? fileId,
    Expression<int>? rowid,
  }) {
    return RawValuesInsertable({
      if (regionId != null) 'region_id': regionId,
      if (fileId != null) 'file_id': fileId,
      if (rowid != null) 'rowid': rowid,
    });
  }

  RegionFilesCompanion copyWith({
    Value<int>? regionId,
    Value<int>? fileId,
    Value<int>? rowid,
  }) {
    return RegionFilesCompanion(
      regionId: regionId ?? this.regionId,
      fileId: fileId ?? this.fileId,
      rowid: rowid ?? this.rowid,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (regionId.present) {
      map['region_id'] = Variable<int>(regionId.value);
    }
    if (fileId.present) {
      map['file_id'] = Variable<int>(fileId.value);
    }
    if (rowid.present) {
      map['rowid'] = Variable<int>(rowid.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('RegionFilesCompanion(')
          ..write('regionId: $regionId, ')
          ..write('fileId: $fileId, ')
          ..write('rowid: $rowid')
          ..write(')'))
        .toString();
  }
}

class $RegionFeedsTable extends RegionFeeds
    with TableInfo<$RegionFeedsTable, RegionFeedRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $RegionFeedsTable(this.attachedDatabase, [this._alias]);
  static const VerificationMeta _regionIdMeta = const VerificationMeta(
    'regionId',
  );
  @override
  late final GeneratedColumn<int> regionId = GeneratedColumn<int>(
    'region_id',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'REFERENCES regions (id) ON DELETE CASCADE',
    ),
  );
  static const VerificationMeta _feedIdMeta = const VerificationMeta('feedId');
  @override
  late final GeneratedColumn<String> feedId = GeneratedColumn<String>(
    'feed_id',
    aliasedName,
    false,
    type: DriftSqlType.string,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _feedIndexMeta = const VerificationMeta(
    'feedIndex',
  );
  @override
  late final GeneratedColumn<int> feedIndex = GeneratedColumn<int>(
    'feed_index',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
  );
  @override
  List<GeneratedColumn> get $columns => [regionId, feedId, feedIndex];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'region_feeds';
  @override
  VerificationContext validateIntegrity(
    Insertable<RegionFeedRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('region_id')) {
      context.handle(
        _regionIdMeta,
        regionId.isAcceptableOrUnknown(data['region_id']!, _regionIdMeta),
      );
    } else if (isInserting) {
      context.missing(_regionIdMeta);
    }
    if (data.containsKey('feed_id')) {
      context.handle(
        _feedIdMeta,
        feedId.isAcceptableOrUnknown(data['feed_id']!, _feedIdMeta),
      );
    } else if (isInserting) {
      context.missing(_feedIdMeta);
    }
    if (data.containsKey('feed_index')) {
      context.handle(
        _feedIndexMeta,
        feedIndex.isAcceptableOrUnknown(data['feed_index']!, _feedIndexMeta),
      );
    } else if (isInserting) {
      context.missing(_feedIndexMeta);
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {regionId, feedId};
  @override
  RegionFeedRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return RegionFeedRow(
      regionId: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}region_id'],
      )!,
      feedId: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}feed_id'],
      )!,
      feedIndex: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}feed_index'],
      )!,
    );
  }

  @override
  $RegionFeedsTable createAlias(String alias) {
    return $RegionFeedsTable(attachedDatabase, alias);
  }
}

class RegionFeedRow extends DataClass implements Insertable<RegionFeedRow> {
  final int regionId;
  final String feedId;

  /// Its index in the transit build, which the manifest orders feeds by.
  final int feedIndex;
  const RegionFeedRow({
    required this.regionId,
    required this.feedId,
    required this.feedIndex,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['region_id'] = Variable<int>(regionId);
    map['feed_id'] = Variable<String>(feedId);
    map['feed_index'] = Variable<int>(feedIndex);
    return map;
  }

  RegionFeedsCompanion toCompanion(bool nullToAbsent) {
    return RegionFeedsCompanion(
      regionId: Value(regionId),
      feedId: Value(feedId),
      feedIndex: Value(feedIndex),
    );
  }

  factory RegionFeedRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return RegionFeedRow(
      regionId: serializer.fromJson<int>(json['regionId']),
      feedId: serializer.fromJson<String>(json['feedId']),
      feedIndex: serializer.fromJson<int>(json['feedIndex']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'regionId': serializer.toJson<int>(regionId),
      'feedId': serializer.toJson<String>(feedId),
      'feedIndex': serializer.toJson<int>(feedIndex),
    };
  }

  RegionFeedRow copyWith({int? regionId, String? feedId, int? feedIndex}) =>
      RegionFeedRow(
        regionId: regionId ?? this.regionId,
        feedId: feedId ?? this.feedId,
        feedIndex: feedIndex ?? this.feedIndex,
      );
  RegionFeedRow copyWithCompanion(RegionFeedsCompanion data) {
    return RegionFeedRow(
      regionId: data.regionId.present ? data.regionId.value : this.regionId,
      feedId: data.feedId.present ? data.feedId.value : this.feedId,
      feedIndex: data.feedIndex.present ? data.feedIndex.value : this.feedIndex,
    );
  }

  @override
  String toString() {
    return (StringBuffer('RegionFeedRow(')
          ..write('regionId: $regionId, ')
          ..write('feedId: $feedId, ')
          ..write('feedIndex: $feedIndex')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(regionId, feedId, feedIndex);
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is RegionFeedRow &&
          other.regionId == this.regionId &&
          other.feedId == this.feedId &&
          other.feedIndex == this.feedIndex);
}

class RegionFeedsCompanion extends UpdateCompanion<RegionFeedRow> {
  final Value<int> regionId;
  final Value<String> feedId;
  final Value<int> feedIndex;
  final Value<int> rowid;
  const RegionFeedsCompanion({
    this.regionId = const Value.absent(),
    this.feedId = const Value.absent(),
    this.feedIndex = const Value.absent(),
    this.rowid = const Value.absent(),
  });
  RegionFeedsCompanion.insert({
    required int regionId,
    required String feedId,
    required int feedIndex,
    this.rowid = const Value.absent(),
  }) : regionId = Value(regionId),
       feedId = Value(feedId),
       feedIndex = Value(feedIndex);
  static Insertable<RegionFeedRow> custom({
    Expression<int>? regionId,
    Expression<String>? feedId,
    Expression<int>? feedIndex,
    Expression<int>? rowid,
  }) {
    return RawValuesInsertable({
      if (regionId != null) 'region_id': regionId,
      if (feedId != null) 'feed_id': feedId,
      if (feedIndex != null) 'feed_index': feedIndex,
      if (rowid != null) 'rowid': rowid,
    });
  }

  RegionFeedsCompanion copyWith({
    Value<int>? regionId,
    Value<String>? feedId,
    Value<int>? feedIndex,
    Value<int>? rowid,
  }) {
    return RegionFeedsCompanion(
      regionId: regionId ?? this.regionId,
      feedId: feedId ?? this.feedId,
      feedIndex: feedIndex ?? this.feedIndex,
      rowid: rowid ?? this.rowid,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (regionId.present) {
      map['region_id'] = Variable<int>(regionId.value);
    }
    if (feedId.present) {
      map['feed_id'] = Variable<String>(feedId.value);
    }
    if (feedIndex.present) {
      map['feed_index'] = Variable<int>(feedIndex.value);
    }
    if (rowid.present) {
      map['rowid'] = Variable<int>(rowid.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('RegionFeedsCompanion(')
          ..write('regionId: $regionId, ')
          ..write('feedId: $feedId, ')
          ..write('feedIndex: $feedIndex, ')
          ..write('rowid: $rowid')
          ..write(')'))
        .toString();
  }
}

class $MapChunksTable extends MapChunks
    with TableInfo<$MapChunksTable, MapChunkRow> {
  @override
  final GeneratedDatabase attachedDatabase;
  final String? _alias;
  $MapChunksTable(this.attachedDatabase, [this._alias]);
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
  static const VerificationMeta _regionIdMeta = const VerificationMeta(
    'regionId',
  );
  @override
  late final GeneratedColumn<int> regionId = GeneratedColumn<int>(
    'region_id',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
    defaultConstraints: GeneratedColumn.constraintIsAlways(
      'REFERENCES regions (id) ON DELETE CASCADE',
    ),
  );
  static const VerificationMeta _startMeta = const VerificationMeta('start');
  @override
  late final GeneratedColumn<int> start = GeneratedColumn<int>(
    'start',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _lengthMeta = const VerificationMeta('length');
  @override
  late final GeneratedColumn<int> length = GeneratedColumn<int>(
    'length',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _tileCountMeta = const VerificationMeta(
    'tileCount',
  );
  @override
  late final GeneratedColumn<int> tileCount = GeneratedColumn<int>(
    'tile_count',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: true,
  );
  static const VerificationMeta _tilesMeta = const VerificationMeta('tiles');
  @override
  late final GeneratedColumn<Uint8List> tiles = GeneratedColumn<Uint8List>(
    'tiles',
    aliasedName,
    false,
    type: DriftSqlType.blob,
    requiredDuringInsert: true,
  );
  @override
  late final GeneratedColumnWithTypeConverter<ChunkState, String> state =
      GeneratedColumn<String>(
        'state',
        aliasedName,
        false,
        type: DriftSqlType.string,
        requiredDuringInsert: true,
      ).withConverter<ChunkState>($MapChunksTable.$converterstate);
  static const VerificationMeta _attemptsMeta = const VerificationMeta(
    'attempts',
  );
  @override
  late final GeneratedColumn<int> attempts = GeneratedColumn<int>(
    'attempts',
    aliasedName,
    false,
    type: DriftSqlType.int,
    requiredDuringInsert: false,
    defaultValue: const Constant(0),
  );
  static const VerificationMeta _errorMeta = const VerificationMeta('error');
  @override
  late final GeneratedColumn<String> error = GeneratedColumn<String>(
    'error',
    aliasedName,
    true,
    type: DriftSqlType.string,
    requiredDuringInsert: false,
  );
  @override
  List<GeneratedColumn> get $columns => [
    id,
    regionId,
    start,
    length,
    tileCount,
    tiles,
    state,
    attempts,
    error,
  ];
  @override
  String get aliasedName => _alias ?? actualTableName;
  @override
  String get actualTableName => $name;
  static const String $name = 'map_chunks';
  @override
  VerificationContext validateIntegrity(
    Insertable<MapChunkRow> instance, {
    bool isInserting = false,
  }) {
    final context = VerificationContext();
    final data = instance.toColumns(true);
    if (data.containsKey('id')) {
      context.handle(_idMeta, id.isAcceptableOrUnknown(data['id']!, _idMeta));
    }
    if (data.containsKey('region_id')) {
      context.handle(
        _regionIdMeta,
        regionId.isAcceptableOrUnknown(data['region_id']!, _regionIdMeta),
      );
    } else if (isInserting) {
      context.missing(_regionIdMeta);
    }
    if (data.containsKey('start')) {
      context.handle(
        _startMeta,
        start.isAcceptableOrUnknown(data['start']!, _startMeta),
      );
    } else if (isInserting) {
      context.missing(_startMeta);
    }
    if (data.containsKey('length')) {
      context.handle(
        _lengthMeta,
        length.isAcceptableOrUnknown(data['length']!, _lengthMeta),
      );
    } else if (isInserting) {
      context.missing(_lengthMeta);
    }
    if (data.containsKey('tile_count')) {
      context.handle(
        _tileCountMeta,
        tileCount.isAcceptableOrUnknown(data['tile_count']!, _tileCountMeta),
      );
    } else if (isInserting) {
      context.missing(_tileCountMeta);
    }
    if (data.containsKey('tiles')) {
      context.handle(
        _tilesMeta,
        tiles.isAcceptableOrUnknown(data['tiles']!, _tilesMeta),
      );
    } else if (isInserting) {
      context.missing(_tilesMeta);
    }
    if (data.containsKey('attempts')) {
      context.handle(
        _attemptsMeta,
        attempts.isAcceptableOrUnknown(data['attempts']!, _attemptsMeta),
      );
    }
    if (data.containsKey('error')) {
      context.handle(
        _errorMeta,
        error.isAcceptableOrUnknown(data['error']!, _errorMeta),
      );
    }
    return context;
  }

  @override
  Set<GeneratedColumn> get $primaryKey => {id};
  @override
  MapChunkRow map(Map<String, dynamic> data, {String? tablePrefix}) {
    final effectivePrefix = tablePrefix != null ? '$tablePrefix.' : '';
    return MapChunkRow(
      id: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}id'],
      )!,
      regionId: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}region_id'],
      )!,
      start: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}start'],
      )!,
      length: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}length'],
      )!,
      tileCount: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}tile_count'],
      )!,
      tiles: attachedDatabase.typeMapping.read(
        DriftSqlType.blob,
        data['${effectivePrefix}tiles'],
      )!,
      state: $MapChunksTable.$converterstate.fromSql(
        attachedDatabase.typeMapping.read(
          DriftSqlType.string,
          data['${effectivePrefix}state'],
        )!,
      ),
      attempts: attachedDatabase.typeMapping.read(
        DriftSqlType.int,
        data['${effectivePrefix}attempts'],
      )!,
      error: attachedDatabase.typeMapping.read(
        DriftSqlType.string,
        data['${effectivePrefix}error'],
      ),
    );
  }

  @override
  $MapChunksTable createAlias(String alias) {
    return $MapChunksTable(attachedDatabase, alias);
  }

  static JsonTypeConverter2<ChunkState, String, String> $converterstate =
      const EnumNameConverter<ChunkState>(ChunkState.values);
}

class MapChunkRow extends DataClass implements Insertable<MapChunkRow> {
  final int id;
  final int regionId;

  /// From the start of the archive.
  final int start;
  final int length;
  final int tileCount;

  /// The tiles in it (opaque bytes from the planner).
  final Uint8List tiles;
  final ChunkState state;
  final int attempts;
  final String? error;
  const MapChunkRow({
    required this.id,
    required this.regionId,
    required this.start,
    required this.length,
    required this.tileCount,
    required this.tiles,
    required this.state,
    required this.attempts,
    this.error,
  });
  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    map['id'] = Variable<int>(id);
    map['region_id'] = Variable<int>(regionId);
    map['start'] = Variable<int>(start);
    map['length'] = Variable<int>(length);
    map['tile_count'] = Variable<int>(tileCount);
    map['tiles'] = Variable<Uint8List>(tiles);
    {
      map['state'] = Variable<String>(
        $MapChunksTable.$converterstate.toSql(state),
      );
    }
    map['attempts'] = Variable<int>(attempts);
    if (!nullToAbsent || error != null) {
      map['error'] = Variable<String>(error);
    }
    return map;
  }

  MapChunksCompanion toCompanion(bool nullToAbsent) {
    return MapChunksCompanion(
      id: Value(id),
      regionId: Value(regionId),
      start: Value(start),
      length: Value(length),
      tileCount: Value(tileCount),
      tiles: Value(tiles),
      state: Value(state),
      attempts: Value(attempts),
      error: error == null && nullToAbsent
          ? const Value.absent()
          : Value(error),
    );
  }

  factory MapChunkRow.fromJson(
    Map<String, dynamic> json, {
    ValueSerializer? serializer,
  }) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return MapChunkRow(
      id: serializer.fromJson<int>(json['id']),
      regionId: serializer.fromJson<int>(json['regionId']),
      start: serializer.fromJson<int>(json['start']),
      length: serializer.fromJson<int>(json['length']),
      tileCount: serializer.fromJson<int>(json['tileCount']),
      tiles: serializer.fromJson<Uint8List>(json['tiles']),
      state: $MapChunksTable.$converterstate.fromJson(
        serializer.fromJson<String>(json['state']),
      ),
      attempts: serializer.fromJson<int>(json['attempts']),
      error: serializer.fromJson<String?>(json['error']),
    );
  }
  @override
  Map<String, dynamic> toJson({ValueSerializer? serializer}) {
    serializer ??= driftRuntimeOptions.defaultSerializer;
    return <String, dynamic>{
      'id': serializer.toJson<int>(id),
      'regionId': serializer.toJson<int>(regionId),
      'start': serializer.toJson<int>(start),
      'length': serializer.toJson<int>(length),
      'tileCount': serializer.toJson<int>(tileCount),
      'tiles': serializer.toJson<Uint8List>(tiles),
      'state': serializer.toJson<String>(
        $MapChunksTable.$converterstate.toJson(state),
      ),
      'attempts': serializer.toJson<int>(attempts),
      'error': serializer.toJson<String?>(error),
    };
  }

  MapChunkRow copyWith({
    int? id,
    int? regionId,
    int? start,
    int? length,
    int? tileCount,
    Uint8List? tiles,
    ChunkState? state,
    int? attempts,
    Value<String?> error = const Value.absent(),
  }) => MapChunkRow(
    id: id ?? this.id,
    regionId: regionId ?? this.regionId,
    start: start ?? this.start,
    length: length ?? this.length,
    tileCount: tileCount ?? this.tileCount,
    tiles: tiles ?? this.tiles,
    state: state ?? this.state,
    attempts: attempts ?? this.attempts,
    error: error.present ? error.value : this.error,
  );
  MapChunkRow copyWithCompanion(MapChunksCompanion data) {
    return MapChunkRow(
      id: data.id.present ? data.id.value : this.id,
      regionId: data.regionId.present ? data.regionId.value : this.regionId,
      start: data.start.present ? data.start.value : this.start,
      length: data.length.present ? data.length.value : this.length,
      tileCount: data.tileCount.present ? data.tileCount.value : this.tileCount,
      tiles: data.tiles.present ? data.tiles.value : this.tiles,
      state: data.state.present ? data.state.value : this.state,
      attempts: data.attempts.present ? data.attempts.value : this.attempts,
      error: data.error.present ? data.error.value : this.error,
    );
  }

  @override
  String toString() {
    return (StringBuffer('MapChunkRow(')
          ..write('id: $id, ')
          ..write('regionId: $regionId, ')
          ..write('start: $start, ')
          ..write('length: $length, ')
          ..write('tileCount: $tileCount, ')
          ..write('tiles: $tiles, ')
          ..write('state: $state, ')
          ..write('attempts: $attempts, ')
          ..write('error: $error')
          ..write(')'))
        .toString();
  }

  @override
  int get hashCode => Object.hash(
    id,
    regionId,
    start,
    length,
    tileCount,
    $driftBlobEquality.hash(tiles),
    state,
    attempts,
    error,
  );
  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      (other is MapChunkRow &&
          other.id == this.id &&
          other.regionId == this.regionId &&
          other.start == this.start &&
          other.length == this.length &&
          other.tileCount == this.tileCount &&
          $driftBlobEquality.equals(other.tiles, this.tiles) &&
          other.state == this.state &&
          other.attempts == this.attempts &&
          other.error == this.error);
}

class MapChunksCompanion extends UpdateCompanion<MapChunkRow> {
  final Value<int> id;
  final Value<int> regionId;
  final Value<int> start;
  final Value<int> length;
  final Value<int> tileCount;
  final Value<Uint8List> tiles;
  final Value<ChunkState> state;
  final Value<int> attempts;
  final Value<String?> error;
  const MapChunksCompanion({
    this.id = const Value.absent(),
    this.regionId = const Value.absent(),
    this.start = const Value.absent(),
    this.length = const Value.absent(),
    this.tileCount = const Value.absent(),
    this.tiles = const Value.absent(),
    this.state = const Value.absent(),
    this.attempts = const Value.absent(),
    this.error = const Value.absent(),
  });
  MapChunksCompanion.insert({
    this.id = const Value.absent(),
    required int regionId,
    required int start,
    required int length,
    required int tileCount,
    required Uint8List tiles,
    required ChunkState state,
    this.attempts = const Value.absent(),
    this.error = const Value.absent(),
  }) : regionId = Value(regionId),
       start = Value(start),
       length = Value(length),
       tileCount = Value(tileCount),
       tiles = Value(tiles),
       state = Value(state);
  static Insertable<MapChunkRow> custom({
    Expression<int>? id,
    Expression<int>? regionId,
    Expression<int>? start,
    Expression<int>? length,
    Expression<int>? tileCount,
    Expression<Uint8List>? tiles,
    Expression<String>? state,
    Expression<int>? attempts,
    Expression<String>? error,
  }) {
    return RawValuesInsertable({
      if (id != null) 'id': id,
      if (regionId != null) 'region_id': regionId,
      if (start != null) 'start': start,
      if (length != null) 'length': length,
      if (tileCount != null) 'tile_count': tileCount,
      if (tiles != null) 'tiles': tiles,
      if (state != null) 'state': state,
      if (attempts != null) 'attempts': attempts,
      if (error != null) 'error': error,
    });
  }

  MapChunksCompanion copyWith({
    Value<int>? id,
    Value<int>? regionId,
    Value<int>? start,
    Value<int>? length,
    Value<int>? tileCount,
    Value<Uint8List>? tiles,
    Value<ChunkState>? state,
    Value<int>? attempts,
    Value<String?>? error,
  }) {
    return MapChunksCompanion(
      id: id ?? this.id,
      regionId: regionId ?? this.regionId,
      start: start ?? this.start,
      length: length ?? this.length,
      tileCount: tileCount ?? this.tileCount,
      tiles: tiles ?? this.tiles,
      state: state ?? this.state,
      attempts: attempts ?? this.attempts,
      error: error ?? this.error,
    );
  }

  @override
  Map<String, Expression> toColumns(bool nullToAbsent) {
    final map = <String, Expression>{};
    if (id.present) {
      map['id'] = Variable<int>(id.value);
    }
    if (regionId.present) {
      map['region_id'] = Variable<int>(regionId.value);
    }
    if (start.present) {
      map['start'] = Variable<int>(start.value);
    }
    if (length.present) {
      map['length'] = Variable<int>(length.value);
    }
    if (tileCount.present) {
      map['tile_count'] = Variable<int>(tileCount.value);
    }
    if (tiles.present) {
      map['tiles'] = Variable<Uint8List>(tiles.value);
    }
    if (state.present) {
      map['state'] = Variable<String>(
        $MapChunksTable.$converterstate.toSql(state.value),
      );
    }
    if (attempts.present) {
      map['attempts'] = Variable<int>(attempts.value);
    }
    if (error.present) {
      map['error'] = Variable<String>(error.value);
    }
    return map;
  }

  @override
  String toString() {
    return (StringBuffer('MapChunksCompanion(')
          ..write('id: $id, ')
          ..write('regionId: $regionId, ')
          ..write('start: $start, ')
          ..write('length: $length, ')
          ..write('tileCount: $tileCount, ')
          ..write('tiles: $tiles, ')
          ..write('state: $state, ')
          ..write('attempts: $attempts, ')
          ..write('error: $error')
          ..write(')'))
        .toString();
  }
}

abstract class _$OfflineDatabase extends GeneratedDatabase {
  _$OfflineDatabase(QueryExecutor e) : super(e);
  $OfflineDatabaseManager get managers => $OfflineDatabaseManager(this);
  late final $ReleasesTable releases = $ReleasesTable(this);
  late final $RegionsTable regions = $RegionsTable(this);
  late final $FilesTable files = $FilesTable(this);
  late final $RegionFilesTable regionFiles = $RegionFilesTable(this);
  late final $RegionFeedsTable regionFeeds = $RegionFeedsTable(this);
  late final $MapChunksTable mapChunks = $MapChunksTable(this);
  late final Index filesState = Index(
    'files_state',
    'CREATE INDEX files_state ON files (state)',
  );
  late final Index regionFilesFile = Index(
    'region_files_file',
    'CREATE INDEX region_files_file ON region_files (file_id)',
  );
  @override
  Iterable<TableInfo<Table, Object?>> get allTables =>
      allSchemaEntities.whereType<TableInfo<Table, Object?>>();
  @override
  List<DatabaseSchemaEntity> get allSchemaEntities => [
    releases,
    regions,
    files,
    regionFiles,
    regionFeeds,
    mapChunks,
    filesState,
    regionFilesFile,
  ];
  @override
  StreamQueryUpdateRules get streamUpdateRules => const StreamQueryUpdateRules([
    WritePropagation(
      on: TableUpdateQuery.onTableName(
        'regions',
        limitUpdateKind: UpdateKind.delete,
      ),
      result: [TableUpdate('region_files', kind: UpdateKind.delete)],
    ),
    WritePropagation(
      on: TableUpdateQuery.onTableName(
        'files',
        limitUpdateKind: UpdateKind.delete,
      ),
      result: [TableUpdate('region_files', kind: UpdateKind.delete)],
    ),
    WritePropagation(
      on: TableUpdateQuery.onTableName(
        'regions',
        limitUpdateKind: UpdateKind.delete,
      ),
      result: [TableUpdate('region_feeds', kind: UpdateKind.delete)],
    ),
    WritePropagation(
      on: TableUpdateQuery.onTableName(
        'regions',
        limitUpdateKind: UpdateKind.delete,
      ),
      result: [TableUpdate('map_chunks', kind: UpdateKind.delete)],
    ),
  ]);
}

typedef $$ReleasesTableCreateCompanionBuilder = ReleasesCompanion Function({
  required ReleaseKind kind,
  required String date,
  Value<int?> valhallaBuildId,
  Value<String?> etag,
  Value<String?> manifestId,
  required DateTime pinnedAt,
  Value<int> rowid,
});
typedef $$ReleasesTableUpdateCompanionBuilder = ReleasesCompanion Function({
  Value<ReleaseKind> kind,
  Value<String> date,
  Value<int?> valhallaBuildId,
  Value<String?> etag,
  Value<String?> manifestId,
  Value<DateTime> pinnedAt,
  Value<int> rowid,
});

class $$ReleasesTableFilterComposer
    extends Composer<_$OfflineDatabase, $ReleasesTable> {
  $$ReleasesTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnWithTypeConverterFilters<ReleaseKind, ReleaseKind, String> get kind =>
      $composableBuilder(
        column: $table.kind,
        builder: (column) => ColumnWithTypeConverterFilters(column),
      );

  ColumnFilters<String> get date => $composableBuilder(
    column: $table.date,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get valhallaBuildId => $composableBuilder(
    column: $table.valhallaBuildId,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get etag => $composableBuilder(
    column: $table.etag,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get manifestId => $composableBuilder(
    column: $table.manifestId,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get pinnedAt => $composableBuilder(
    column: $table.pinnedAt,
    builder: (column) => ColumnFilters(column),
  );
}

class $$ReleasesTableOrderingComposer
    extends Composer<_$OfflineDatabase, $ReleasesTable> {
  $$ReleasesTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<String> get kind => $composableBuilder(
    column: $table.kind,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get date => $composableBuilder(
    column: $table.date,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get valhallaBuildId => $composableBuilder(
    column: $table.valhallaBuildId,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get etag => $composableBuilder(
    column: $table.etag,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get manifestId => $composableBuilder(
    column: $table.manifestId,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get pinnedAt => $composableBuilder(
    column: $table.pinnedAt,
    builder: (column) => ColumnOrderings(column),
  );
}

class $$ReleasesTableAnnotationComposer
    extends Composer<_$OfflineDatabase, $ReleasesTable> {
  $$ReleasesTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumnWithTypeConverter<ReleaseKind, String> get kind =>
      $composableBuilder(column: $table.kind, builder: (column) => column);

  GeneratedColumn<String> get date =>
      $composableBuilder(column: $table.date, builder: (column) => column);

  GeneratedColumn<int> get valhallaBuildId => $composableBuilder(
    column: $table.valhallaBuildId,
    builder: (column) => column,
  );

  GeneratedColumn<String> get etag =>
      $composableBuilder(column: $table.etag, builder: (column) => column);

  GeneratedColumn<String> get manifestId => $composableBuilder(
    column: $table.manifestId,
    builder: (column) => column,
  );

  GeneratedColumn<DateTime> get pinnedAt =>
      $composableBuilder(column: $table.pinnedAt, builder: (column) => column);
}

class $$ReleasesTableTableManager
    extends
        RootTableManager<
          _$OfflineDatabase,
          $ReleasesTable,
          ReleaseRow,
          $$ReleasesTableFilterComposer,
          $$ReleasesTableOrderingComposer,
          $$ReleasesTableAnnotationComposer,
          $$ReleasesTableCreateCompanionBuilder,
          $$ReleasesTableUpdateCompanionBuilder,
          (
            ReleaseRow,
            BaseReferences<_$OfflineDatabase, $ReleasesTable, ReleaseRow>,
          ),
          ReleaseRow,
          PrefetchHooks Function()
        > {
  $$ReleasesTableTableManager(_$OfflineDatabase db, $ReleasesTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$ReleasesTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$ReleasesTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$ReleasesTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<ReleaseKind> kind = const Value.absent(),
                Value<String> date = const Value.absent(),
                Value<int?> valhallaBuildId = const Value.absent(),
                Value<String?> etag = const Value.absent(),
                Value<String?> manifestId = const Value.absent(),
                Value<DateTime> pinnedAt = const Value.absent(),
                Value<int> rowid = const Value.absent(),
              }) => ReleasesCompanion(
                kind: kind,
                date: date,
                valhallaBuildId: valhallaBuildId,
                etag: etag,
                manifestId: manifestId,
                pinnedAt: pinnedAt,
                rowid: rowid,
              ),
          createCompanionCallback:
              ({
                required ReleaseKind kind,
                required String date,
                Value<int?> valhallaBuildId = const Value.absent(),
                Value<String?> etag = const Value.absent(),
                Value<String?> manifestId = const Value.absent(),
                required DateTime pinnedAt,
                Value<int> rowid = const Value.absent(),
              }) => ReleasesCompanion.insert(
                kind: kind,
                date: date,
                valhallaBuildId: valhallaBuildId,
                etag: etag,
                manifestId: manifestId,
                pinnedAt: pinnedAt,
                rowid: rowid,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$ReleasesTable, ReleaseRow>(table),
                  BaseReferences<_$OfflineDatabase, $ReleasesTable, ReleaseRow>(
                    db,
                    table,
                    e,
                  ),
                ),
              )
              .toList(),
          prefetchHooksCallback: null,
        ),
      );
}

typedef $$ReleasesTableProcessedTableManager =
    ProcessedTableManager<
      _$OfflineDatabase,
      $ReleasesTable,
      ReleaseRow,
      $$ReleasesTableFilterComposer,
      $$ReleasesTableOrderingComposer,
      $$ReleasesTableAnnotationComposer,
      $$ReleasesTableCreateCompanionBuilder,
      $$ReleasesTableUpdateCompanionBuilder,
      (
        ReleaseRow,
        BaseReferences<_$OfflineDatabase, $ReleasesTable, ReleaseRow>,
      ),
      ReleaseRow,
      PrefetchHooks Function()
    >;
typedef $$RegionsTableCreateCompanionBuilder = RegionsCompanion Function({
  Value<int> id,
  required RegionKind kind,
  Value<String?> name,
  Value<String?> polygon,
  Value<String?> mapRelease,
  Value<String?> routingRelease,
  Value<String?> transitRelease,
  required RegionState state,
  Value<String?> error,
  Value<Uint8List?> mapTiles,
  Value<int> mapBytes,
  required DateTime createdAt,
  required DateTime updatedAt,
});
typedef $$RegionsTableUpdateCompanionBuilder = RegionsCompanion Function({
  Value<int> id,
  Value<RegionKind> kind,
  Value<String?> name,
  Value<String?> polygon,
  Value<String?> mapRelease,
  Value<String?> routingRelease,
  Value<String?> transitRelease,
  Value<RegionState> state,
  Value<String?> error,
  Value<Uint8List?> mapTiles,
  Value<int> mapBytes,
  Value<DateTime> createdAt,
  Value<DateTime> updatedAt,
});

final class $$RegionsTableReferences
    extends BaseReferences<_$OfflineDatabase, $RegionsTable, RegionRow> {
  $$RegionsTableReferences(super.$_db, super.$_table, super.$_typedResult);

  static MultiTypedResultKey<$RegionFilesTable, List<RegionFileRow>>
  _regionFilesRefsTable(_$OfflineDatabase db) => MultiTypedResultKey.fromTable(
    db.regionFiles,
    aliasName: 'regions__id__region_files__region_id',
  );

  $$RegionFilesTableProcessedTableManager get regionFilesRefs {
    final manager = $$RegionFilesTableTableManager(
      $_db,
      $_db.regionFiles,
    ).filter((f) => f.regionId.id.sqlEquals($_itemColumn<int>('id')!));

    final cache = $_typedResult.readTableOrNull(_regionFilesRefsTable($_db));
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: cache),
    );
  }

  static MultiTypedResultKey<$RegionFeedsTable, List<RegionFeedRow>>
  _regionFeedsRefsTable(_$OfflineDatabase db) => MultiTypedResultKey.fromTable(
    db.regionFeeds,
    aliasName: 'regions__id__region_feeds__region_id',
  );

  $$RegionFeedsTableProcessedTableManager get regionFeedsRefs {
    final manager = $$RegionFeedsTableTableManager(
      $_db,
      $_db.regionFeeds,
    ).filter((f) => f.regionId.id.sqlEquals($_itemColumn<int>('id')!));

    final cache = $_typedResult.readTableOrNull(_regionFeedsRefsTable($_db));
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: cache),
    );
  }

  static MultiTypedResultKey<$MapChunksTable, List<MapChunkRow>>
  _mapChunksRefsTable(_$OfflineDatabase db) => MultiTypedResultKey.fromTable(
    db.mapChunks,
    aliasName: 'regions__id__map_chunks__region_id',
  );

  $$MapChunksTableProcessedTableManager get mapChunksRefs {
    final manager = $$MapChunksTableTableManager(
      $_db,
      $_db.mapChunks,
    ).filter((f) => f.regionId.id.sqlEquals($_itemColumn<int>('id')!));

    final cache = $_typedResult.readTableOrNull(_mapChunksRefsTable($_db));
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: cache),
    );
  }
}

class $$RegionsTableFilterComposer
    extends Composer<_$OfflineDatabase, $RegionsTable> {
  $$RegionsTableFilterComposer({
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

  ColumnWithTypeConverterFilters<RegionKind, RegionKind, String> get kind =>
      $composableBuilder(
        column: $table.kind,
        builder: (column) => ColumnWithTypeConverterFilters(column),
      );

  ColumnFilters<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get polygon => $composableBuilder(
    column: $table.polygon,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get mapRelease => $composableBuilder(
    column: $table.mapRelease,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get routingRelease => $composableBuilder(
    column: $table.routingRelease,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get transitRelease => $composableBuilder(
    column: $table.transitRelease,
    builder: (column) => ColumnFilters(column),
  );

  ColumnWithTypeConverterFilters<RegionState, RegionState, String> get state =>
      $composableBuilder(
        column: $table.state,
        builder: (column) => ColumnWithTypeConverterFilters(column),
      );

  ColumnFilters<String> get error => $composableBuilder(
    column: $table.error,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<Uint8List> get mapTiles => $composableBuilder(
    column: $table.mapTiles,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get mapBytes => $composableBuilder(
    column: $table.mapBytes,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get createdAt => $composableBuilder(
    column: $table.createdAt,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get updatedAt => $composableBuilder(
    column: $table.updatedAt,
    builder: (column) => ColumnFilters(column),
  );

  Expression<bool> regionFilesRefs(
    Expression<bool> Function($$RegionFilesTableFilterComposer f) f,
  ) {
    final $$RegionFilesTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.regionFiles,
      getReferencedColumn: (t) => t.regionId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionFilesTableFilterComposer(
            $db: $db,
            $table: $db.regionFiles,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }

  Expression<bool> regionFeedsRefs(
    Expression<bool> Function($$RegionFeedsTableFilterComposer f) f,
  ) {
    final $$RegionFeedsTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.regionFeeds,
      getReferencedColumn: (t) => t.regionId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionFeedsTableFilterComposer(
            $db: $db,
            $table: $db.regionFeeds,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }

  Expression<bool> mapChunksRefs(
    Expression<bool> Function($$MapChunksTableFilterComposer f) f,
  ) {
    final $$MapChunksTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.mapChunks,
      getReferencedColumn: (t) => t.regionId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$MapChunksTableFilterComposer(
            $db: $db,
            $table: $db.mapChunks,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }
}

class $$RegionsTableOrderingComposer
    extends Composer<_$OfflineDatabase, $RegionsTable> {
  $$RegionsTableOrderingComposer({
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

  ColumnOrderings<String> get kind => $composableBuilder(
    column: $table.kind,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get name => $composableBuilder(
    column: $table.name,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get polygon => $composableBuilder(
    column: $table.polygon,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get mapRelease => $composableBuilder(
    column: $table.mapRelease,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get routingRelease => $composableBuilder(
    column: $table.routingRelease,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get transitRelease => $composableBuilder(
    column: $table.transitRelease,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get state => $composableBuilder(
    column: $table.state,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get error => $composableBuilder(
    column: $table.error,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<Uint8List> get mapTiles => $composableBuilder(
    column: $table.mapTiles,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get mapBytes => $composableBuilder(
    column: $table.mapBytes,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get createdAt => $composableBuilder(
    column: $table.createdAt,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get updatedAt => $composableBuilder(
    column: $table.updatedAt,
    builder: (column) => ColumnOrderings(column),
  );
}

class $$RegionsTableAnnotationComposer
    extends Composer<_$OfflineDatabase, $RegionsTable> {
  $$RegionsTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<int> get id =>
      $composableBuilder(column: $table.id, builder: (column) => column);

  GeneratedColumnWithTypeConverter<RegionKind, String> get kind =>
      $composableBuilder(column: $table.kind, builder: (column) => column);

  GeneratedColumn<String> get name =>
      $composableBuilder(column: $table.name, builder: (column) => column);

  GeneratedColumn<String> get polygon =>
      $composableBuilder(column: $table.polygon, builder: (column) => column);

  GeneratedColumn<String> get mapRelease => $composableBuilder(
    column: $table.mapRelease,
    builder: (column) => column,
  );

  GeneratedColumn<String> get routingRelease => $composableBuilder(
    column: $table.routingRelease,
    builder: (column) => column,
  );

  GeneratedColumn<String> get transitRelease => $composableBuilder(
    column: $table.transitRelease,
    builder: (column) => column,
  );

  GeneratedColumnWithTypeConverter<RegionState, String> get state =>
      $composableBuilder(column: $table.state, builder: (column) => column);

  GeneratedColumn<String> get error =>
      $composableBuilder(column: $table.error, builder: (column) => column);

  GeneratedColumn<Uint8List> get mapTiles =>
      $composableBuilder(column: $table.mapTiles, builder: (column) => column);

  GeneratedColumn<int> get mapBytes =>
      $composableBuilder(column: $table.mapBytes, builder: (column) => column);

  GeneratedColumn<DateTime> get createdAt =>
      $composableBuilder(column: $table.createdAt, builder: (column) => column);

  GeneratedColumn<DateTime> get updatedAt =>
      $composableBuilder(column: $table.updatedAt, builder: (column) => column);

  Expression<T> regionFilesRefs<T extends Object>(
    Expression<T> Function($$RegionFilesTableAnnotationComposer a) f,
  ) {
    final $$RegionFilesTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.regionFiles,
      getReferencedColumn: (t) => t.regionId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionFilesTableAnnotationComposer(
            $db: $db,
            $table: $db.regionFiles,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }

  Expression<T> regionFeedsRefs<T extends Object>(
    Expression<T> Function($$RegionFeedsTableAnnotationComposer a) f,
  ) {
    final $$RegionFeedsTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.regionFeeds,
      getReferencedColumn: (t) => t.regionId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionFeedsTableAnnotationComposer(
            $db: $db,
            $table: $db.regionFeeds,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }

  Expression<T> mapChunksRefs<T extends Object>(
    Expression<T> Function($$MapChunksTableAnnotationComposer a) f,
  ) {
    final $$MapChunksTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.mapChunks,
      getReferencedColumn: (t) => t.regionId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$MapChunksTableAnnotationComposer(
            $db: $db,
            $table: $db.mapChunks,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }
}

class $$RegionsTableTableManager
    extends
        RootTableManager<
          _$OfflineDatabase,
          $RegionsTable,
          RegionRow,
          $$RegionsTableFilterComposer,
          $$RegionsTableOrderingComposer,
          $$RegionsTableAnnotationComposer,
          $$RegionsTableCreateCompanionBuilder,
          $$RegionsTableUpdateCompanionBuilder,
          (RegionRow, $$RegionsTableReferences),
          RegionRow,
          PrefetchHooks Function({
            bool regionFilesRefs,
            bool regionFeedsRefs,
            bool mapChunksRefs,
          })
        > {
  $$RegionsTableTableManager(_$OfflineDatabase db, $RegionsTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$RegionsTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$RegionsTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$RegionsTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                Value<RegionKind> kind = const Value.absent(),
                Value<String?> name = const Value.absent(),
                Value<String?> polygon = const Value.absent(),
                Value<String?> mapRelease = const Value.absent(),
                Value<String?> routingRelease = const Value.absent(),
                Value<String?> transitRelease = const Value.absent(),
                Value<RegionState> state = const Value.absent(),
                Value<String?> error = const Value.absent(),
                Value<Uint8List?> mapTiles = const Value.absent(),
                Value<int> mapBytes = const Value.absent(),
                Value<DateTime> createdAt = const Value.absent(),
                Value<DateTime> updatedAt = const Value.absent(),
              }) => RegionsCompanion(
                id: id,
                kind: kind,
                name: name,
                polygon: polygon,
                mapRelease: mapRelease,
                routingRelease: routingRelease,
                transitRelease: transitRelease,
                state: state,
                error: error,
                mapTiles: mapTiles,
                mapBytes: mapBytes,
                createdAt: createdAt,
                updatedAt: updatedAt,
              ),
          createCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                required RegionKind kind,
                Value<String?> name = const Value.absent(),
                Value<String?> polygon = const Value.absent(),
                Value<String?> mapRelease = const Value.absent(),
                Value<String?> routingRelease = const Value.absent(),
                Value<String?> transitRelease = const Value.absent(),
                required RegionState state,
                Value<String?> error = const Value.absent(),
                Value<Uint8List?> mapTiles = const Value.absent(),
                Value<int> mapBytes = const Value.absent(),
                required DateTime createdAt,
                required DateTime updatedAt,
              }) => RegionsCompanion.insert(
                id: id,
                kind: kind,
                name: name,
                polygon: polygon,
                mapRelease: mapRelease,
                routingRelease: routingRelease,
                transitRelease: transitRelease,
                state: state,
                error: error,
                mapTiles: mapTiles,
                mapBytes: mapBytes,
                createdAt: createdAt,
                updatedAt: updatedAt,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$RegionsTable, RegionRow>(table),
                  $$RegionsTableReferences(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback:
              ({
                regionFilesRefs = false,
                regionFeedsRefs = false,
                mapChunksRefs = false,
              }) {
                return PrefetchHooks(
                  db: db,
                  explicitlyWatchedTables: [
                    if (regionFilesRefs) db.regionFiles,
                    if (regionFeedsRefs) db.regionFeeds,
                    if (mapChunksRefs) db.mapChunks,
                  ],
                  addJoins: null,
                  getPrefetchedDataCallback: (items) async {
                    return [
                      if (regionFilesRefs)
                        await $_getPrefetchedDataByColumns<
                          RegionRow,
                          $RegionsTable,
                          RegionFileRow,
                          $RegionFilesTable
                        >(
                          currentTable: table,
                          referencedTable: $$RegionsTableReferences
                              ._regionFilesRefsTable(db),
                          managerFromTypedResult: (p0) =>
                              $$RegionsTableReferences(
                                db,
                                table,
                                p0,
                              ).regionFilesRefs,
                          currentColumn: 'id',
                          referencedColumn: 'region_id',
                          typedResults: items,
                        ),
                      if (regionFeedsRefs)
                        await $_getPrefetchedDataByColumns<
                          RegionRow,
                          $RegionsTable,
                          RegionFeedRow,
                          $RegionFeedsTable
                        >(
                          currentTable: table,
                          referencedTable: $$RegionsTableReferences
                              ._regionFeedsRefsTable(db),
                          managerFromTypedResult: (p0) =>
                              $$RegionsTableReferences(
                                db,
                                table,
                                p0,
                              ).regionFeedsRefs,
                          currentColumn: 'id',
                          referencedColumn: 'region_id',
                          typedResults: items,
                        ),
                      if (mapChunksRefs)
                        await $_getPrefetchedDataByColumns<
                          RegionRow,
                          $RegionsTable,
                          MapChunkRow,
                          $MapChunksTable
                        >(
                          currentTable: table,
                          referencedTable: $$RegionsTableReferences
                              ._mapChunksRefsTable(db),
                          managerFromTypedResult: (p0) =>
                              $$RegionsTableReferences(
                                db,
                                table,
                                p0,
                              ).mapChunksRefs,
                          currentColumn: 'id',
                          referencedColumn: 'region_id',
                          typedResults: items,
                        ),
                    ];
                  },
                );
              },
        ),
      );
}

typedef $$RegionsTableProcessedTableManager =
    ProcessedTableManager<
      _$OfflineDatabase,
      $RegionsTable,
      RegionRow,
      $$RegionsTableFilterComposer,
      $$RegionsTableOrderingComposer,
      $$RegionsTableAnnotationComposer,
      $$RegionsTableCreateCompanionBuilder,
      $$RegionsTableUpdateCompanionBuilder,
      (RegionRow, $$RegionsTableReferences),
      RegionRow,
      PrefetchHooks Function({
        bool regionFilesRefs,
        bool regionFeedsRefs,
        bool mapChunksRefs,
      })
    >;
typedef $$FilesTableCreateCompanionBuilder = FilesCompanion Function({
  Value<int> id,
  required ReleaseKind kind,
  required String release,
  required String path,
  required String installPath,
  Value<int?> bytes,
  Value<String?> expectedSha256,
  Value<String?> verifiedSha256,
  required FileState state,
  Value<int> attempts,
  Value<String?> error,
  Value<DateTime?> lastUsed,
});
typedef $$FilesTableUpdateCompanionBuilder = FilesCompanion Function({
  Value<int> id,
  Value<ReleaseKind> kind,
  Value<String> release,
  Value<String> path,
  Value<String> installPath,
  Value<int?> bytes,
  Value<String?> expectedSha256,
  Value<String?> verifiedSha256,
  Value<FileState> state,
  Value<int> attempts,
  Value<String?> error,
  Value<DateTime?> lastUsed,
});

final class $$FilesTableReferences
    extends BaseReferences<_$OfflineDatabase, $FilesTable, FileRow> {
  $$FilesTableReferences(super.$_db, super.$_table, super.$_typedResult);

  static MultiTypedResultKey<$RegionFilesTable, List<RegionFileRow>>
  _regionFilesRefsTable(_$OfflineDatabase db) => MultiTypedResultKey.fromTable(
    db.regionFiles,
    aliasName: 'files__id__region_files__file_id',
  );

  $$RegionFilesTableProcessedTableManager get regionFilesRefs {
    final manager = $$RegionFilesTableTableManager(
      $_db,
      $_db.regionFiles,
    ).filter((f) => f.fileId.id.sqlEquals($_itemColumn<int>('id')!));

    final cache = $_typedResult.readTableOrNull(_regionFilesRefsTable($_db));
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: cache),
    );
  }
}

class $$FilesTableFilterComposer
    extends Composer<_$OfflineDatabase, $FilesTable> {
  $$FilesTableFilterComposer({
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

  ColumnWithTypeConverterFilters<ReleaseKind, ReleaseKind, String> get kind =>
      $composableBuilder(
        column: $table.kind,
        builder: (column) => ColumnWithTypeConverterFilters(column),
      );

  ColumnFilters<String> get release => $composableBuilder(
    column: $table.release,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get path => $composableBuilder(
    column: $table.path,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get installPath => $composableBuilder(
    column: $table.installPath,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get bytes => $composableBuilder(
    column: $table.bytes,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get expectedSha256 => $composableBuilder(
    column: $table.expectedSha256,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get verifiedSha256 => $composableBuilder(
    column: $table.verifiedSha256,
    builder: (column) => ColumnFilters(column),
  );

  ColumnWithTypeConverterFilters<FileState, FileState, String> get state =>
      $composableBuilder(
        column: $table.state,
        builder: (column) => ColumnWithTypeConverterFilters(column),
      );

  ColumnFilters<int> get attempts => $composableBuilder(
    column: $table.attempts,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get error => $composableBuilder(
    column: $table.error,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<DateTime> get lastUsed => $composableBuilder(
    column: $table.lastUsed,
    builder: (column) => ColumnFilters(column),
  );

  Expression<bool> regionFilesRefs(
    Expression<bool> Function($$RegionFilesTableFilterComposer f) f,
  ) {
    final $$RegionFilesTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.regionFiles,
      getReferencedColumn: (t) => t.fileId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionFilesTableFilterComposer(
            $db: $db,
            $table: $db.regionFiles,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }
}

class $$FilesTableOrderingComposer
    extends Composer<_$OfflineDatabase, $FilesTable> {
  $$FilesTableOrderingComposer({
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

  ColumnOrderings<String> get kind => $composableBuilder(
    column: $table.kind,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get release => $composableBuilder(
    column: $table.release,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get path => $composableBuilder(
    column: $table.path,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get installPath => $composableBuilder(
    column: $table.installPath,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get bytes => $composableBuilder(
    column: $table.bytes,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get expectedSha256 => $composableBuilder(
    column: $table.expectedSha256,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get verifiedSha256 => $composableBuilder(
    column: $table.verifiedSha256,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get state => $composableBuilder(
    column: $table.state,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get attempts => $composableBuilder(
    column: $table.attempts,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get error => $composableBuilder(
    column: $table.error,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<DateTime> get lastUsed => $composableBuilder(
    column: $table.lastUsed,
    builder: (column) => ColumnOrderings(column),
  );
}

class $$FilesTableAnnotationComposer
    extends Composer<_$OfflineDatabase, $FilesTable> {
  $$FilesTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<int> get id =>
      $composableBuilder(column: $table.id, builder: (column) => column);

  GeneratedColumnWithTypeConverter<ReleaseKind, String> get kind =>
      $composableBuilder(column: $table.kind, builder: (column) => column);

  GeneratedColumn<String> get release =>
      $composableBuilder(column: $table.release, builder: (column) => column);

  GeneratedColumn<String> get path =>
      $composableBuilder(column: $table.path, builder: (column) => column);

  GeneratedColumn<String> get installPath => $composableBuilder(
    column: $table.installPath,
    builder: (column) => column,
  );

  GeneratedColumn<int> get bytes =>
      $composableBuilder(column: $table.bytes, builder: (column) => column);

  GeneratedColumn<String> get expectedSha256 => $composableBuilder(
    column: $table.expectedSha256,
    builder: (column) => column,
  );

  GeneratedColumn<String> get verifiedSha256 => $composableBuilder(
    column: $table.verifiedSha256,
    builder: (column) => column,
  );

  GeneratedColumnWithTypeConverter<FileState, String> get state =>
      $composableBuilder(column: $table.state, builder: (column) => column);

  GeneratedColumn<int> get attempts =>
      $composableBuilder(column: $table.attempts, builder: (column) => column);

  GeneratedColumn<String> get error =>
      $composableBuilder(column: $table.error, builder: (column) => column);

  GeneratedColumn<DateTime> get lastUsed =>
      $composableBuilder(column: $table.lastUsed, builder: (column) => column);

  Expression<T> regionFilesRefs<T extends Object>(
    Expression<T> Function($$RegionFilesTableAnnotationComposer a) f,
  ) {
    final $$RegionFilesTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.id,
      referencedTable: $db.regionFiles,
      getReferencedColumn: (t) => t.fileId,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionFilesTableAnnotationComposer(
            $db: $db,
            $table: $db.regionFiles,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return f(composer);
  }
}

class $$FilesTableTableManager
    extends
        RootTableManager<
          _$OfflineDatabase,
          $FilesTable,
          FileRow,
          $$FilesTableFilterComposer,
          $$FilesTableOrderingComposer,
          $$FilesTableAnnotationComposer,
          $$FilesTableCreateCompanionBuilder,
          $$FilesTableUpdateCompanionBuilder,
          (FileRow, $$FilesTableReferences),
          FileRow,
          PrefetchHooks Function({bool regionFilesRefs})
        > {
  $$FilesTableTableManager(_$OfflineDatabase db, $FilesTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$FilesTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$FilesTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$FilesTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                Value<ReleaseKind> kind = const Value.absent(),
                Value<String> release = const Value.absent(),
                Value<String> path = const Value.absent(),
                Value<String> installPath = const Value.absent(),
                Value<int?> bytes = const Value.absent(),
                Value<String?> expectedSha256 = const Value.absent(),
                Value<String?> verifiedSha256 = const Value.absent(),
                Value<FileState> state = const Value.absent(),
                Value<int> attempts = const Value.absent(),
                Value<String?> error = const Value.absent(),
                Value<DateTime?> lastUsed = const Value.absent(),
              }) => FilesCompanion(
                id: id,
                kind: kind,
                release: release,
                path: path,
                installPath: installPath,
                bytes: bytes,
                expectedSha256: expectedSha256,
                verifiedSha256: verifiedSha256,
                state: state,
                attempts: attempts,
                error: error,
                lastUsed: lastUsed,
              ),
          createCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                required ReleaseKind kind,
                required String release,
                required String path,
                required String installPath,
                Value<int?> bytes = const Value.absent(),
                Value<String?> expectedSha256 = const Value.absent(),
                Value<String?> verifiedSha256 = const Value.absent(),
                required FileState state,
                Value<int> attempts = const Value.absent(),
                Value<String?> error = const Value.absent(),
                Value<DateTime?> lastUsed = const Value.absent(),
              }) => FilesCompanion.insert(
                id: id,
                kind: kind,
                release: release,
                path: path,
                installPath: installPath,
                bytes: bytes,
                expectedSha256: expectedSha256,
                verifiedSha256: verifiedSha256,
                state: state,
                attempts: attempts,
                error: error,
                lastUsed: lastUsed,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$FilesTable, FileRow>(table),
                  $$FilesTableReferences(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: ({regionFilesRefs = false}) {
            return PrefetchHooks(
              db: db,
              explicitlyWatchedTables: [if (regionFilesRefs) db.regionFiles],
              addJoins: null,
              getPrefetchedDataCallback: (items) async {
                return [
                  if (regionFilesRefs)
                    await $_getPrefetchedDataByColumns<
                      FileRow,
                      $FilesTable,
                      RegionFileRow,
                      $RegionFilesTable
                    >(
                      currentTable: table,
                      referencedTable: $$FilesTableReferences
                          ._regionFilesRefsTable(db),
                      managerFromTypedResult: (p0) =>
                          $$FilesTableReferences(db, table, p0).regionFilesRefs,
                      currentColumn: 'id',
                      referencedColumn: 'file_id',
                      typedResults: items,
                    ),
                ];
              },
            );
          },
        ),
      );
}

typedef $$FilesTableProcessedTableManager =
    ProcessedTableManager<
      _$OfflineDatabase,
      $FilesTable,
      FileRow,
      $$FilesTableFilterComposer,
      $$FilesTableOrderingComposer,
      $$FilesTableAnnotationComposer,
      $$FilesTableCreateCompanionBuilder,
      $$FilesTableUpdateCompanionBuilder,
      (FileRow, $$FilesTableReferences),
      FileRow,
      PrefetchHooks Function({bool regionFilesRefs})
    >;
typedef $$RegionFilesTableCreateCompanionBuilder =
    RegionFilesCompanion Function({
      required int regionId,
      required int fileId,
      Value<int> rowid,
    });
typedef $$RegionFilesTableUpdateCompanionBuilder =
    RegionFilesCompanion Function({
      Value<int> regionId,
      Value<int> fileId,
      Value<int> rowid,
    });

final class $$RegionFilesTableReferences
    extends
        BaseReferences<_$OfflineDatabase, $RegionFilesTable, RegionFileRow> {
  $$RegionFilesTableReferences(super.$_db, super.$_table, super.$_typedResult);

  static $RegionsTable _regionIdTable(_$OfflineDatabase db) =>
      db.regions.createAlias('region_files__region_id__regions__id');

  $$RegionsTableProcessedTableManager get regionId {
    final $_column = $_itemColumn<int>('region_id')!;

    final manager = $$RegionsTableTableManager(
      $_db,
      $_db.regions,
    ).filter((f) => f.id.sqlEquals($_column));
    final item = $_typedResult.readTableOrNull(_regionIdTable($_db));
    if (item == null) return manager;
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: [item]),
    );
  }

  static $FilesTable _fileIdTable(_$OfflineDatabase db) =>
      db.files.createAlias('region_files__file_id__files__id');

  $$FilesTableProcessedTableManager get fileId {
    final $_column = $_itemColumn<int>('file_id')!;

    final manager = $$FilesTableTableManager(
      $_db,
      $_db.files,
    ).filter((f) => f.id.sqlEquals($_column));
    final item = $_typedResult.readTableOrNull(_fileIdTable($_db));
    if (item == null) return manager;
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: [item]),
    );
  }
}

class $$RegionFilesTableFilterComposer
    extends Composer<_$OfflineDatabase, $RegionFilesTable> {
  $$RegionFilesTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  $$RegionsTableFilterComposer get regionId {
    final $$RegionsTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableFilterComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }

  $$FilesTableFilterComposer get fileId {
    final $$FilesTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.fileId,
      referencedTable: $db.files,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$FilesTableFilterComposer(
            $db: $db,
            $table: $db.files,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$RegionFilesTableOrderingComposer
    extends Composer<_$OfflineDatabase, $RegionFilesTable> {
  $$RegionFilesTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  $$RegionsTableOrderingComposer get regionId {
    final $$RegionsTableOrderingComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableOrderingComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }

  $$FilesTableOrderingComposer get fileId {
    final $$FilesTableOrderingComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.fileId,
      referencedTable: $db.files,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$FilesTableOrderingComposer(
            $db: $db,
            $table: $db.files,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$RegionFilesTableAnnotationComposer
    extends Composer<_$OfflineDatabase, $RegionFilesTable> {
  $$RegionFilesTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  $$RegionsTableAnnotationComposer get regionId {
    final $$RegionsTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableAnnotationComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }

  $$FilesTableAnnotationComposer get fileId {
    final $$FilesTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.fileId,
      referencedTable: $db.files,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$FilesTableAnnotationComposer(
            $db: $db,
            $table: $db.files,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$RegionFilesTableTableManager
    extends
        RootTableManager<
          _$OfflineDatabase,
          $RegionFilesTable,
          RegionFileRow,
          $$RegionFilesTableFilterComposer,
          $$RegionFilesTableOrderingComposer,
          $$RegionFilesTableAnnotationComposer,
          $$RegionFilesTableCreateCompanionBuilder,
          $$RegionFilesTableUpdateCompanionBuilder,
          (RegionFileRow, $$RegionFilesTableReferences),
          RegionFileRow,
          PrefetchHooks Function({bool regionId, bool fileId})
        > {
  $$RegionFilesTableTableManager(_$OfflineDatabase db, $RegionFilesTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$RegionFilesTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$RegionFilesTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$RegionFilesTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<int> regionId = const Value.absent(),
                Value<int> fileId = const Value.absent(),
                Value<int> rowid = const Value.absent(),
              }) => RegionFilesCompanion(
                regionId: regionId,
                fileId: fileId,
                rowid: rowid,
              ),
          createCompanionCallback:
              ({
                required int regionId,
                required int fileId,
                Value<int> rowid = const Value.absent(),
              }) => RegionFilesCompanion.insert(
                regionId: regionId,
                fileId: fileId,
                rowid: rowid,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$RegionFilesTable, RegionFileRow>(table),
                  $$RegionFilesTableReferences(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: ({regionId = false, fileId = false}) {
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
                    if (regionId) {
                      state = state.withJoin(
                        currentTable: table,
                        currentColumn: table.regionId,
                        referencedTable: $$RegionFilesTableReferences
                            ._regionIdTable(db),
                        referencedColumn: $$RegionFilesTableReferences
                            ._regionIdTable(db)
                            .id,
                      ) as T;
                    }
                    if (fileId) {
                      state = state.withJoin(
                        currentTable: table,
                        currentColumn: table.fileId,
                        referencedTable: $$RegionFilesTableReferences
                            ._fileIdTable(db),
                        referencedColumn: $$RegionFilesTableReferences
                            ._fileIdTable(db)
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

typedef $$RegionFilesTableProcessedTableManager =
    ProcessedTableManager<
      _$OfflineDatabase,
      $RegionFilesTable,
      RegionFileRow,
      $$RegionFilesTableFilterComposer,
      $$RegionFilesTableOrderingComposer,
      $$RegionFilesTableAnnotationComposer,
      $$RegionFilesTableCreateCompanionBuilder,
      $$RegionFilesTableUpdateCompanionBuilder,
      (RegionFileRow, $$RegionFilesTableReferences),
      RegionFileRow,
      PrefetchHooks Function({bool regionId, bool fileId})
    >;
typedef $$RegionFeedsTableCreateCompanionBuilder =
    RegionFeedsCompanion Function({
      required int regionId,
      required String feedId,
      required int feedIndex,
      Value<int> rowid,
    });
typedef $$RegionFeedsTableUpdateCompanionBuilder =
    RegionFeedsCompanion Function({
      Value<int> regionId,
      Value<String> feedId,
      Value<int> feedIndex,
      Value<int> rowid,
    });

final class $$RegionFeedsTableReferences
    extends
        BaseReferences<_$OfflineDatabase, $RegionFeedsTable, RegionFeedRow> {
  $$RegionFeedsTableReferences(super.$_db, super.$_table, super.$_typedResult);

  static $RegionsTable _regionIdTable(_$OfflineDatabase db) =>
      db.regions.createAlias('region_feeds__region_id__regions__id');

  $$RegionsTableProcessedTableManager get regionId {
    final $_column = $_itemColumn<int>('region_id')!;

    final manager = $$RegionsTableTableManager(
      $_db,
      $_db.regions,
    ).filter((f) => f.id.sqlEquals($_column));
    final item = $_typedResult.readTableOrNull(_regionIdTable($_db));
    if (item == null) return manager;
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: [item]),
    );
  }
}

class $$RegionFeedsTableFilterComposer
    extends Composer<_$OfflineDatabase, $RegionFeedsTable> {
  $$RegionFeedsTableFilterComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnFilters<String> get feedId => $composableBuilder(
    column: $table.feedId,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get feedIndex => $composableBuilder(
    column: $table.feedIndex,
    builder: (column) => ColumnFilters(column),
  );

  $$RegionsTableFilterComposer get regionId {
    final $$RegionsTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableFilterComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$RegionFeedsTableOrderingComposer
    extends Composer<_$OfflineDatabase, $RegionFeedsTable> {
  $$RegionFeedsTableOrderingComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  ColumnOrderings<String> get feedId => $composableBuilder(
    column: $table.feedId,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get feedIndex => $composableBuilder(
    column: $table.feedIndex,
    builder: (column) => ColumnOrderings(column),
  );

  $$RegionsTableOrderingComposer get regionId {
    final $$RegionsTableOrderingComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableOrderingComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$RegionFeedsTableAnnotationComposer
    extends Composer<_$OfflineDatabase, $RegionFeedsTable> {
  $$RegionFeedsTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<String> get feedId =>
      $composableBuilder(column: $table.feedId, builder: (column) => column);

  GeneratedColumn<int> get feedIndex =>
      $composableBuilder(column: $table.feedIndex, builder: (column) => column);

  $$RegionsTableAnnotationComposer get regionId {
    final $$RegionsTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableAnnotationComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$RegionFeedsTableTableManager
    extends
        RootTableManager<
          _$OfflineDatabase,
          $RegionFeedsTable,
          RegionFeedRow,
          $$RegionFeedsTableFilterComposer,
          $$RegionFeedsTableOrderingComposer,
          $$RegionFeedsTableAnnotationComposer,
          $$RegionFeedsTableCreateCompanionBuilder,
          $$RegionFeedsTableUpdateCompanionBuilder,
          (RegionFeedRow, $$RegionFeedsTableReferences),
          RegionFeedRow,
          PrefetchHooks Function({bool regionId})
        > {
  $$RegionFeedsTableTableManager(_$OfflineDatabase db, $RegionFeedsTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$RegionFeedsTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$RegionFeedsTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$RegionFeedsTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<int> regionId = const Value.absent(),
                Value<String> feedId = const Value.absent(),
                Value<int> feedIndex = const Value.absent(),
                Value<int> rowid = const Value.absent(),
              }) => RegionFeedsCompanion(
                regionId: regionId,
                feedId: feedId,
                feedIndex: feedIndex,
                rowid: rowid,
              ),
          createCompanionCallback:
              ({
                required int regionId,
                required String feedId,
                required int feedIndex,
                Value<int> rowid = const Value.absent(),
              }) => RegionFeedsCompanion.insert(
                regionId: regionId,
                feedId: feedId,
                feedIndex: feedIndex,
                rowid: rowid,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$RegionFeedsTable, RegionFeedRow>(table),
                  $$RegionFeedsTableReferences(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: ({regionId = false}) {
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
                    if (regionId) {
                      state = state.withJoin(
                        currentTable: table,
                        currentColumn: table.regionId,
                        referencedTable: $$RegionFeedsTableReferences
                            ._regionIdTable(db),
                        referencedColumn: $$RegionFeedsTableReferences
                            ._regionIdTable(db)
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

typedef $$RegionFeedsTableProcessedTableManager =
    ProcessedTableManager<
      _$OfflineDatabase,
      $RegionFeedsTable,
      RegionFeedRow,
      $$RegionFeedsTableFilterComposer,
      $$RegionFeedsTableOrderingComposer,
      $$RegionFeedsTableAnnotationComposer,
      $$RegionFeedsTableCreateCompanionBuilder,
      $$RegionFeedsTableUpdateCompanionBuilder,
      (RegionFeedRow, $$RegionFeedsTableReferences),
      RegionFeedRow,
      PrefetchHooks Function({bool regionId})
    >;
typedef $$MapChunksTableCreateCompanionBuilder = MapChunksCompanion Function({
  Value<int> id,
  required int regionId,
  required int start,
  required int length,
  required int tileCount,
  required Uint8List tiles,
  required ChunkState state,
  Value<int> attempts,
  Value<String?> error,
});
typedef $$MapChunksTableUpdateCompanionBuilder = MapChunksCompanion Function({
  Value<int> id,
  Value<int> regionId,
  Value<int> start,
  Value<int> length,
  Value<int> tileCount,
  Value<Uint8List> tiles,
  Value<ChunkState> state,
  Value<int> attempts,
  Value<String?> error,
});

final class $$MapChunksTableReferences
    extends BaseReferences<_$OfflineDatabase, $MapChunksTable, MapChunkRow> {
  $$MapChunksTableReferences(super.$_db, super.$_table, super.$_typedResult);

  static $RegionsTable _regionIdTable(_$OfflineDatabase db) =>
      db.regions.createAlias('map_chunks__region_id__regions__id');

  $$RegionsTableProcessedTableManager get regionId {
    final $_column = $_itemColumn<int>('region_id')!;

    final manager = $$RegionsTableTableManager(
      $_db,
      $_db.regions,
    ).filter((f) => f.id.sqlEquals($_column));
    final item = $_typedResult.readTableOrNull(_regionIdTable($_db));
    if (item == null) return manager;
    return ProcessedTableManager(
      manager.$state.copyWith(prefetchedData: [item]),
    );
  }
}

class $$MapChunksTableFilterComposer
    extends Composer<_$OfflineDatabase, $MapChunksTable> {
  $$MapChunksTableFilterComposer({
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

  ColumnFilters<int> get start => $composableBuilder(
    column: $table.start,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get length => $composableBuilder(
    column: $table.length,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<int> get tileCount => $composableBuilder(
    column: $table.tileCount,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<Uint8List> get tiles => $composableBuilder(
    column: $table.tiles,
    builder: (column) => ColumnFilters(column),
  );

  ColumnWithTypeConverterFilters<ChunkState, ChunkState, String> get state =>
      $composableBuilder(
        column: $table.state,
        builder: (column) => ColumnWithTypeConverterFilters(column),
      );

  ColumnFilters<int> get attempts => $composableBuilder(
    column: $table.attempts,
    builder: (column) => ColumnFilters(column),
  );

  ColumnFilters<String> get error => $composableBuilder(
    column: $table.error,
    builder: (column) => ColumnFilters(column),
  );

  $$RegionsTableFilterComposer get regionId {
    final $$RegionsTableFilterComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableFilterComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$MapChunksTableOrderingComposer
    extends Composer<_$OfflineDatabase, $MapChunksTable> {
  $$MapChunksTableOrderingComposer({
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

  ColumnOrderings<int> get start => $composableBuilder(
    column: $table.start,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get length => $composableBuilder(
    column: $table.length,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get tileCount => $composableBuilder(
    column: $table.tileCount,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<Uint8List> get tiles => $composableBuilder(
    column: $table.tiles,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get state => $composableBuilder(
    column: $table.state,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<int> get attempts => $composableBuilder(
    column: $table.attempts,
    builder: (column) => ColumnOrderings(column),
  );

  ColumnOrderings<String> get error => $composableBuilder(
    column: $table.error,
    builder: (column) => ColumnOrderings(column),
  );

  $$RegionsTableOrderingComposer get regionId {
    final $$RegionsTableOrderingComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableOrderingComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$MapChunksTableAnnotationComposer
    extends Composer<_$OfflineDatabase, $MapChunksTable> {
  $$MapChunksTableAnnotationComposer({
    required super.$db,
    required super.$table,
    super.joinBuilder,
    super.$addJoinBuilderToRootComposer,
    super.$removeJoinBuilderFromRootComposer,
  });
  GeneratedColumn<int> get id =>
      $composableBuilder(column: $table.id, builder: (column) => column);

  GeneratedColumn<int> get start =>
      $composableBuilder(column: $table.start, builder: (column) => column);

  GeneratedColumn<int> get length =>
      $composableBuilder(column: $table.length, builder: (column) => column);

  GeneratedColumn<int> get tileCount =>
      $composableBuilder(column: $table.tileCount, builder: (column) => column);

  GeneratedColumn<Uint8List> get tiles =>
      $composableBuilder(column: $table.tiles, builder: (column) => column);

  GeneratedColumnWithTypeConverter<ChunkState, String> get state =>
      $composableBuilder(column: $table.state, builder: (column) => column);

  GeneratedColumn<int> get attempts =>
      $composableBuilder(column: $table.attempts, builder: (column) => column);

  GeneratedColumn<String> get error =>
      $composableBuilder(column: $table.error, builder: (column) => column);

  $$RegionsTableAnnotationComposer get regionId {
    final $$RegionsTableAnnotationComposer composer = $composerBuilder(
      composer: this,
      getCurrentColumn: (t) => t.regionId,
      referencedTable: $db.regions,
      getReferencedColumn: (t) => t.id,
      builder:
          (
            joinBuilder, {
            $addJoinBuilderToRootComposer,
            $removeJoinBuilderFromRootComposer,
          }) => $$RegionsTableAnnotationComposer(
            $db: $db,
            $table: $db.regions,
            $addJoinBuilderToRootComposer: $addJoinBuilderToRootComposer,
            joinBuilder: joinBuilder,
            $removeJoinBuilderFromRootComposer:
                $removeJoinBuilderFromRootComposer,
          ),
    );
    return composer;
  }
}

class $$MapChunksTableTableManager
    extends
        RootTableManager<
          _$OfflineDatabase,
          $MapChunksTable,
          MapChunkRow,
          $$MapChunksTableFilterComposer,
          $$MapChunksTableOrderingComposer,
          $$MapChunksTableAnnotationComposer,
          $$MapChunksTableCreateCompanionBuilder,
          $$MapChunksTableUpdateCompanionBuilder,
          (MapChunkRow, $$MapChunksTableReferences),
          MapChunkRow,
          PrefetchHooks Function({bool regionId})
        > {
  $$MapChunksTableTableManager(_$OfflineDatabase db, $MapChunksTable table)
    : super(
        TableManagerState(
          db: db,
          table: table,
          createFilteringComposer: () =>
              $$MapChunksTableFilterComposer($db: db, $table: table),
          createOrderingComposer: () =>
              $$MapChunksTableOrderingComposer($db: db, $table: table),
          createComputedFieldComposer: () =>
              $$MapChunksTableAnnotationComposer($db: db, $table: table),
          updateCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                Value<int> regionId = const Value.absent(),
                Value<int> start = const Value.absent(),
                Value<int> length = const Value.absent(),
                Value<int> tileCount = const Value.absent(),
                Value<Uint8List> tiles = const Value.absent(),
                Value<ChunkState> state = const Value.absent(),
                Value<int> attempts = const Value.absent(),
                Value<String?> error = const Value.absent(),
              }) => MapChunksCompanion(
                id: id,
                regionId: regionId,
                start: start,
                length: length,
                tileCount: tileCount,
                tiles: tiles,
                state: state,
                attempts: attempts,
                error: error,
              ),
          createCompanionCallback:
              ({
                Value<int> id = const Value.absent(),
                required int regionId,
                required int start,
                required int length,
                required int tileCount,
                required Uint8List tiles,
                required ChunkState state,
                Value<int> attempts = const Value.absent(),
                Value<String?> error = const Value.absent(),
              }) => MapChunksCompanion.insert(
                id: id,
                regionId: regionId,
                start: start,
                length: length,
                tileCount: tileCount,
                tiles: tiles,
                state: state,
                attempts: attempts,
                error: error,
              ),
          withReferenceMapper: (p0) => p0
              .map(
                (e) => (
                  e.readTable<$MapChunksTable, MapChunkRow>(table),
                  $$MapChunksTableReferences(db, table, e),
                ),
              )
              .toList(),
          prefetchHooksCallback: ({regionId = false}) {
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
                    if (regionId) {
                      state = state.withJoin(
                        currentTable: table,
                        currentColumn: table.regionId,
                        referencedTable: $$MapChunksTableReferences
                            ._regionIdTable(db),
                        referencedColumn: $$MapChunksTableReferences
                            ._regionIdTable(db)
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

typedef $$MapChunksTableProcessedTableManager =
    ProcessedTableManager<
      _$OfflineDatabase,
      $MapChunksTable,
      MapChunkRow,
      $$MapChunksTableFilterComposer,
      $$MapChunksTableOrderingComposer,
      $$MapChunksTableAnnotationComposer,
      $$MapChunksTableCreateCompanionBuilder,
      $$MapChunksTableUpdateCompanionBuilder,
      (MapChunkRow, $$MapChunksTableReferences),
      MapChunkRow,
      PrefetchHooks Function({bool regionId})
    >;

class $OfflineDatabaseManager {
  final _$OfflineDatabase _db;
  $OfflineDatabaseManager(this._db);
  $$ReleasesTableTableManager get releases =>
      $$ReleasesTableTableManager(_db, _db.releases);
  $$RegionsTableTableManager get regions =>
      $$RegionsTableTableManager(_db, _db.regions);
  $$FilesTableTableManager get files =>
      $$FilesTableTableManager(_db, _db.files);
  $$RegionFilesTableTableManager get regionFiles =>
      $$RegionFilesTableTableManager(_db, _db.regionFiles);
  $$RegionFeedsTableTableManager get regionFeeds =>
      $$RegionFeedsTableTableManager(_db, _db.regionFeeds);
  $$MapChunksTableTableManager get mapChunks =>
      $$MapChunksTableTableManager(_db, _db.mapChunks);
}
