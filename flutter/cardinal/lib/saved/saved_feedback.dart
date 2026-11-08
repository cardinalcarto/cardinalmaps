import 'package:material_ui/material_ui.dart';

import '../sheet/sheet.dart';

/// Shows [message] above what the sheet covers: over the map above a partly raised
/// sheet, or at the bottom of a full one, clear of its bars and [above] more.
void showSheetMessage(
  BuildContext context,
  String message, {
  String? action,
  VoidCallback? onAction,
  double above = 0,
}) {
  final view = SheetScaffold.viewOf(context);
  final geometry = view.geometry;
  final full = view.extent >= geometry.maxExtent - 1;
  final bottom = full
      ? geometry.padding.bottom + SheetScaffold.bottomBarHeightOf(context)
      : view.extent;
  ScaffoldMessenger.of(context)
    ..hideCurrentSnackBar()
    ..showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
        persist: false,
        margin: EdgeInsets.fromLTRB(8, 0, 8, bottom + above + 8),
        action: action == null
            ? null
            : SnackBarAction(label: action, onPressed: onAction ?? () {}),
      ),
    );
}
