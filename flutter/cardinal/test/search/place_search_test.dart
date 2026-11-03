import 'dart:async';
import 'dart:convert';

import 'package:cardinal/geocoder/geocoder.dart';
import 'package:cardinal/search/place_search.dart';
import 'package:fake_async/fake_async.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import '../geocoder/geocoder_test.dart' show spaceNeedle;

/// A geocoder whose responses arrive when the test completes them, by query.
class _Server {
  final asked = <String>[];
  final _pending = <String, Completer<http.Response>>{};

  late final geocoder = Geocoder(
    Uri.parse('http://localhost:3000'),
    client: MockClient((request) {
      final q = request.url.queryParameters['q']!;
      asked.add(q);
      return (_pending[q] = Completer()).future;
    }),
  );

  void respond(String q, String name) {
    final result = spaceNeedle()..['name'] = name;
    _pending[q]!.complete(
      http.Response(
        jsonEncode({
          'query': {},
          'results': [result],
        }),
        200,
      ),
    );
  }
}

void main() {
  test('waits for typing to pause', () {
    fakeAsync((async) {
      final server = _Server();
      final search = PlaceSearch(server.geocoder);
      for (final text in ['s', 'sp', 'spa', 'spac']) {
        search.update(text);
        async.elapse(const Duration(milliseconds: 100));
      }
      expect(server.asked, isEmpty);
      expect(search.loading, isTrue);
      async.elapse(const Duration(milliseconds: 300));
      expect(server.asked, ['spac']);
      server.respond('spac', 'Space Needle');
      async.flushMicrotasks();
      expect(search.loading, isFalse);
      expect(search.results.single.name, 'Space Needle');
    });
  });

  test('keeps earlier requests going, showing their answers until newer ones come', () {
    fakeAsync((async) {
      final server = _Server();
      final search = PlaceSearch(server.geocoder);
      search.update('star');
      async.elapse(const Duration(milliseconds: 300));
      search.update('starbucks');
      async.elapse(const Duration(milliseconds: 300));
      expect(server.asked, ['star', 'starbucks']);
      // The first answer arrives while the second is on its way: a little behind, but
      // better than nothing.
      server.respond('star', 'Starbuck');
      async.flushMicrotasks();
      expect(search.results.single.name, 'Starbuck');
      expect(search.loading, isTrue);
      server.respond('starbucks', 'Starbucks');
      async.flushMicrotasks();
      expect(search.results.single.name, 'Starbucks');
      expect(search.loading, isFalse);
    });
  });

  test('an older answer never replaces a newer one', () {
    fakeAsync((async) {
      final server = _Server();
      final search = PlaceSearch(server.geocoder);
      search.update('space');
      async.elapse(const Duration(milliseconds: 300));
      search.update('space needle');
      async.elapse(const Duration(milliseconds: 300));
      server.respond('space needle', 'Space Needle');
      async.flushMicrotasks();
      server.respond('space', 'Spaceport');
      async.flushMicrotasks();
      expect(search.results.single.name, 'Space Needle');
      expect(search.loading, isFalse);
    });
  });

  test('answers for cleared text are dropped', () {
    fakeAsync((async) {
      final server = _Server();
      final search = PlaceSearch(server.geocoder);
      search.update('space');
      async.elapse(const Duration(milliseconds: 300));
      search.update('');
      server.respond('space', 'Space Needle');
      async.flushMicrotasks();
      expect(search.results, isEmpty);
    });
  });

  test('clearing the text clears the results', () {
    fakeAsync((async) {
      final server = _Server();
      final search = PlaceSearch(server.geocoder);
      search.update('space');
      async.elapse(const Duration(milliseconds: 300));
      server.respond('space', 'Space Needle');
      async.flushMicrotasks();
      search.update('');
      expect(search.results, isEmpty);
      expect(search.loading, isFalse);
    });
  });
}
