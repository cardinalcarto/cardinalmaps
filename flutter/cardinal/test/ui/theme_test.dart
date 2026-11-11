import 'package:cardinal/ui/theme.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart';

void main() {
  final seeded = ColorScheme.fromSeed(seedColor: Colors.blueGrey);

  group('on Android', () {
    final theme = appTheme(Brightness.light, platform: TargetPlatform.android);

    test('is Material 3 from the seed, as it was', () {
      expect(theme.colorScheme, seeded);
      expect(
        theme.textTheme,
        ThemeData(
          colorScheme: seeded,
          platform: TargetPlatform.android,
        ).textTheme,
      );
      expect(theme.splashFactory, isNot(NoSplash.splashFactory));
    });
  });

  group('on iOS', () {
    final light = appTheme(Brightness.light, platform: TargetPlatform.iOS);
    final dark = appTheme(Brightness.dark, platform: TargetPlatform.iOS);

    test('keeps the accent, over the system colors', () {
      expect(light.colorScheme.primary, seeded.primary);
      expect(light.colorScheme.surface, const Color(0xFFFFFFFF));
      expect(light.colorScheme.surfaceContainerLow, const Color(0xFFF2F2F7));
      expect(dark.colorScheme.surface, const Color(0xFF000000));
      expect(dark.colorScheme.surfaceContainerLow, const Color(0xFF1C1C1E));
    });

    test('uses the iOS text styles, with their tracking', () {
      final body = light.textTheme.bodyLarge!;
      expect(body.fontSize, 17);
      expect(body.letterSpacing, -0.43);
      expect(body.height, closeTo(22 / 17, 1e-9));
      expect(light.textTheme.headlineSmall?.fontSize, 28);
      expect(light.textTheme.headlineSmall?.fontWeight, FontWeight.w700);
      expect(light.textTheme.bodySmall?.fontSize, 13);
    });

    test('highlights rather than ripples', () {
      expect(light.splashFactory, NoSplash.splashFactory);
    });

    test('gives Cupertino widgets the accent and brightness', () {
      final cupertino = dark.cupertinoOverrideTheme;
      expect(cupertino?.primaryColor, dark.colorScheme.primary);
      expect(cupertino?.brightness, Brightness.dark);
    });
  });
}
