import 'package:cardinal/map/overlays.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late MapOverlays overlays;
  var changes = 0;

  setUp(() {
    overlays = MapOverlays();
    changes = 0;
    overlays.addListener(() => changes++);
  });

  test('overlays show until something hides them', () {
    expect(overlays.shown, isTrue);
    final release = overlays.hide();
    expect(overlays.shown, isFalse);
    expect(changes, 1);
    release();
    expect(overlays.shown, isTrue);
    expect(changes, 2);
  });

  test('they come back only when the last screen lets go', () {
    final first = overlays.hide();
    final second = overlays.hide();
    expect(changes, 1);
    first();
    expect(overlays.shown, isFalse);
    expect(changes, 1);
    second();
    expect(overlays.shown, isTrue);
    expect(changes, 2);
  });

  test('letting go twice counts once', () {
    final first = overlays.hide();
    final second = overlays.hide();
    first();
    first();
    expect(overlays.shown, isFalse);
    second();
    expect(overlays.shown, isTrue);
  });
}
