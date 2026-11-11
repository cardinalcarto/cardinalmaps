import 'package:cupertino_ui/cupertino_ui.dart' show CupertinoIcons;
import 'package:material_ui/material_ui.dart';

import 'adaptive.dart';

/// The icons for common actions: SF Symbols' look (`CupertinoIcons`) on iOS, Material's
/// elsewhere. The app's own icons, its navigation bar's, are `AppIcons`.
class AppSymbols {
  const AppSymbols._(this._ios);

  static const _cupertino = AppSymbols._(true);
  static const _material = AppSymbols._(false);

  static AppSymbols of(BuildContext context) =>
      isCupertinoOf(context) ? _cupertino : _material;

  final bool _ios;

  IconData _pick(IconData cupertino, IconData material) =>
      _ios ? cupertino : material;

  IconData get close => _pick(CupertinoIcons.xmark, Icons.close);
  IconData get clear => _pick(CupertinoIcons.xmark_circle_fill, Icons.cancel);
  IconData get back => _pick(CupertinoIcons.chevron_back, Icons.arrow_back);
  IconData get forward =>
      _pick(CupertinoIcons.chevron_forward, Icons.chevron_right);
  IconData get add => _pick(CupertinoIcons.add, Icons.add);
  IconData get share => _pick(CupertinoIcons.share, Icons.share);
  IconData get more => _pick(CupertinoIcons.ellipsis, Icons.more_vert);
  IconData get swap =>
      _pick(CupertinoIcons.arrow_up_arrow_down, Icons.swap_vert);
  IconData get refresh => _pick(CupertinoIcons.arrow_clockwise, Icons.refresh);
  IconData get save => _pick(CupertinoIcons.heart, Icons.favorite_border);
  IconData get saved => _pick(CupertinoIcons.heart_fill, Icons.favorite);
  IconData get delete => _pick(CupertinoIcons.trash, Icons.delete_outline);
  IconData get edit => _pick(CupertinoIcons.pencil, Icons.edit_outlined);
  IconData get check => _pick(CupertinoIcons.checkmark, Icons.check);
  IconData get search => _pick(CupertinoIcons.search, Icons.search);
  IconData get myLocation => _pick(CupertinoIcons.location, Icons.my_location);
  IconData get layers =>
      _pick(CupertinoIcons.square_stack_3d_up, Icons.layers_outlined);
}
