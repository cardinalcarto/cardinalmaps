import 'dart:io';

import 'package:cardinal/store/store.dart';
import 'package:drift/drift.dart' show driftRuntimeOptions;
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';

enum Shade { light, dark }

final shade = Setting.choice('test.shade', Shade.values, Shade.light);
final enabled = Setting.flag('test.enabled', false);

void main() {
  // Each test opens its own database.
  driftRuntimeOptions.dontWarnAboutMultipleDatabases = true;

  // The same behavior from both kinds of store.
  for (final (name, open) in [
    (
      'database',
      () => SettingsStore(CardinalDatabase(NativeDatabase.memory())),
    ),
    ('memory', SettingsStore.inMemory),
  ]) {
    group('$name settings', () {
      test('read as their default until set', () async {
        final settings = open();
        expect(await settings.get(shade), Shade.light);
        expect(await settings.get(enabled), false);
      });

      test('read back what was set', () async {
        final settings = open();
        await settings.set(shade, Shade.dark);
        await settings.set(enabled, true);
        expect(await settings.get(shade), Shade.dark);
        expect(await settings.get(enabled), true);
      });

      test('are watched from their current value', () async {
        final settings = open();
        final seen = <Shade>[];
        final watching = settings.watch(shade).listen(seen.add);
        addTearDown(watching.cancel);
        await pumpEventQueue();
        await settings.set(shade, Shade.dark);
        await pumpEventQueue();
        // Setting the same value again isn't a change.
        await settings.set(shade, Shade.dark);
        await pumpEventQueue();
        expect(seen, [Shade.light, Shade.dark]);
      });

      test('read a value of the wrong kind as the default', () async {
        final settings = open();
        // As if a choice had been removed since it was stored.
        await settings.set(
          Setting.choice('test.shade', [Shade.dark], Shade.dark),
          Shade.dark,
        );
        expect(
          await settings.get(
            Setting.choice('test.shade', [Shade.light], Shade.light),
          ),
          Shade.light,
        );
        await settings.set(Setting.flag('test.shade', true), true);
        expect(await settings.get(shade), Shade.light);
      });
    });
  }

  test('settings persist across opening the database', () async {
    final dir = await Directory.systemTemp.createTemp('cardinal_settings');
    addTearDown(() => dir.delete(recursive: true));
    final file = File('${dir.path}/cardinal.sqlite');

    final first = Store(CardinalDatabase(NativeDatabase(file)));
    await first.settings.set(shade, Shade.dark);
    await first.close();

    final second = Store(CardinalDatabase(NativeDatabase(file)));
    addTearDown(second.close);
    expect(await second.settings.get(shade), Shade.dark);
  });
}
