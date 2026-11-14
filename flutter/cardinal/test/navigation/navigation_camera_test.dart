import 'package:cardinal/navigation/navigation_camera.dart';
import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

void main() {
  group('the zoom from speed', () {
    for (final mode in NavigationMode.values) {
      final camera = followCamera(mode);
      final v = camera.referenceSpeed;

      group(mode.name, () {
        test('is the base zoom without a speed, and up to the reference', () {
          expect(camera.zoomAt(null), camera.zoom);
          expect(camera.zoomAt(0), camera.zoom);
          expect(camera.zoomAt(v / 2), camera.zoom);
          expect(camera.zoomAt(v), camera.zoom);
        });

        test('is a level further out for each doubling of speed', () {
          expect(camera.zoomAt(2 * v), closeTo(camera.zoom - 1, 1e-9));
          // Halving the speed comes a level back in.
          expect(
            camera.zoomAt(1.5 * v) - camera.zoomAt(3 * v),
            closeTo(1, 1e-9),
          );
        });

        test('stops at the least zoom', () {
          expect(camera.zoomAt(4 * v), closeTo(camera.minZoom, 1e-9));
          expect(camera.zoomAt(100 * v), camera.minZoom);
        });
      });
    }

    test("keeps each mode's chevron lag the same at its reference speed", () {
      for (final mode in NavigationMode.values) {
        final camera = followCamera(mode);
        expect(
          camera.referenceSpeed * (1 << (camera.zoom - 16).round()),
          closeTo(lagLimitSpeedAtZoom16, 1e-9),
        );
      }
    });

    test("uses Ferrostar's zooms and tilts", () {
      expect(
        [
          for (final mode in NavigationMode.values)
            (followCamera(mode).zoom, followCamera(mode).pitch),
        ],
        [(16.0, 45.0), (18.0, 45.0), (20.0, 10.0)],
      );
    });
  });

  group('the chevron', () {
    test('sits three quarters down in portrait, clear of the status bar', () {
      final padding = followPadding(
        size: const Size(412, 915),
        mapPadding: const EdgeInsets.only(top: 24),
        systemPadding: const EdgeInsets.only(top: 24, bottom: 24),
        direction: TextDirection.ltr,
      );
      expect(padding, const EdgeInsets.only(top: 915 / 2 - 24));
    });

    test('sits in the end half in landscape', () {
      final padding = followPadding(
        size: const Size(915, 412),
        mapPadding: EdgeInsets.zero,
        systemPadding: EdgeInsets.zero,
        direction: TextDirection.ltr,
      );
      expect(padding, const EdgeInsets.only(left: 915 / 2, top: 206));
    });

    test('keeps above the system bars in a short window', () {
      final padding = followPadding(
        size: const Size(400, 100),
        mapPadding: EdgeInsets.zero,
        systemPadding: const EdgeInsets.only(bottom: 20),
        direction: TextDirection.ltr,
      );
      // Halfway between 50 and 100 − bottom is 100 − 44.
      expect(50 + (100 - 50 - padding.bottom) / 2, closeTo(100 - 44, 1e-9));
    });

    test('glides for as long as fixes are apart, up to a limit', () {
      expect(glideDuration(null), Duration.zero);
      expect(
        glideDuration(const Duration(milliseconds: 1000)),
        const Duration(milliseconds: 1000),
      );
      expect(glideDuration(const Duration(seconds: 30)), maxGlide);
    });
  });
}
