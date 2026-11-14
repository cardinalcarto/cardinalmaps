/// A route's vehicles as markers on the map: a disc in the route's color with its kind of
/// vehicle on it, and, under it, a pointer turned the way the vehicle is heading.
library;

import 'dart:ui' as ui;

import 'package:material_ui/material_ui.dart';
import 'package:sparkle/sparkle.dart';

import '../directions/transit_plan.dart';
import '../directions/transit_sheet.dart' show parseRouteColor, vehicleIcon;
import '../directions/transit_timeline.dart' show routeTextColor;

/// Draws [route]'s vehicle marker and heading pointer as sprite images, named
/// [vehicleMarkerName] and [vehicleHeadingName].
typedef VehicleMarkerPainter = Future<List<SpriteImageInput>> Function(
  TransitRoute? route,
);

/// The color a route without one of its own is drawn in on the map.
const defaultRouteColor = Color(0xff3b81de);

/// [route]'s color on the map.
Color mapRouteColor(TransitRoute? route) =>
    parseRouteColor(route?.color) ?? defaultRouteColor;

String _hex(Color color) =>
    (color.toARGB32() & 0xffffff).toRadixString(16).padLeft(6, '0');

/// The sprite image of [route]'s vehicle marker: its color, text color, and vehicle.
String vehicleMarkerName(TransitRoute? route) {
  final color = mapRouteColor(route);
  return 'transit_vehicle_${_hex(color)}_'
      '${_hex(routeTextColor(route, color))}_'
      '${(route?.kind ?? VehicleKind.other).name}';
}

/// The sprite image of [route]'s heading pointer, pointing north.
String vehicleHeadingName(TransitRoute? route) =>
    'transit_heading_${_hex(mapRouteColor(route))}';

/// Both images' size, in logical pixels: the pointer reaches to the edge.
const _size = 36.0;
const _discRadius = 10.0;
const _ringWidth = 2.0;
const _glyphSize = 14.0;

/// The device's [VehicleMarkerPainter], drawing at the first view's pixel ratio.
Future<List<SpriteImageInput>> paintVehicleMarkers(TransitRoute? route) async {
  final ratio =
      WidgetsBinding.instance.platformDispatcher.views.first.devicePixelRatio;
  final color = mapRouteColor(route);
  return [
    await _image(vehicleMarkerName(route), ratio, (canvas) {
      const center = Offset(_size / 2, _size / 2);
      canvas
        ..drawCircle(
          center.translate(0, 1),
          _discRadius + _ringWidth,
          Paint()
            ..color = const Color(0x40000000)
            ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 1.5),
        )
        ..drawCircle(
          center,
          _discRadius + _ringWidth,
          Paint()..color = Colors.white,
        )
        ..drawCircle(center, _discRadius, Paint()..color = color);
      final icon = vehicleIcon(route?.kind ?? VehicleKind.other);
      final glyph = TextPainter(
        textDirection: TextDirection.ltr,
        text: TextSpan(
          text: String.fromCharCode(icon.codePoint),
          style: TextStyle(
            fontFamily: icon.fontFamily,
            package: icon.fontPackage,
            fontSize: _glyphSize,
            height: 1,
            color: routeTextColor(route, color),
          ),
        ),
      )..layout();
      glyph.paint(canvas, center - Offset(glyph.width / 2, glyph.height / 2));
      glyph.dispose();
    }),
    await _image(vehicleHeadingName(route), ratio, (canvas) {
      // A point above the disc, which covers its base.
      const mid = _size / 2;
      Path pointer(double inset) => Path()
        ..moveTo(mid, 2 + inset)
        ..lineTo(mid - 7 + inset, 13)
        ..lineTo(mid + 7 - inset, 13)
        ..close();
      canvas
        ..drawPath(pointer(0), Paint()..color = Colors.white)
        ..drawPath(pointer(_ringWidth), Paint()..color = color);
    }),
  ];
}

Future<SpriteImageInput> _image(
  String name,
  double ratio,
  void Function(Canvas canvas) draw,
) async {
  final size = (_size * ratio).ceil();
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder)..scale(ratio);
  draw(canvas);
  final picture = recorder.endRecording();
  final image = await picture.toImage(size, size);
  picture.dispose();
  // Premultiplied, as sprites are.
  final bytes = await image.toByteData();
  image.dispose();
  return SpriteImageInput(
    name: name,
    pixels: bytes!.buffer.asUint8List(bytes.offsetInBytes, bytes.lengthInBytes),
    width: size,
    height: size,
    pixelRatio: ratio,
  );
}
