import 'package:flutter/services.dart';

/// The app's haptics, named for when to use them rather than how they feel.
abstract final class Haptics {
  /// A choice changed: a segment, a picker's value.
  static void selection() => HapticFeedback.selectionClick();

  /// Something snapped into place, or a small action took.
  static void impact() => HapticFeedback.lightImpact();

  /// Something the user did was kept, as when a place is saved.
  static void success() => HapticFeedback.mediumImpact();
}
