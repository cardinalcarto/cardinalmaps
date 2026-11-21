import 'package:material_ui/material_ui.dart';

import '../sheet/sheet.dart';
import '../ui/adaptive.dart';
import '../ui/glass.dart';
import 'compass.dart';
import 'layers.dart';
import 'location.dart';
import 'map_control.dart';

/// The map's buttons, the layer picker's over my location, with the compass above them
/// while the map isn't north up, riding the top of the sheet at the end of the screen. They fade out as the sheet rises past halfway, as the map
/// does under it.
class MapButtons extends StatelessWidget {
  const MapButtons({super.key, this.location = const MyLocationButton()});

  /// The my location button; tests give one with a fake location source.
  final Widget location;

  @override
  Widget build(BuildContext context) {
    final padding = MediaQuery.paddingOf(context);
    final cupertino = isCupertinoOf(context);
    return SheetBuilder(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const CompassButton(),
          SizedBox(height: cupertino ? 12 : 16),
          _buttons(cupertino),
        ],
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
              // Over the sheet, or the bottom bar while the sheet's below it.
              bottom: 16 + (docked ? padding.bottom : sheet.obscured.bottom),
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

  /// The layer and my location buttons: on iOS one glass capsule, the way it groups a
  /// map's controls.
  Widget _buttons(bool cupertino) => cupertino
      ? AppGlassLayer(
          // More tinted than the rest: the map behind it can be anything.
          tintAmount: 0.8,
          child: GlassSurface(
            radius: MapControl.size / 2,
            floating: true,
            child: SizedBox(
              width: MapControl.size,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const LayerButton(),
                  const Divider(indent: 10, endIndent: 10),
                  location,
                ],
              ),
            ),
          ),
        )
      : Column(
          mainAxisSize: MainAxisSize.min,
          children: [const LayerButton(), const SizedBox(height: 16), location],
        );
}
