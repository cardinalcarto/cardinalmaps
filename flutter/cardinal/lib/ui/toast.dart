import 'dart:async';

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoButton, CupertinoButtonSize;
import 'package:material_ui/material_ui.dart';

import 'glass.dart';

/// Shows [message] in a glass capsule at the top of the screen, under the status bar,
/// with an [action] button if there is one, the way iOS confirms what just happened:
/// clear of the map's controls and the sheet, it fades away by itself, and a new one
/// takes its place.
void showToast(
  BuildContext context,
  String message, {
  String? action,
  VoidCallback? onAction,
}) {
  final overlay = Overlay.of(context, rootOverlay: true);
  _Toast.current?.dismiss(immediately: true);
  late final OverlayEntry entry;
  final toast = _Toast(
    message: message,
    action: action,
    onAction: onAction,
    remove: () => entry.remove(),
  );
  entry = OverlayEntry(builder: (_) => _ToastView(toast: toast));
  _Toast.current = toast;
  overlay.insert(entry);
}

/// A toast on screen, and the way to take it away.
class _Toast {
  _Toast({
    required this.message,
    required this.action,
    required this.onAction,
    required this.remove,
  });

  static _Toast? current;

  final String message;
  final String? action;
  final VoidCallback? onAction;
  final VoidCallback remove;

  _ToastViewState? _view;
  bool _removed = false;

  /// How long it stays: longer with an action, to have time to take it.
  Duration get duration => Duration(seconds: action == null ? 3 : 5);

  void dismiss({bool immediately = false}) {
    if (_removed) return;
    final view = _view;
    if (immediately || view == null || !view.mounted) {
      _finish();
    } else {
      unawaited(view.hide().then((_) => _finish()));
    }
  }

  void _finish() {
    if (_removed) return;
    _removed = true;
    if (current == this) current = null;
    remove();
  }
}

class _ToastView extends StatefulWidget {
  const _ToastView({required this.toast});

  final _Toast toast;

  @override
  State<_ToastView> createState() => _ToastViewState();
}

class _ToastViewState extends State<_ToastView>
    with SingleTickerProviderStateMixin {
  late final _shown = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 250),
  );
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    widget.toast._view = this;
    unawaited(_shown.forward());
    _timer = Timer(widget.toast.duration, widget.toast.dismiss);
  }

  Future<void> hide() => _shown.reverse();

  @override
  void dispose() {
    _timer?.cancel();
    _shown.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final toast = widget.toast;
    final theme = Theme.of(context);
    final curve = CurvedAnimation(parent: _shown, curve: Curves.easeOutCubic);
    return Positioned(
      left: 16,
      right: 16,
      top: MediaQuery.viewPaddingOf(context).top + 8,
      child: SafeArea(
        top: false,
        bottom: false,
        child: Center(
          child: FadeTransition(
            opacity: curve,
            child: SlideTransition(
              position: Tween(
                begin: const Offset(0, -0.4),
                end: Offset.zero,
              ).animate(curve),
              child: Semantics(
                liveRegion: true,
                child: AppGlassLayer(
                  // As tinted as the map's controls: it's over the map too.
                  tintAmount: 0.8,
                  child: GlassSurface(
                    radius: 24,
                    floating: true,
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(minHeight: 48),
                      child: Padding(
                        padding: EdgeInsetsDirectional.only(
                          start: 20,
                          end: toast.action == null ? 20 : 4,
                        ),
                        child: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Flexible(
                              child: Padding(
                                padding: const EdgeInsets.symmetric(
                                  vertical: 12,
                                ),
                                child: Text(
                                  toast.message,
                                  style: theme.textTheme.bodyMedium?.copyWith(
                                    color: theme.colorScheme.onSurface,
                                  ),
                                ),
                              ),
                            ),
                            if (toast.action case final action?)
                              CupertinoButton(
                                sizeStyle: CupertinoButtonSize.medium,
                                onPressed: () {
                                  toast.dismiss();
                                  toast.onAction?.call();
                                },
                                child: Text(action),
                              ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
