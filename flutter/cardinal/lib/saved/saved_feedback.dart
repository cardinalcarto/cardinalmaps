import 'dart:math' as math;

import 'package:material_ui/material_ui.dart';

import '../sheet/sheet.dart';
import '../ui/adaptive.dart';
import '../ui/toast.dart';

/// Shows [message]: on iOS a glass toast at the top of the screen; elsewhere a snack
/// bar above what the sheet covers, over the map above a partly raised sheet or at the
/// bottom of a full one, clear of its bars and [above] more (at the bottom of the
/// screen without a sheet).
void showSheetMessage(
  BuildContext context,
  String message, {
  String? action,
  VoidCallback? onAction,
  double above = 0,
}) {
  if (isCupertinoOf(context)) {
    showToast(context, message, action: action, onAction: onAction);
    return;
  }
  final view = SheetScaffold.maybeViewOf(context);
  final double bottom;
  if (view == null) {
    bottom = MediaQuery.viewPaddingOf(context).bottom;
  } else {
    final geometry = view.geometry;
    final full = view.extent >= geometry.maxExtent - 1;
    // Above the sheet, or the bottom bar when the sheet's under it or covers it all.
    bottom = full
        ? geometry.padding.bottom + SheetScaffold.bottomBarHeightOf(context)
        : math.max(view.extent, view.bottomBarCover);
  }
  ScaffoldMessenger.of(context)
    ..hideCurrentSnackBar()
    ..showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
        persist: false,
        margin: view == null
            ? null
            : EdgeInsets.fromLTRB(8, 0, 8, bottom + above + 8),
        action: action == null
            ? null
            : SnackBarAction(label: action, onPressed: onAction ?? () {}),
      ),
    );
}
