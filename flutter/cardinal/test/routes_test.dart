import 'package:cardinal/routes.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:go_router/go_router.dart';
import 'package:material_ui/material_ui.dart';

void main() {
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
