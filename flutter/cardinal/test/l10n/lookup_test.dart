import 'package:cardinal/l10n/lookup.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('falls back from a regional tag to its language', () {
    for (final tag in ['en', 'en-US', 'en-GB', 'en_AU', 'EN-gb']) {
      expect(localizationsFor(tag)?.rerouting, 'Rerouting', reason: tag);
    }
  });

  test("is null for a language the app isn't translated into", () {
    for (final tag in ['de-DE', 'pt-BR', 'zh-Hans-CN', '']) {
      expect(localizationsFor(tag), isNull, reason: tag);
    }
  });
}
