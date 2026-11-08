import 'package:flutter/widgets.dart';

/// The icon font: see `tool/icon_font.py`.
abstract final class AppIcons {
  static const _family = 'CardinalIcons';

  static const saved = IconData(0xE000, fontFamily: _family);
  static const nearby = IconData(0xE001, fontFamily: _family);
  static const search = IconData(0xE002, fontFamily: _family);
  static const transit = IconData(0xE003, fontFamily: _family);
  static const offline = IconData(0xE004, fontFamily: _family);
}
