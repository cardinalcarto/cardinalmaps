import 'dart:io';

import 'package:cardinal/offline/offline_area.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    "an area covers the quarter degrees whose level 2 tiles it has",
    () async {
      final root = await Directory.systemTemp.createTemp('offline_area_test');
      addTearDown(() => root.delete(recursive: true));
      // Seattle's square, 47.6, -122.33: row 550, column 230.
      await File('${root.path}/routing/r/tiles/2/000/792/230.gph')
          .create(recursive: true);
      final area = OfflineArea(
        routing: '${root.path}/routing/r',
        transitBuild: '${root.path}/transit/t',
      );
      expect(await area.covers(47.6062, -122.3321), isTrue);
      expect(await area.covers(47.6062, -122.0), isFalse);
      expect(await area.covers(45.5, -122.6), isFalse);
    },
  );
}
