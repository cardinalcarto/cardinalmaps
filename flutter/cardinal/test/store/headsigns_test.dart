import 'package:cardinal/store/store.dart';
import 'package:drift/drift.dart' show driftRuntimeOptions;
import 'package:drift/native.dart';
import 'package:flutter_test/flutter_test.dart';

HeadsignStop stop(String route, String stop, {String feed = 'f'}) =>
    (feed: feed, routeId: route, stopId: stop);

void main() {
  // Each test opens its own database.
  driftRuntimeOptions.dontWarnAboutMultipleDatabases = true;
  final at = DateTime(2026, 11, 13, 9);

  // The same behavior from both kinds of store.
  for (final (name, open) in [
    (
      'database',
      () => HeadsignStore(CardinalDatabase(NativeDatabase.memory())),
    ),
    ('memory', HeadsignStore.inMemory),
  ]) {
    group('$name headsigns', () {
      test('remember nothing at first', () async {
        expect(await open().recall([stop('44', 'a')]), isEmpty);
        expect(await open().recall([]), isEmpty);
      });

      test('recall what was remembered at the stops asked for', () async {
        final headsigns = open();
        await headsigns.remember(stop('44', 'a'), 'Ballard', chosenAt: at);
        await headsigns.remember(stop('44', 'b'), 'UW', chosenAt: at);
        await headsigns.remember(stop('44', 'a', feed: 'g'), 'X', chosenAt: at);
        expect(await headsigns.recall([stop('44', 'a'), stop('43', 'a')]), {
          stop('44', 'a'): RememberedHeadsign('Ballard', at),
        });
      });

      test('keep the latest choice at a stop', () async {
        final headsigns = open();
        final later = at.add(const Duration(minutes: 1));
        await headsigns.remember(stop('44', 'a'), 'Ballard', chosenAt: at);
        await headsigns.remember(stop('44', 'a'), '', chosenAt: later);
        expect(await headsigns.recall([stop('44', 'a')]), {
          stop('44', 'a'): RememberedHeadsign('', later),
        });
      });
    });
  }
}
