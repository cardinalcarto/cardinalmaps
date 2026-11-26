import 'dart:async';
import 'dart:convert';

import 'package:drift/drift.dart';

import 'database.dart';

/// A setting: its key in the store, its value until it's set, and how the value is
/// written as JSON.
///
/// Settings are declared once, as constants beside what they configure. A stored value
/// that no longer reads as the setting's type, say a choice that's since been removed,
/// reads as the default.
class Setting<T> {
  const Setting(
    this.key,
    this.defaultValue, {
    required this.encode,
    required this.decode,
  });

  /// One of [values], stored by name.
  static Setting<E> choice<E extends Enum>(
    String key,
    List<E> values,
    E defaultValue,
  ) => Setting(
    key,
    defaultValue,
    encode: (value) => value.name,
    decode: (json) =>
        values.firstWhere((v) => v.name == json, orElse: () => defaultValue),
  );

  /// One of [values], stored by name, or null until one is set: for when not having
  /// chosen yet means something a default value can't say.
  static Setting<E?> optionalChoice<E extends Enum>(
    String key,
    List<E> values,
  ) => Setting(
    key,
    null,
    encode: (value) => value?.name,
    decode: (json) => values.where((v) => v.name == json).firstOrNull,
  );

  /// On or off.
  static Setting<bool> flag(String key, bool defaultValue) => Setting(
    key,
    defaultValue,
    encode: (value) => value,
    decode: (json) => json is bool ? json : defaultValue,
  );

  final String key;
  final T defaultValue;

  /// The value as a JSON value: a string, number, bool, list or map.
  final Object? Function(T value) encode;
  final T Function(Object? json) decode;

  T _read(String? text) {
    if (text == null) return defaultValue;
    try {
      return decode(jsonDecode(text));
    } on Object {
      return defaultValue;
    }
  }
}

/// Whether to keep local search history. Turning this off clears it.
final saveSearchHistory = Setting.flag('saveSearchHistory', true);

/// The app's settings, kept in the database (or in memory, for tests).
abstract class SettingsStore {
  factory SettingsStore(CardinalDatabase db) = _DatabaseSettings;

  /// Settings that last only as long as the store, for tests.
  factory SettingsStore.inMemory() = _MemorySettings;

  /// The setting's value, or its default if it isn't set.
  Future<T> get<T>(Setting<T> setting);

  /// The setting's value, then each value it's set to.
  Stream<T> watch<T>(Setting<T> setting);

  Future<void> set<T>(Setting<T> setting, T value);
}

class _DatabaseSettings implements SettingsStore {
  _DatabaseSettings(this._db);

  final CardinalDatabase _db;

  Selectable<SettingRow> _select(String key) =>
      _db.select(_db.settings)..where((s) => s.key.equals(key));

  @override
  Future<T> get<T>(Setting<T> setting) async =>
      setting._read((await _select(setting.key).getSingleOrNull())?.value);

  @override
  Stream<T> watch<T>(Setting<T> setting) =>
      _select(setting.key)
          .watchSingleOrNull()
          .map((row) => setting._read(row?.value))
          .distinct();

  @override
  Future<void> set<T>(Setting<T> setting, T value) => _db.transaction(() async {
    await _db
        .into(_db.settings)
        .insertOnConflictUpdate(
          SettingRow(
            key: setting.key,
            value: jsonEncode(setting.encode(value)),
          ),
        );
    if (setting.key == saveSearchHistory.key && value == false) {
      await _db.delete(_db.recentSearches).go();
    }
  });
}

class _MemorySettings implements SettingsStore {
  final _values = <String, String>{};
  final _changes = StreamController<String>.broadcast(sync: true);

  @override
  Future<T> get<T>(Setting<T> setting) async =>
      setting._read(_values[setting.key]);

  @override
  Stream<T> watch<T>(Setting<T> setting) => Stream<T>.multi((controller) {
    final subscription = _changes.stream
        .where((key) => key == setting.key)
        .listen((_) => controller.add(setting._read(_values[setting.key])));
    controller.add(setting._read(_values[setting.key]));
    controller.onCancel = subscription.cancel;
  }).distinct();

  @override
  Future<void> set<T>(Setting<T> setting, T value) async {
    _values[setting.key] = jsonEncode(setting.encode(value));
    _changes.add(setting.key);
  }
}
