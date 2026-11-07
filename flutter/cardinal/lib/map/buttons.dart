import 'package:material_ui/material_ui.dart';

import '../sheet/sheet.dart';
import 'layers.dart';
import 'location.dart';

/// The map's buttons, the layer picker's over my location, riding the top of the sheet
/// at the end of the screen. They fade out as the sheet rises past halfway, as the map
/// does under it.
class MapButtons extends StatelessWidget {
  const MapButtons({super.key, this.location = const MyLocationButton()});

  /// The my location button; tests give one with a fake location source.
  final Widget location;

  @override
  Widget build(BuildContext context) {
    final padding = MediaQuery.paddingOf(context);
    return SheetBuilder(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [const LayerButton(), const SizedBox(height: 16), location],
      ),
      builder: (context, sheet, child) {
        final docked = sheet.geometry.placement == SheetPlacement.docked;
        final height = sheet.geometry.size.height;
        final opacity = docked
            ? 1.0
            : (1 - (sheet.extent - height * 0.45) / (height * 0.1)).clamp(
                0.0,
                1.0,
              );
        return Stack(
          children: [
            PositionedDirectional(
              end: 16 + padding.right,
              bottom: 16 + (docked ? padding.bottom : sheet.extent),
              child: IgnorePointer(
                ignoring: opacity == 0,
                child: Opacity(opacity: opacity, child: child),
              ),
            ),
          ],
        );
      },
    );
  }
}
