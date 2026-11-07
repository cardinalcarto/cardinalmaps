import 'package:cardinal/routes.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

void main() {
  testWidgets('tapping places on the map keeps one card, and back goes home', (tester) async {
    final shown = <String>[];
    final router = GoRouter(
      routes: [
        GoRoute(
          path: '/',
          builder: (context, state) => const Text('home'),
          routes: [
            GoRoute(
              path: 'place/:id',
              builder: (context, state) {
                shown.add(state.pathParameters['id']!);
                return Text(state.pathParameters['id']!);
              },
            ),
          ],
        ),
      ],
    );
    addTearDown(router.dispose);
    await tester.pumpWidget(MaterialApp.router(routerConfig: router));

    openPlaceFromMap(router, 'poi:1');
    await tester.pumpAndSettle();
    expect(find.text('poi:1'), findsOneWidget);

    openPlaceFromMap(router, 'poi:2');
    await tester.pumpAndSettle();
    expect(find.text('poi:2'), findsOneWidget);
    expect(find.text('poi:1'), findsNothing);

    // The one showing, again: nothing changes.
    openPlaceFromMap(router, 'poi:2');
    await tester.pumpAndSettle();
    expect(find.text('poi:2'), findsOneWidget);

    router.pop();
    await tester.pumpAndSettle();
    expect(find.text('home'), findsOneWidget);
    expect(router.canPop(), isFalse);
  });

  for (final id in ['poi:97175', 'wof:101730401', 'osm:way/12903132']) {
    testWidgets('the place route gets back $id', (tester) async {
      String? routed;
      final router = GoRouter(
        routes: [
          GoRoute(path: '/', builder: (context, state) => const SizedBox()),
          GoRoute(
            path: '/place/:id',
            builder: (context, state) {
              routed = state.pathParameters['id'];
              return const SizedBox();
            },
          ),
        ],
      );
      addTearDown(router.dispose);
      await tester.pumpWidget(MaterialApp.router(routerConfig: router));
      router.push(placeLocation(id));
      await tester.pumpAndSettle();
      expect(routed, id);
    });
  }
}
