import 'package:material_ui/material_ui.dart';

import 'downloads/engine.dart';

/// Provides offline downloads to the pages below it: the Offline tab and drawing an
/// area to download.
class OfflineScope extends InheritedWidget {
  const OfflineScope({
    super.key,
    required this.downloads,
    required this.freeBytes,
    required super.child,
  });

  /// Downloads, once open; null if this build has none (no `CARDINAL_RELEASES_URL`).
  final Future<OfflineDownloads?> downloads;

  /// How many bytes downloads may still write where they go.
  final Future<int> Function() freeBytes;

  static OfflineScope of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<OfflineScope>()!;

  @override
  bool updateShouldNotify(OfflineScope oldWidget) =>
      downloads != oldWidget.downloads || freeBytes != oldWidget.freeBytes;
}
