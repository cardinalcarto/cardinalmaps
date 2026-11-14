import 'dart:math' as math;

import 'package:cardinal/navigation/navigation_camera.dart';
import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:routing/routing.dart';

void main() {
  group('the zoom from speed', () {
    const kmh = 1 / 3.6;

    for (final mode in [NavigationMode.driving, NavigationMode.cycling]) {
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
          // Half a level for each factor of √2, and so on.
          expect(
            camera.zoomAt(1.2 * v) - camera.zoomAt(1.2 * math.sqrt2 * v),
            closeTo(0.5, 1e-9),
          );
        });

        test('stops at the least zoom', () {
          expect(camera.zoomAt(100 * v), camera.minZoom);
        });
      });
    }

    test('driving holds z16 to 30 km/h, and is about z14 at 100', () {
      final camera = followCamera(NavigationMode.driving);
      expect(camera.zoomAt(30 * kmh), 16);
      expect(camera.zoomAt(60 * kmh), closeTo(15, 1e-9));
      expect(camera.zoomAt(100 * kmh), closeTo(14.26, 0.01));
      expect(camera.zoomAt(120 * kmh), closeTo(14, 1e-9));
      expect(camera.zoomAt(200 * kmh), 14);
    });

    test('cycling holds z18 to 20 km/h, and goes no further out than z17', () {
      final camera = followCamera(NavigationMode.cycling);
      expect(camera.zoomAt(20 * kmh), 18);
      expect(camera.zoomAt(30 * kmh), closeTo(17.42, 0.01));
      expect(camera.zoomAt(40 * kmh), closeTo(17, 1e-9));
      expect(camera.zoomAt(60 * kmh), 17);
    });

    test('walking never zooms out', () {
      final camera = followCamera(NavigationMode.walking);
      for (final speed in [null, 0.0, 1.5, 5.0, 30.0, 1000.0]) {
        expect(camera.zoomAt(speed), 20);
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

  group("the chevron's lag", () {
    const kmh = 1 / 3.6;

    test('zooming out only shrinks it', () {
      for (final mode in NavigationMode.values) {
        final camera = followCamera(mode);
        for (var speed = 0.0; speed <= 60; speed += 0.5) {
          expect(
            chevronLag(speed, camera.zoomAt(speed)),
            lessThanOrEqualTo(chevronLag(speed, camera.zoom)),
            reason: '${mode.name} at $speed m/s',
          );
        }
      }
    });

    test('driving keeps it about 12 px above 30 km/h, under the disk', () {
      final camera = followCamera(NavigationMode.driving);
      double lag(double kmPerHour) =>
          chevronLag(kmPerHour * kmh, camera.zoomAt(kmPerHour * kmh));
      expect(lag(30), closeTo(12.08, 0.01));
      expect(lag(50), closeTo(12.08, 0.01));
      expect(lag(100), closeTo(12.08, 0.01));
      // Past the floor it grows again, slowly.
      expect(lag(130), closeTo(13.09, 0.01));
      for (var v = 0.0; v <= 200; v += 5) {
        expect(lag(v), lessThanOrEqualTo(chevronLagLimit));
      }
    });

    test('cycling keeps it under the disk to about 45 km/h', () {
      final camera = followCamera(NavigationMode.cycling);
      double lag(double kmPerHour) =>
          chevronLag(kmPerHour * kmh, camera.zoomAt(kmPerHour * kmh));
      expect(lag(20), closeTo(32.2, 0.1));
      expect(lag(40), closeTo(32.2, 0.1));
      expect(lag(44), lessThanOrEqualTo(chevronLagLimit));
      expect(lag(46), greaterThan(chevronLagLimit));
    });

    test('walking keeps it under the disk to about 5.6 km/h', () {
      final camera = followCamera(NavigationMode.walking);
      double lag(double kmPerHour) =>
          chevronLag(kmPerHour * kmh, camera.zoomAt(kmPerHour * kmh));
      expect(lag(5.5), lessThanOrEqualTo(chevronLagLimit));
      expect(lag(5.7), greaterThan(chevronLagLimit));
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
